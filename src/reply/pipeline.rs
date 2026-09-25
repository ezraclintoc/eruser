//! From replies in history to drafts on the task list.
//!
//! Deliberately decoupled from the scan loop: the monitor's job is to read
//! the mailbox and file what it finds, and drafting reads *history* — the
//! reply bodies are stored precisely so they can be re-read without going
//! back to the mailbox. That keeps a slow sidecar from holding a mailbox
//! connection open, and means re-running the drafter after the classifier
//! changes needs no new mail.
//!
//! Two things happen per reply, and both have a rules-only answer:
//!
//! - **Which reply** it deserves — a table (an unfinished reply gets the
//!   missing-information reply), or a decision model when one is configured.
//! - **What that reply says** — the pre-written wording that fits, or a
//!   model writing one, per the setting.
//!
//! Every failure here is a log line, not an error: a sidecar that is
//! switched off, unreachable, or unable to produce anything usable leaves
//! the reply exactly where it would have been without any of this — in
//! history, and on the task list as plain review.

use super::choices;
use super::routing::Router;
use super::{Drafter, ReplyContext, ReplyType, validate};
use crate::config::{Profile, Wording};
use crate::decision::Decider;
use crate::history::{DraftStatus, NewReplyDraft, ResponseFilter, Store};
use crate::template::Engine;

/// Everything a drafting run needs, gathered once by the caller.
///
/// All of it is optional in practice: with no drafter and no decider, and the
/// rule table deciding, this is the behaviour the tool had before any model
/// was involved.
pub struct Drafting<'a> {
    /// Writes the reply, when the wording is not pre-written.
    pub drafter: Option<&'a dyn Drafter>,
    /// Chooses between answers eruser supplies, when one is configured.
    pub decider: Option<&'a dyn Decider>,
    /// Which of the two produces the words.
    pub wording: Wording,
    /// An answer below this confidence is thrown away.
    pub min_confidence: f32,
    /// Whether the decider may choose the reply type, over the rule table.
    pub route: bool,
}

impl<'a> Drafting<'a> {
    /// The settings that predate the decision model: a drafting sidecar
    /// writes the reply, and the rule table chooses the type.
    pub fn generated(drafter: &'a dyn Drafter) -> Self {
        Self {
            drafter: Some(drafter),
            decider: None,
            wording: Wording::Generated,
            min_confidence: crate::decision::DEFAULT_MIN_CONFIDENCE,
            route: false,
        }
    }

    /// Build the settings from the two config sections, with whichever
    /// clients the caller managed to construct.
    ///
    /// The clients are borrowed rather than built here, because building
    /// them yields an `Arc` that has to outlive the run — the caller owns
    /// those, exactly as it owns the store and the sender.
    pub fn from_config(
        ai: &crate::config::AiConfig,
        decider: &crate::config::DeciderConfig,
        drafter: Option<&'a dyn Drafter>,
        decider_client: Option<&'a dyn Decider>,
    ) -> Self {
        Self {
            drafter,
            decider: decider_client,
            wording: ai.wording,
            min_confidence: decider.min_confidence,
            route: decider.route,
        }
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
    drafting: &Drafting<'_>,
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

    let router = Router {
        decider: drafting.decider,
        enabled: drafting.route,
        min_confidence: drafting.min_confidence,
    };

    // Parsed once for the whole run rather than per reply. A malformed
    // template is a bug in a shipped candidate, which a test catches long
    // before this; this is the belt to that pair of braces.
    let engine = match drafting.wording {
        Wording::Canned => Some(
            Engine::new().map_err(|problem| crate::reply::Error::Wording(problem.to_string()))?,
        ),
        Wording::Generated => None,
    };

    for response in &responses {
        if summary.drafted >= max_drafts as usize {
            break;
        }

        let Some(reply_type) = router.reply_for(response).await else {
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

        let composed = match compose(drafting, &context, engine.as_ref()).await {
            Ok(composed) => composed,
            Err(problem) => {
                tracing::warn!(
                    broker = %response.broker_id,
                    %problem,
                    "could not prepare a reply"
                );
                continue;
            }
        };

        match validate(
            &super::RawDraft {
                body: composed.body,
            },
            &context,
        ) {
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
                        model: composed.producer,
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
                    "the reply was refused"
                );
            }
        }
    }

    Ok(summary)
}

/// The wording for one reply, and what produced it.
struct Composed {
    body: String,
    /// Recorded on the draft, e.g. `canned:provide_details` or `qwen3:4b`, so
    /// the task page says where the words came from.
    producer: String,
}

/// Produce the body of one reply, by the route the settings chose.
async fn compose(
    drafting: &Drafting<'_>,
    context: &ReplyContext,
    engine: Option<&Engine>,
) -> Result<Composed, crate::reply::Error> {
    match drafting.wording {
        Wording::Canned => compose_from_library(drafting, context, engine).await,
        Wording::Generated => compose_from_model(drafting, context).await,
    }
}

/// Take the pre-written reply that fits best.
///
/// The decider, when there is one, only names a candidate: the words it
/// picks from are eruser's own, so nothing a model produces can reach the
/// email.
async fn compose_from_library(
    drafting: &Drafting<'_>,
    context: &ReplyContext,
    engine: Option<&Engine>,
) -> Result<Composed, crate::reply::Error> {
    let state = crate::decision::state_of(
        &context.broker_name,
        &context.original_subject,
        &context.original_body,
    );

    let chosen = choices::choose(
        drafting.decider,
        drafting.min_confidence,
        context.reply_type,
        &state,
    )
    .await
    .ok_or_else(|| {
        crate::reply::Error::Wording(format!(
            "no pre-written reply exists for {}",
            context.reply_type.label()
        ))
    })?;

    let engine = engine.ok_or_else(|| {
        crate::reply::Error::Wording("the wording templates were not loaded".to_string())
    })?;

    let body = choices::render(
        engine,
        chosen.candidate,
        &context.profile,
        &context.broker_name,
    )
    .map_err(|problem| crate::reply::Error::Wording(problem.to_string()))?;

    tracing::info!(
        broker = %context.broker_name,
        candidate = chosen.candidate.name,
        chosen_by = chosen.by,
        "using a pre-written reply"
    );

    Ok(Composed {
        body,
        producer: chosen.producer(),
    })
}

/// Have a drafting model write the body.
async fn compose_from_model(
    drafting: &Drafting<'_>,
    context: &ReplyContext,
) -> Result<Composed, crate::reply::Error> {
    let drafter = drafting.drafter.ok_or_else(|| {
        crate::reply::Error::DrafterUnavailable("no drafting model is configured".to_string())
    })?;

    let raw = drafter.draft(context).await?;
    Ok(Composed {
        body: raw.body,
        producer: drafter.name().to_string(),
    })
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
