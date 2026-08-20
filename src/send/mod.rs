//! The send pipeline: render a request per broker, send it, record it.
//!
//! In the Go version this loop existed twice — once in `cmd/eraser/main.go`
//! for the CLI and once in `internal/web/job.go` for the web UI — and the two
//! copies had already drifted: the web copy chunked sends across days to stay
//! under Gmail's limit and the CLI copy did not. Here there is one
//! implementation, and the caller supplies a progress sink.

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::broker::Broker;
use crate::config::Profile;
use crate::email::{self, Message, Sender};
use crate::history::{DEFAULT_USER_ID, NewRecord, Store};
use crate::template::Engine;

/// Knobs for one run of the pipeline.
#[derive(Debug, Clone)]
pub struct SendOptions {
    /// Template name: `gdpr`, `ccpa`, or `generic`.
    pub template: String,
    /// The From address on every message.
    pub from: String,
    /// Pause between sends, to stay clear of provider rate limits.
    pub rate_limit: Duration,
    /// Stop after this many sends. `None` means no cap.
    ///
    /// Gmail cuts off around 500 messages a day; the web UI passes a cap so a
    /// 750-broker run does not die halfway through with no way to resume.
    pub daily_limit: Option<usize>,
    pub user_id: i64,
}

impl Default for SendOptions {
    fn default() -> Self {
        Self {
            template: crate::template::DEFAULT_TEMPLATE.to_string(),
            from: String::new(),
            rate_limit: Duration::from_millis(crate::config::DEFAULT_RATE_LIMIT_MS),
            daily_limit: None,
            user_id: DEFAULT_USER_ID,
        }
    }
}

/// What happened for one broker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Sent {
        message_id: String,
    },
    Failed {
        error: String,
    },
    /// The daily cap was reached before this broker's turn.
    SkippedOverLimit,
}

/// One step of a run, handed to the caller's progress sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    Started {
        total: usize,
    },
    Broker {
        /// 1-based position in the run.
        index: usize,
        total: usize,
        broker_id: String,
        broker_name: String,
        broker_email: String,
        outcome: Outcome,
    },
    Finished(Summary),
}

/// Totals for a completed (or cancelled) run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    pub sent: usize,
    pub failed: usize,
    pub skipped: usize,
    /// True when the run stopped early because it was cancelled.
    pub cancelled: bool,
}

impl Summary {
    pub fn attempted(&self) -> usize {
        self.sent + self.failed
    }
}

/// Everything the pipeline needs to run.
pub struct SendJob {
    pub brokers: Vec<Broker>,
    pub profile: Profile,
    pub engine: Arc<Engine>,
    pub sender: Arc<dyn Sender>,
    /// `None` records nothing, for a preview that should leave no trace.
    pub store: Option<Store>,
    pub options: SendOptions,
}

impl SendJob {
    /// Run the pipeline, reporting each step to `progress`.
    ///
    /// A single broker failing never aborts the run: a bad address in a
    /// 750-entry community database must not stop the other 749 requests.
    pub async fn run(
        self,
        cancel: &CancellationToken,
        mut progress: impl FnMut(Progress),
    ) -> Summary {
        let total = self.brokers.len();
        progress(Progress::Started { total });

        let mut summary = Summary::default();

        for (index, broker) in self.brokers.iter().enumerate() {
            if cancel.is_cancelled() {
                summary.cancelled = true;
                summary.skipped += total - index;
                break;
            }

            let over_limit = self
                .options
                .daily_limit
                .is_some_and(|limit| summary.attempted() >= limit);

            let outcome = if over_limit {
                summary.skipped += 1;
                Outcome::SkippedOverLimit
            } else {
                let outcome = self.process(broker).await;
                match &outcome {
                    Outcome::Sent { .. } => summary.sent += 1,
                    Outcome::Failed { .. } => summary.failed += 1,
                    Outcome::SkippedOverLimit => summary.skipped += 1,
                }
                outcome
            };

            progress(Progress::Broker {
                index: index + 1,
                total,
                broker_id: broker.id.clone(),
                broker_name: broker.name.clone(),
                broker_email: broker.email.clone(),
                outcome,
            });

            // No point sleeping after the last broker, or after one that was
            // skipped without contacting anything.
            let is_last = index + 1 == total;
            if !is_last && !over_limit && !self.options.rate_limit.is_zero() {
                // Waiting inside select! means a cancel takes effect at once
                // rather than after the full delay.
                tokio::select! {
                    _ = tokio::time::sleep(self.options.rate_limit) => {}
                    _ = cancel.cancelled() => {}
                }
            }
        }

        progress(Progress::Finished(summary));
        summary
    }

    /// Render, send, and record one request.
    async fn process(&self, broker: &Broker) -> Outcome {
        let email = match self
            .engine
            .render(&self.options.template, &self.profile, broker)
        {
            Ok(email) => email,
            Err(error) => return self.record_failure(broker, &error_chain(&error)).await,
        };

        let message = Message {
            to: broker.email.clone(),
            from: self.options.from.clone(),
            subject: email.subject,
            body: email.body,
        };

        match self.sender.send(&message).await {
            Ok(sent) => {
                let record = NewRecord::sent(
                    &broker.id,
                    &broker.name,
                    &broker.email,
                    &self.options.template,
                    &sent.message_id,
                )
                .for_user(self.options.user_id);
                self.record(record).await;
                Outcome::Sent {
                    message_id: sent.message_id,
                }
            }
            Err(error) => self.record_failure(broker, &error_chain(&error)).await,
        }
    }

    async fn record_failure(&self, broker: &Broker, error: &str) -> Outcome {
        let record = NewRecord::failed(
            &broker.id,
            &broker.name,
            &broker.email,
            &self.options.template,
            error,
        )
        .for_user(self.options.user_id);
        self.record(record).await;
        Outcome::Failed {
            error: error.to_string(),
        }
    }

    /// Write to history, if there is a store.
    ///
    /// A history write failing must not turn a delivered email into a
    /// reported failure — the broker already has the request either way — so
    /// this logs and moves on.
    async fn record(&self, record: NewRecord) {
        let Some(store) = &self.store else {
            return;
        };
        if let Err(error) = store.add_record(&record).await {
            tracing::warn!(
                broker_id = %record.broker_id,
                %error,
                "failed to record a removal request in history"
            );
        }
    }
}

/// Flatten an error and its `source()` chain into a single line.
///
/// Rust errors nest: the top level reads "invalid recipient address" and the
/// address that caused it lives one level down. These strings are written to
/// history and shown in the UI, where that detail is the whole point.
pub fn error_chain(error: &dyn std::error::Error) -> String {
    let mut parts = vec![error.to_string()];
    let mut current = error.source();
    while let Some(source) = current {
        let text = source.to_string();
        // Skip a link that only restates the one above it.
        if parts.last() != Some(&text) {
            parts.push(text);
        }
        current = source.source();
    }
    parts.join(": ")
}

/// Build the sender a run should use: the real transport, or a no-op one for
/// a dry run.
pub fn sender_for(
    config: &crate::config::EmailConfig,
    dry_run: bool,
) -> Result<Arc<dyn Sender>, email::Error> {
    if dry_run {
        Ok(Arc::new(email::DryRunSender))
    } else {
        Ok(Arc::from(email::new_sender(config)?))
    }
}

#[cfg(test)]
mod tests;
