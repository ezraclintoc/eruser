//! Outbound email: the sender abstraction and its SMTP implementation.
//!
//! Ported from `internal/email/`.

use crate::config::EmailConfig;

mod api;
mod error;
mod smtp;

pub use api::{ApiSender, Provider};
pub use error::{Error, ValidationError};
pub use smtp::SmtpSender;

/// One removal request, addressed and rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub to: String,
    pub from: String,
    pub subject: String,
    pub body: String,
}

/// What a successful send produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    /// The RFC 5322 Message-ID this request was sent with, including angle
    /// brackets. Stored in history so a later reply can be matched back to
    /// the request it answers via In-Reply-To / References.
    pub message_id: String,
    /// The transport's response line, for diagnostics. May be empty.
    pub response: String,
}

/// A transport that can deliver a [`Message`].
///
/// Kept object-safe via `async_trait` so the CLI, the web job runner, and
/// tests can all hold a `Box<dyn Sender>` and swap in a fake.
#[async_trait::async_trait]
pub trait Sender: Send + Sync {
    async fn send(&self, message: &Message) -> Result<Sent, Error>;

    /// Short transport name, as recorded in history and shown in the UI.
    fn name(&self) -> &'static str;
}

/// Build the sender named by the config.
pub fn new_sender(config: &EmailConfig) -> Result<Box<dyn Sender>, Error> {
    match config.provider.as_str() {
        "" | "smtp" => Ok(Box::new(SmtpSender::new(
            config.smtp.clone(),
            config.from.clone(),
        )?)),
        "resend" => Ok(Box::new(ApiSender::new(
            Provider::Resend,
            config.resend.api_key.clone(),
            config.from.clone(),
        )?)),
        "sendgrid" => Ok(Box::new(ApiSender::new(
            Provider::SendGrid,
            config.sendgrid.api_key.clone(),
            config.from.clone(),
        )?)),
        other => Err(Error::UnknownProvider(other.to_string())),
    }
}

/// A sender that reports success without contacting anything, for `--dry-run`.
#[derive(Debug, Default, Clone, Copy)]
pub struct DryRunSender;

#[async_trait::async_trait]
impl Sender for DryRunSender {
    async fn send(&self, message: &Message) -> Result<Sent, Error> {
        // Validate anyway: a dry run that accepts an address the real
        // transport would reject is not much of a preview.
        validate_message(message)?;
        Ok(Sent {
            message_id: String::new(),
            response: "dry run: not sent".to_string(),
        })
    }

    fn name(&self) -> &'static str {
        "dry-run"
    }
}

/// Reject addresses that could smuggle extra SMTP headers, then check the
/// address actually parses.
///
/// The comma and semicolon checks matter because a broker database entry is
/// community-contributed: `a@b.example, attacker@evil.example` must not turn
/// one recipient into two.
pub fn validate_email(address: &str) -> Result<(), ValidationError> {
    if address.contains(['\r', '\n', ',', ';']) {
        return Err(ValidationError::IllegalCharacters(address.to_string()));
    }
    address
        .parse::<lettre::message::Mailbox>()
        .map(|_| ())
        .map_err(|_| ValidationError::Malformed(address.to_string()))
}

/// Validate the parts of a message that end up in headers.
pub fn validate_message(message: &Message) -> Result<(), ValidationError> {
    validate_email(&message.from).map_err(|e| ValidationError::Sender(Box::new(e)))?;
    validate_email(&message.to).map_err(|e| ValidationError::Recipient(Box::new(e)))?;
    if message.subject.contains(['\r', '\n']) {
        return Err(ValidationError::SubjectLineBreak);
    }
    Ok(())
}

/// Generate an RFC 5322 Message-ID, using the sender's domain when it has one.
pub(crate) fn generate_message_id(from: &str) -> String {
    let domain = from
        .rsplit_once('@')
        .map(|(_, d)| d)
        .unwrap_or("eruser.local");
    format!("<{}@{}>", uuid::Uuid::new_v4(), domain)
}

#[cfg(test)]
pub(crate) mod tests;
