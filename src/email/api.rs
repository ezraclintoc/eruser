//! Sending through a transactional email provider.
//!
//! Upstream advertised these — `config.example.yaml` documents a SendGrid
//! section, `CLAUDE.md` lists three providers, and `go.mod` pulls in both
//! client libraries — but `internal/email/` only ever contained `sender.go`
//! and `smtp.go`, and `NewSender` rejected anything that was not `smtp`.
//!
//! They are worth having for one reason: an API key is a single value the
//! user pastes in. The SMTP path needs two-factor authentication turned on,
//! a Google app password generated, and a host and port that mean nothing to
//! most people — and that setup is where anyone trying this gives up.

use serde::Serialize;

use super::{Error, Message, Sender, Sent, generate_message_id, validate_message};

/// How long to wait on the provider.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Which provider to send through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Resend,
    SendGrid,
}

impl Provider {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Resend => "resend",
            Self::SendGrid => "sendgrid",
        }
    }

    const fn endpoint(self) -> &'static str {
        match self {
            Self::Resend => "https://api.resend.com/emails",
            Self::SendGrid => "https://api.sendgrid.com/v3/mail/send",
        }
    }

    /// Where to get a key, for the message shown when one is missing.
    pub const fn signup_url(self) -> &'static str {
        match self {
            Self::Resend => "https://resend.com/api-keys",
            Self::SendGrid => "https://app.sendgrid.com/settings/api_keys",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "resend" => Some(Self::Resend),
            "sendgrid" => Some(Self::SendGrid),
            _ => None,
        }
    }
}

impl std::fmt::Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Resend's request shape.
#[derive(Debug, Serialize)]
struct ResendRequest<'a> {
    from: &'a str,
    to: [&'a str; 1],
    subject: &'a str,
    text: &'a str,
    /// Resend passes custom headers through, which is how the Message-ID
    /// eruser generates survives to the broker and back on their reply.
    headers: std::collections::BTreeMap<&'static str, &'a str>,
}

/// SendGrid's request shape, which is rather more elaborate.
#[derive(Debug, Serialize)]
struct SendGridRequest<'a> {
    personalizations: [SendGridPersonalization<'a>; 1],
    from: SendGridAddress<'a>,
    subject: &'a str,
    content: [SendGridContent<'a>; 1],
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    headers: std::collections::BTreeMap<&'static str, &'a str>,
}

#[derive(Debug, Serialize)]
struct SendGridPersonalization<'a> {
    to: [SendGridAddress<'a>; 1],
}

#[derive(Debug, Serialize)]
struct SendGridAddress<'a> {
    email: &'a str,
}

#[derive(Debug, Serialize)]
struct SendGridContent<'a> {
    #[serde(rename = "type")]
    content_type: &'static str,
    value: &'a str,
}

/// Sends through a provider's HTTP API.
pub struct ApiSender {
    client: reqwest::Client,
    provider: Provider,
    api_key: String,
    from: String,
}

impl std::fmt::Debug for ApiSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the key.
        f.debug_struct("ApiSender")
            .field("provider", &self.provider)
            .field("from", &self.from)
            .finish()
    }
}

impl ApiSender {
    pub fn new(provider: Provider, api_key: String, from: String) -> Result<Self, Error> {
        if api_key.trim().is_empty() {
            return Err(Error::Configuration(format!(
                "no {provider} API key configured. Create one at {}",
                provider.signup_url()
            )));
        }

        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|_| Error::Configuration("could not build an HTTP client".into()))?;

        Ok(Self {
            client,
            provider,
            api_key,
            from,
        })
    }

    pub fn from(&self) -> &str {
        &self.from
    }

    /// The JSON body for one message.
    fn body(&self, message: &Message, message_id: &str) -> serde_json::Value {
        let headers = std::collections::BTreeMap::from([("Message-ID", message_id)]);

        match self.provider {
            Provider::Resend => serde_json::to_value(ResendRequest {
                from: &message.from,
                to: [&message.to],
                subject: &message.subject,
                text: &message.body,
                headers,
            }),
            Provider::SendGrid => serde_json::to_value(SendGridRequest {
                personalizations: [SendGridPersonalization {
                    to: [SendGridAddress { email: &message.to }],
                }],
                from: SendGridAddress {
                    email: &message.from,
                },
                subject: &message.subject,
                content: [SendGridContent {
                    content_type: "text/plain",
                    value: &message.body,
                }],
                headers,
            }),
        }
        .unwrap_or(serde_json::Value::Null)
    }
}

#[async_trait::async_trait]
impl Sender for ApiSender {
    async fn send(&self, message: &Message) -> Result<Sent, Error> {
        validate_message(message)?;

        let message_id = generate_message_id(&message.from);
        let response = self
            .client
            .post(self.provider.endpoint())
            .bearer_auth(&self.api_key)
            .json(&self.body(message, &message_id))
            .send()
            .await
            .map_err(classify_transport)?;

        let status = response.status();
        // Read the body before deciding, since a provider's explanation of a
        // rejection is the only useful thing in it.
        let body = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(classify_status(status.as_u16(), &body));
        }

        Ok(Sent {
            message_id,
            response: provider_reference(&body).unwrap_or_else(|| status.to_string()),
        })
    }

    fn name(&self) -> &'static str {
        match self.provider {
            Provider::Resend => "resend",
            Provider::SendGrid => "sendgrid",
        }
    }
}

/// The provider's own id for the message, if it gave one.
fn provider_reference(body: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    json.get("id")?.as_str().map(str::to_string)
}

/// Map an HTTP status onto something the user can act on.
///
/// The response body is not passed through: it is the provider's, it can
/// echo back the From address and parts of the request, and these strings
/// reach the web UI and the logs.
fn classify_status(status: u16, body: &str) -> Error {
    match status {
        401 | 403 => Error::Authentication,
        // A provider will not send from an address you have not proved you
        // own, and that is the commonest first failure with these services.
        422 if body.contains("domain") || body.contains("from") => Error::Configuration(
            "the provider will not send from that address — verify the domain \
             or sender in its dashboard first"
                .into(),
        ),
        400 | 422 => Error::Rejected("the provider rejected the message".into()),
        429 => Error::Rejected("the provider is rate limiting; try again later".into()),
        500..=599 => Error::Connection,
        other => Error::Rejected(format!("the provider answered {other}")),
    }
}

fn classify_transport(error: reqwest::Error) -> Error {
    if error.is_timeout() || error.is_connect() {
        Error::Connection
    } else {
        Error::Rejected("the request to the provider failed".into())
    }
}

#[cfg(test)]
mod tests;
