//! SMTP transport, built on lettre.
//!
//! Ported from `internal/email/smtp.go`, which hand-rolled the MIME envelope
//! and the TLS handshake. lettre handles both, so this module is mostly
//! configuration and error classification.

use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message as LettreMessage, Tokio1Executor};

use super::{Error, Message, Sent, generate_message_id, validate_message};
use crate::config::SmtpConfig;

/// Implicit TLS ("SMTPS"). Any other port with TLS enabled uses STARTTLS.
const IMPLICIT_TLS_PORT: u16 = 465;

pub struct SmtpSender {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: String,
}

impl std::fmt::Debug for SmtpSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deliberately does not print the transport, which holds credentials.
        f.debug_struct("SmtpSender")
            .field("from", &self.from)
            .finish()
    }
}

impl SmtpSender {
    pub fn new(config: SmtpConfig, from: String) -> Result<Self, Error> {
        if config.host.is_empty() {
            return Err(Error::Configuration("no SMTP host configured".into()));
        }
        if config.port == 0 {
            return Err(Error::Configuration("no SMTP port configured".into()));
        }

        let mut builder = if config.use_tls {
            if config.port == IMPLICIT_TLS_PORT {
                AsyncSmtpTransport::<Tokio1Executor>::relay(&config.host).map_err(|_| Error::Tls)?
            } else {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.host)
                    .map_err(|_| Error::Tls)?
            }
        } else {
            // Credentials over a cleartext connection would be sent in the
            // clear, so refuse rather than silently leak them.
            if !config.username.is_empty() {
                return Err(Error::Configuration(
                    "SMTP authentication requires TLS; set use_tls: true".into(),
                ));
            }
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&config.host)
        }
        .port(config.port);

        if !config.username.is_empty() {
            builder = builder.credentials(Credentials::new(
                config.username.clone(),
                config.password.clone(),
            ));
        }

        Ok(Self {
            transport: builder.build(),
            from,
        })
    }

    /// The address messages are sent from, for callers that need to display it.
    pub fn from(&self) -> &str {
        &self.from
    }
}

#[async_trait::async_trait]
impl super::Sender for SmtpSender {
    async fn send(&self, message: &Message) -> Result<Sent, Error> {
        validate_message(message)?;

        let message_id = generate_message_id(&message.from);
        let envelope = LettreMessage::builder()
            .from(message.from.parse().map_err(|_| {
                Error::Invalid(super::ValidationError::Sender(Box::new(
                    super::ValidationError::Malformed(message.from.clone()),
                )))
            })?)
            .to(message.to.parse().map_err(|_| {
                Error::Invalid(super::ValidationError::Recipient(Box::new(
                    super::ValidationError::Malformed(message.to.clone()),
                )))
            })?)
            .subject(&message.subject)
            .message_id(Some(message_id.clone()))
            .header(lettre::message::header::ContentType::TEXT_PLAIN)
            .body(message.body.clone())
            .map_err(|e| Error::Configuration(e.to_string()))?;

        let response = self.transport.send(envelope).await.map_err(classify)?;

        Ok(Sent {
            message_id,
            response: response.message().collect::<Vec<_>>().join(" "),
        })
    }

    fn name(&self) -> &'static str {
        "smtp"
    }
}

/// Map a transport failure onto a category the user can act on.
///
/// The raw error is deliberately dropped rather than wrapped: lettre includes
/// the server's response text, which for some providers echoes back the
/// username, and these errors are surfaced in the web UI and written to logs.
fn classify(error: lettre::transport::smtp::Error) -> Error {
    if error.is_permanent() || error.is_transient() {
        // A response code came back, so the connection itself was fine.
        if let Some(code) = error.status() {
            let severity = code.severity as u8;
            let category = code.category as u8;
            // 4xx/5xx with category 3 or code 535 is an authentication problem.
            if (severity == 5 || severity == 4) && category == 3 {
                return Error::Authentication;
            }
            return Error::Rejected(format!("{severity}.{category}"));
        }
    }
    if error.is_tls() {
        return Error::Tls;
    }
    if error.is_client() {
        return Error::Configuration("the message was rejected before sending".into());
    }
    Error::Connection
}
