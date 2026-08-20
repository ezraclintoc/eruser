//! Fetch, classify, store.
//!
//! The Go version did this inline in `cmd/eraser/main.go` for the CLI and
//! again in `internal/web/server.go` for the web UI. As with the send
//! pipeline, there is one implementation here and callers supply a progress
//! sink.

use super::classifier::{ClassifiedResponse, ResponseType, Summary};
use super::{Email, Monitor, classifier, monitor};
use crate::history::{DEFAULT_USER_ID, NewBrokerResponse, PipelineStatus, Store};

/// How far back to look when nothing else is asked for.
pub const DEFAULT_DAYS: i64 = 7;

#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// How many days of mail to read.
    pub days: i64,
    pub user_id: i64,
    /// Store and classify mail that matched no known broker.
    ///
    /// Off by default: a mailbox holds a lot that has nothing to do with
    /// eruser, and filing it all as broker replies would bury the real ones.
    pub include_unmatched: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            days: DEFAULT_DAYS,
            user_id: DEFAULT_USER_ID,
            include_unmatched: false,
        }
    }
}

/// What one scan did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanSummary {
    /// Messages read from the mailbox.
    pub fetched: usize,
    /// Of those, the ones from a known broker.
    pub matched: usize,
    /// Rows written or updated.
    pub stored: usize,
    /// Brokers whose pipeline stage moved.
    pub advanced: usize,
    /// Addresses that came back as undeliverable.
    pub bounced: usize,
    pub by_type: Summary,
}

