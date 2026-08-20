//! Reading and classifying what brokers send back.
//!
//! Ported from `internal/inbox/`. Roughly a fifth of brokers reply asking for
//! something more — a form, a confirmation click, proof of identity — and
//! this is the part that reads those replies and sorts them.

use chrono::{DateTime, Utc};

pub mod classifier;
pub mod monitor;
pub mod parser;

pub use classifier::{ClassifiedResponse, ResponseType, classify, classify_by_subject};
pub use monitor::Monitor;
pub use parser::ExtractedUrls;

/// One message fetched from the mailbox.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Email {
    /// IMAP UID, for archiving or marking the message afterwards.
    pub uid: u32,
    pub message_id: String,
    pub from: String,
    /// The display name, e.g. "Mail Delivery System".
    pub from_name: String,
    pub from_domain: String,
    pub subject: String,
    pub body: String,
    pub html_body: String,
    pub received_at: Option<DateTime<Utc>>,
    /// The broker this reply was matched to, if one was found.
    pub broker_id: String,
    pub broker_name: String,
}

impl Email {
    /// The text to classify: the plain part, or the HTML with tags removed.
    pub fn text(&self) -> String {
        if !self.body.is_empty() {
            self.body.clone()
        } else {
            collapse_whitespace(&decode_entities(&parser::strip_tags(&self.html_body)))
        }
    }
}

/// Decode the handful of entities that appear in broker mail.
fn decode_entities(text: &str) -> String {
    text.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        // Last, so a doubly-encoded "&amp;lt;" does not become "<".
        .replace("&amp;", "&")
}

/// Squeeze runs of whitespace into single spaces.
fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
