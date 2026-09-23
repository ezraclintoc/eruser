//! From replies in history to drafts on the task list.
//!
//! Deliberately decoupled from the scan loop: the monitor's job is to read
//! the mailbox and file what it finds, and drafting reads *history* — the
//! reply bodies are stored precisely so they can be re-read without going
//! back to the mailbox. That keeps a slow sidecar from holding a mailbox
//! connection open, and means re-running the drafter after the classifier
//! changes needs no new mail.
//!
//! Every failure here is a log line, not an error: a sidecar that is
//! switched off, unreachable, or unable to produce anything usable leaves
//! the reply exactly where it would have been without drafting — in
//! history, and on the task list as plain review.

use super::{Drafter, ReplyContext, ReplyType, validate};
use crate::config::Profile;
use crate::history::{DraftStatus, NewReplyDraft, ResponseFilter, Store};

/// Which stored replies deserve a drafted answer.
///
/// A pending reply means the broker is working on the request but asked
/// something in passing, or acknowledged without concluding — the commonest
/// shape of "reply to discuss". Adding a verdict here is what makes a new
/// kind of reply get drafted; nothing else needs to change.
fn draftable(response_type: crate::history::ResponseType) -> Option<ReplyType> {
    match response_type {
        crate::history::ResponseType::Pending => Some(ReplyType::MissingInfo),
        _ => None,
    }
}

/// How a drafting run went.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DraftRunSummary {
    /// Replies considered, before any cap.
    pub considered: usize,
    /// Drafts actually written and kept.
    pub drafted: usize,
    /// Replies that already had a draft.
    pub already_answered: usize,
    /// Attempts the validator refused.
    pub refused: usize,
}

/// Draft replies for everything in history that deserves one.
///
/// `profile` is the person the replies were sent for; the model may use
/// those facts and no others. Returns what happened, for the progress
/// output.
pub async fn draft_replies(
    store: &Store,
    drafter: &dyn Drafter,
    profile: &Profile,
    user_id: i64,
    max_drafts: u32,
) -> Result<DraftRunSummary, crate::reply::Error> {
    let responses = store
        .broker_responses(user_id, ResponseFilter::default())
        .await
        .map_err(history_error)?;
    let mut summary = DraftRunSummary {
        considered: responses.len(),
        ..Default::default()
    };

    for response in &responses {
        if summary.drafted >= max_drafts as usize {
            break;
        }

        let Some(reply_type) = draftable(response.response_type) else {
            continue;
        };

        if store
            .has_draft(
                user_id,
                &response.broker_id,
                &response.email_subject,
                reply_type,
            )
            .await
            .map_err(history_error)?
        {
            summary.already_answered += 1;
            continue;
        }

        let context = ReplyContext {
            reply_type,
            broker_name: response.broker_name.clone(),
            original_subject: response.email_subject.clone(),
            original_body: response.email_body.clone(),
            sent_subject: String::new(),
            profile: profile.clone(),
        };

        match drafter.draft(&context).await {
            Ok(raw) => match validate(&raw, &context) {
                Ok(draft) => {
                    let id = store
                        .upsert_draft(&NewReplyDraft {
                            user_id,
                            broker_id: response.broker_id.clone(),
                            broker_name: response.broker_name.clone(),
                            reply_type,
                            in_reply_to: response.email_subject.clone(),
                            subject: reply_type.subject_for(&response.email_subject),
                            body: draft.body,
                            model: drafter.name().to_string(),
                            validation: draft.validation,
                        })
                        .await
                        .map_err(history_error)?;

                    let stored = store.draft(user_id, id).await.map_err(history_error)?;
                    store
                        .queue_draft_task(&stored)
                        .await
                        .map_err(history_error)?;
                    summary.drafted += 1;
                }
                Err(problem) => {
                    summary.refused += 1;
                    tracing::warn!(
                        broker = %response.broker_id,
                        %problem,
                        "the drafting model's reply was refused"
                    );
                }
            },
            Err(problem) => {
                tracing::warn!(
                    broker = %response.broker_id,
                    %problem,
                    "could not draft a reply"
                );
            }
        }
    }

    Ok(summary)
}

/// Whether a stored draft may go out on its own, per the whitelist.
///
/// Called by whichever path does the sending; this module only answers the
/// question, and never puts mail on the wire itself.
pub fn may_auto_send(reply_type: ReplyType, whitelist: &[String]) -> bool {
    reply_type.may_auto_send(whitelist)
}

/// Count the drafts still waiting, for pages that want to say so.
pub async fn pending_count(store: &Store, user_id: i64) -> Result<usize, crate::history::Error> {
    Ok(store
        .pending_drafts(user_id)
        .await?
        .iter()
        .filter(|draft| draft.status == DraftStatus::Draft)
        .count())
}

/// The pipeline reads history and writes drafts; its own error type only
/// knows about the sidecar, so store failures are carried verbatim.
fn history_error(error: crate::history::Error) -> crate::reply::Error {
    crate::reply::Error::DrafterUnavailable(error.to_string())
}

#[cfg(test)]
mod tests;