/// Progress through one scan.
#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    Connected,
    Fetched {
        count: usize,
    },
    Classified {
        index: usize,
        total: usize,
        broker_name: String,
        response_type: ResponseType,
        confidence: f64,
    },
    Finished(ScanSummary),
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Monitor(#[from] monitor::Error),

    #[error(transparent)]
    History(#[from] crate::history::Error),
}

/// Read the mailbox, classify what is there, and record it.
pub async fn scan(
    monitor: &mut Monitor,
    store: &Store,
    options: &ScanOptions,
    mut progress: impl FnMut(Progress),
) -> Result<ScanSummary, Error> {
    monitor.connect().await?;
    progress(Progress::Connected);

    let emails = monitor.recent_emails(options.days).await?;
    monitor.disconnect().await;
    progress(Progress::Fetched {
        count: emails.len(),
    });

    let mut summary = ScanSummary {
        fetched: emails.len(),
        ..Default::default()
    };

    let considered: Vec<&Email> = emails
        .iter()
        .filter(|email| options.include_unmatched || !email.broker_id.is_empty())
        .collect();

    let total = considered.len();
    let mut classified = Vec::with_capacity(total);

    for (index, email) in considered.into_iter().enumerate() {
        if !email.broker_id.is_empty() {
            summary.matched += 1;
        }

        let result = classifier::classify(email);
        if result.response_type == ResponseType::Bounced {
            summary.bounced += 1;
        }

        progress(Progress::Classified {
            index: index + 1,
            total,
            broker_name: display_name(email),
            response_type: result.response_type,
            confidence: result.confidence,
        });

        if store_response(store, options.user_id, email, &result).await? {
            summary.stored += 1;
        }
        if advance_pipeline(store, options.user_id, email, result.response_type).await? {
            summary.advanced += 1;
        }

        classified.push(result);
    }

    summary.by_type = classifier::summarize(&classified);
    progress(Progress::Finished(summary));

    Ok(summary)
}

/// Write one classified reply to history.
///
/// Returns whether anything was stored: a message that matched no broker is
/// skipped, since a response row keyed to no broker cannot be acted on.
async fn store_response(
    store: &Store,
    user_id: i64,
    email: &Email,
    result: &ClassifiedResponse,
) -> Result<bool, crate::history::Error> {
    if email.broker_id.is_empty() {
        return Ok(false);
    }

    store
        .upsert_broker_response(&NewBrokerResponse {
            user_id,
            broker_id: email.broker_id.clone(),
            broker_name: display_name(email),
            response_type: result.response_type.into(),
            email_from: email.from.clone(),
            email_subject: email.subject.clone(),
            // Kept so a later classifier change can re-read this reply
            // without going back to the mailbox.
            email_body: email.text(),
            form_url: result.form_url.clone().unwrap_or_default(),
            confirm_url: result.confirm_url.clone().unwrap_or_default(),
            confidence: result.confidence,
            needs_review: result.needs_review,
            received_at: email.received_at,
        })
        .await?;

    Ok(true)
}

/// Move the broker's pipeline stage to match what the reply said.
async fn advance_pipeline(
    store: &Store,
    user_id: i64,
    email: &Email,
    response_type: ResponseType,
) -> Result<bool, crate::history::Error> {
    if email.broker_id.is_empty() {
        return Ok(false);
    }

    store
        .update_pipeline_status(user_id, &email.broker_id, stage_for(response_type))
        .await
}

/// Which pipeline stage a reply puts the broker in.
pub fn stage_for(response_type: ResponseType) -> PipelineStatus {
    match response_type {
        ResponseType::Success => PipelineStatus::Confirmed,
        ResponseType::FormRequired => PipelineStatus::FormRequired,
        ResponseType::ConfirmationRequired => PipelineStatus::AwaitingConfirmation,
        ResponseType::Rejected => PipelineStatus::Rejected,
        // A bounce means the request never arrived, which is a failure of
        // the send rather than a refusal by the broker.
        ResponseType::Bounced => PipelineStatus::Failed,
        // Acknowledged, or unreadable: either way something more is coming.
        ResponseType::Pending | ResponseType::Unknown => PipelineStatus::AwaitingResponse,
    }
}

/// Re-read every stored reply with the current classifier.
///
/// Worth running after the patterns change: replies whose bodies were kept
/// are classified in full, and the rest fall back to their subject lines.
pub async fn reclassify_stored(store: &Store, user_id: i64) -> Result<usize, Error> {
    let stored = store
        .broker_responses(user_id, crate::history::ResponseFilter::default())
        .await?;

    let mut changed = 0;

    for response in stored {
        let (response_type, confidence, needs_review, form_url, confirm_url) =
            if response.email_body.is_empty() {
                // No body kept, so the subject is all there is.
                let (response_type, confidence, needs_review) =
                    classifier::classify_by_subject(&response.email_subject);
                (
                    response_type,
                    confidence,
                    needs_review,
                    response.form_url.clone(),
                    response.confirm_url.clone(),
                )
            } else {
                let email = Email {
                    from: response.email_from.clone(),
                    from_domain: domain_of_address(&response.email_from),
                    subject: response.email_subject.clone(),
                    body: response.email_body.clone(),
                    broker_id: response.broker_id.clone(),
                    broker_name: response.broker_name.clone(),
                    ..Default::default()
                };
                let result = classifier::classify(&email);
                (
                    result.response_type,
                    result.confidence,
                    result.needs_review,
                    result.form_url.unwrap_or_default(),
                    result.confirm_url.unwrap_or_default(),
                )
            };

        let stored_type: crate::history::ResponseType = response_type.into();
        if stored_type == response.response_type
            && (confidence - response.confidence).abs() < f64::EPSILON
            && needs_review == response.needs_review
            && form_url == response.form_url
            && confirm_url == response.confirm_url
        {
            continue;
        }

        store
            .update_response_classification(
                response.id,
                stored_type,
                &form_url,
                &confirm_url,
                confidence,
                needs_review,
            )
            .await?;
        store
            .update_pipeline_status(user_id, &response.broker_id, stage_for(response_type))
            .await?;

        changed += 1;
    }

    Ok(changed)
}

/// The best name available for the sender.
fn display_name(email: &Email) -> String {
    if !email.broker_name.is_empty() {
        email.broker_name.clone()
    } else if !email.from_name.is_empty() {
        email.from_name.clone()
    } else if !email.from.is_empty() {
        email.from.clone()
    } else {
        "unknown sender".to_string()
    }
}

fn domain_of_address(address: &str) -> String {
    address
        .rsplit_once('@')
        .map(|(_, domain)| domain.to_lowercase())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
