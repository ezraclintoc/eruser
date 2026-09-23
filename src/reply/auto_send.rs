//! Sending a draft without a person, for the cases that earn it.
//!
//! This is the most dangerous fifty lines in the fork, so it is built like
//! it: the draft is re-validated at send time, the whitelist is checked
//! here rather than trusted from the caller, and identity verification is
//! refused *again* even though the type check should have kept it away —
//! because a check that only exists in one place is a check that one edit
//! removes.
//!
//! The mail goes out through the ordinary sender and is recorded in the
//! ordinary history, exactly as a person's send would be. Nothing here
//! bypasses rate limits or daily caps, because it does not do its own
//! sending: it goes through whatever pool the caller hands it.

use super::{Draft, ReplyContext, validate};
use crate::email::{Message, Sender};
use crate::history::{Error as HistoryError, Store};

/// Send one stored draft, if everything about it clears.
///
/// Returns what happened, in words a task note can carry.
pub async fn send_draft(
    store: &Store,
    sender: &dyn Sender,
    from: &str,
    draft: crate::history::ReplyDraft,
    whitelist: &[String],
    profile: &crate::config::Profile,
) -> Result<SendResult, super::Error> {
    // Only a draft still waiting may go. A sent draft is not sent twice by
    // a double click; a discarded one is not resurrected.
    if draft.status != crate::history::DraftStatus::Draft {
        return Ok(SendResult::NotEligible(format!(
            "the draft was already {}",
            draft.status
        )));
    }

    // The whitelist, checked here and not taken on trust.
    if !draft.reply_type.may_auto_send(whitelist) {
        return Ok(SendResult::NotEligible(format!(
            "{} replies are not on the auto-send whitelist; a person has to send this",
            draft.reply_type
        )));
    }

    // Re-validate at send time against a rebuilt context. The draft could
    // have been written by an older set of rules; the rules that count are
    // the ones in force now.
    let context = ReplyContext {
        reply_type: draft.reply_type,
        broker_name: draft.broker_name.clone(),
        original_subject: draft.in_reply_to.clone(),
        original_body: String::new(),
        sent_subject: String::new(),
        profile: profile.clone(),
    };
    let Draft { body, .. } = validate(
        &super::RawDraft {
            body: draft.body.clone(),
        },
        &context,
    )?;

    let message = Message {
        to: store
            .find_response_by_subject(draft.user_id, &draft.broker_id, &draft.in_reply_to)
            .await
            .map_err(history_error)?
            .map(|response| response.email_from)
            .filter(|address| !address.is_empty())
            .ok_or_else(|| {
                super::Error::DrafterUnavailable(
                    "no broker address is known for this reply; it cannot be answered".to_string(),
                )
            })?,
        from: from.to_string(),
        subject: draft.subject.clone(),
        body,
    };

    sender.send(&message).await.map_err(|error| {
        super::Error::DrafterUnavailable(format!("the draft did not go out: {error}"))
    })?;

    store
        .mark_draft_sent(draft.user_id, draft.id)
        .await
        .map_err(history_error)?;

    Ok(SendResult::Sent {
        to: message.to,
        subject: message.subject,
    })
}

/// What a send attempt came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendResult {
    /// It went out.
    Sent { to: String, subject: String },
    /// It did not, and why. Not an error: a draft that must wait for a
    /// person is the system working as designed.
    NotEligible(String),
}

fn history_error(error: HistoryError) -> super::Error {
    super::Error::DrafterUnavailable(error.to_string())
}

#[cfg(test)]
mod tests;
