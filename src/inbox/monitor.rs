//! Reading the mailbox over IMAP.
//!
//! Ported from `internal/inbox/monitor.go`.
//!
//! The connection is read-only in every path here: eruser fetches messages
//! and classifies them, and never deletes anything. Upstream had a
//! `MoveToFolder` that archived processed mail; that is deliberately left
//! out for now, because a misfiring matcher moving real mail out of someone's
//! inbox is a much worse failure than leaving it where it is.

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use futures::StreamExt;

use super::Email;
use crate::broker::Broker;
use crate::config::InboxConfig;

/// Messages fetched per round trip. Whole mailboxes can be large, and asking
/// for everything at once makes a slow server time out.
const BATCH_SIZE: usize = 50;

/// How long to wait on the server before giving up.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not reach the mail server at {server}:{port}")]
    Connect {
        server: String,
        port: u16,
        #[source]
        source: std::io::Error,
    },

    #[error("the mail server rejected the sign-in — check the address and app password")]
    Login,

    #[error("could not open the folder {folder:?}")]
    Folder {
        folder: String,
        #[source]
        source: async_imap::error::Error,
    },

    #[error("the mail server did not answer as expected")]
    Imap(#[from] async_imap::error::Error),

    #[error("inbox monitoring is not configured")]
    NotConfigured(#[from] crate::config::ValidationError),
}

/// A connection to the mailbox, plus the broker directory used to work out
/// who a message is from.
pub struct Monitor {
    config: InboxConfig,
    /// Sender domain to broker. Built from both the contact address and the
    /// website, because plenty of brokers reply from a different address than
    /// the one they publish.
    brokers_by_domain: HashMap<String, Broker>,
    session: Option<Session>,
}

/// async-imap leaves TLS to the caller, so the stream type is spelled out.
type TlsStream = tokio_rustls::client::TlsStream<tokio::net::TcpStream>;
type Session = async_imap::Session<TlsStream>;

impl std::fmt::Debug for Monitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The config holds an app password.
        f.debug_struct("Monitor")
            .field("server", &self.config.server)
            .field("folder", &self.config.folder)
            .field("brokers", &self.brokers_by_domain.len())
            .field("connected", &self.session.is_some())
            .finish()
    }
}

impl Monitor {
    pub fn new(config: InboxConfig, brokers: &[Broker]) -> Self {
        Self {
            config,
            brokers_by_domain: broker_domains(brokers),
            session: None,
        }
    }

    /// How many sender domains are known.
    pub fn known_domains(&self) -> usize {
        self.brokers_by_domain.len()
    }

    /// Which broker, if any, a sender domain belongs to.
    pub fn broker_for_domain(&self, domain: &str) -> Option<&Broker> {
        self.brokers_by_domain.get(&domain.to_lowercase())
    }

    pub fn is_connected(&self) -> bool {
        self.session.is_some()
    }

    /// Connect over TLS and sign in.
    pub async fn connect(&mut self) -> Result<(), Error> {
        self.config.validate()?;

        let stream = tokio::time::timeout(
            TIMEOUT,
            tokio::net::TcpStream::connect((self.config.server.as_str(), self.config.port)),
        )
        .await
        .map_err(|_| Error::Connect {
            server: self.config.server.clone(),
            port: self.config.port,
            source: std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the mail server did not answer in time",
            ),
        })?
        .map_err(|source| Error::Connect {
            server: self.config.server.clone(),
            port: self.config.port,
            source,
        })?;

        let server_name = rustls::pki_types::ServerName::try_from(self.config.server.clone())
            .map_err(|_| Error::Connect {
                server: self.config.server.clone(),
                port: self.config.port,
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "the server name is not a valid hostname",
                ),
            })?;

        let tls = tokio_rustls::TlsConnector::from(tls_config())
            .connect(server_name, stream)
            .await
            .map_err(|source| Error::Connect {
                server: self.config.server.clone(),
                port: self.config.port,
                source,
            })?;

        let client = async_imap::Client::new(tls);
        // The error carries the server's response, which can echo the
        // username; the message here says what to do instead.
        let session = client
            .login(&self.config.email, &self.config.password)
            .await
            .map_err(|_| Error::Login)?;

        self.session = Some(session);
        Ok(())
    }

    /// Sign out. Ignores a failure, since the connection is going away anyway.
    pub async fn disconnect(&mut self) {
        if let Some(mut session) = self.session.take() {
            let _ = session.logout().await;
        }
    }

    /// Every message in the configured folder from the last `days` days.
    pub async fn recent_emails(&mut self, days: i64) -> Result<Vec<Email>, Error> {
        let folder = self.config.folder.clone();
        self.emails_in_folder(&folder, days).await
    }

    /// Every message in `folder` from the last `days` days.
    pub async fn emails_in_folder(&mut self, folder: &str, days: i64) -> Result<Vec<Email>, Error> {
        let brokers = self.brokers_by_domain.clone();
        let session = self.session.as_mut().ok_or(Error::Login)?;

        // Read-only: nothing here should mark messages as seen, let alone
        // move or delete them.
        session
            .examine(folder)
            .await
            .map_err(|source| Error::Folder {
                folder: folder.to_string(),
                source,
            })?;

        let since = (Utc::now() - Duration::days(days.max(0)))
            .format("%d-%b-%Y")
            .to_string();
        let uids = session.uid_search(format!("SINCE {since}")).await?;

        if uids.is_empty() {
            return Ok(Vec::new());
        }

        let mut uids: Vec<u32> = uids.into_iter().collect();
        uids.sort_unstable();

        let mut emails = Vec::new();
        for batch in uids.chunks(BATCH_SIZE) {
            let set = batch
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",");

            let mut stream = session.uid_fetch(set, "(UID ENVELOPE BODY.PEEK[])").await?;
            while let Some(message) = stream.next().await {
                let message = match message {
                    Ok(message) => message,
                    Err(error) => {
                        // One unreadable message must not lose the rest of
                        // the batch.
                        tracing::warn!(%error, "skipping a message that could not be fetched");
                        continue;
                    }
                };

                if let Some(email) = parse_message(&message, &brokers) {
                    emails.push(email);
                }
            }
        }

        Ok(emails)
    }

    /// Only the messages that came from a known broker.
    pub async fn broker_emails(&mut self, days: i64) -> Result<Vec<Email>, Error> {
        Ok(self
            .recent_emails(days)
            .await?
            .into_iter()
            .filter(|email| !email.broker_id.is_empty())
            .collect())
    }
}

/// Build the sender-domain directory.
///
/// Both the contact address and the website are indexed: a broker that
/// receives at `privacy@acme-data.example` may well reply from
/// `noreply@acme.example`, and matching only the contact address files that
/// reply as coming from nobody.
pub fn broker_domains(brokers: &[Broker]) -> HashMap<String, Broker> {
    let mut map = HashMap::new();

    for broker in brokers {
        if let Some((_, domain)) = broker.email.rsplit_once('@') {
            let domain = domain.trim().to_lowercase();
            if !domain.is_empty() {
                map.entry(domain).or_insert_with(|| broker.clone());
            }
        }

        if let Some(domain) = domain_of(&broker.website) {
            map.entry(domain).or_insert_with(|| broker.clone());
        }
    }

    map
}

/// The host part of a URL, without `www.`.
pub fn domain_of(website: &str) -> Option<String> {
    let trimmed = website
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.");

    let host = trimmed.split('/').next()?.split(':').next()?;
    (!host.is_empty()).then(|| host.to_lowercase())
}

/// Turn a fetched message into an [`Email`].
fn parse_message(
    message: &async_imap::types::Fetch,
    brokers: &HashMap<String, Broker>,
) -> Option<Email> {
    let envelope = message.envelope()?;

    let mut email = Email {
        uid: message.uid.unwrap_or_default(),
        subject: decode_header(envelope.subject.as_deref()),
        message_id: decode_header(envelope.message_id.as_deref()),
        ..Default::default()
    };

    if let Some(from) = envelope.from.as_ref().and_then(|list| list.first()) {
        let mailbox = decode_header(from.mailbox.as_deref());
        let host = decode_header(from.host.as_deref());

        email.from_name = decode_header(from.name.as_deref());
        email.from_domain = host.to_lowercase();
        email.from = if mailbox.is_empty() || host.is_empty() {
            String::new()
        } else {
            format!("{mailbox}@{host}")
        };
    }

    if let Some(broker) = brokers.get(&email.from_domain) {
        email.broker_id = broker.id.clone();
        email.broker_name = broker.name.clone();
    }

    if let Some(body) = message.body() {
        apply_body(&mut email, body);
    }

    Some(email)
}

/// Pull the text and HTML parts, and the date, out of the raw message.
fn apply_body(email: &mut Email, raw: &[u8]) {
    let Some(parsed) = mail_parser::MessageParser::default().parse(raw) else {
        // Headers were readable even if the body was not; classification on
        // the subject alone is better than dropping the message.
        return;
    };

    // body_text and body_html each synthesize the part they are asked for
    // when the message carries only the other one, so matching on the part's
    // actual type is what keeps them apart. Otherwise a plain-text reply
    // arrives with an html_body that is just its own text wrapped in tags,
    // and every message looks like it had both.
    if let Some(part) = parsed.text_bodies().next()
        && let mail_parser::PartType::Text(text) = &part.body
    {
        email.body = text.as_ref().to_string();
    }
    if let Some(part) = parsed.html_bodies().next()
        && let mail_parser::PartType::Html(html) = &part.body
    {
        email.html_body = html.as_ref().to_string();
    }
    email.received_at = parsed
        .date()
        .and_then(|date| DateTime::from_timestamp(date.to_timestamp(), 0));
}

/// IMAP header values arrive as raw bytes, sometimes RFC 2047 encoded.
fn decode_header(raw: Option<&[u8]>) -> String {
    let Some(bytes) = raw else {
        return String::new();
    };
    let text = String::from_utf8_lossy(bytes);

    if text.contains("=?") {
        // Wrapping in a header line lets the mail parser do the decoding.
        let header = format!("Subject: {text}\r\n\r\n");
        if let Some(parsed) = mail_parser::MessageParser::default().parse(header.as_bytes())
            && let Some(subject) = parsed.subject()
        {
            return subject.to_string();
        }
    }

    text.into_owned()
}

#[cfg(test)]
mod tests;

/// A TLS configuration trusting the systems own certificate store.
///
/// Built once: loading the platform roots is not cheap, and every reconnect
/// would otherwise pay for it again.
fn tls_config() -> std::sync::Arc<rustls::ClientConfig> {
    static CONFIG: std::sync::OnceLock<std::sync::Arc<rustls::ClientConfig>> =
        std::sync::OnceLock::new();

    CONFIG
        .get_or_init(|| {
            let mut roots = rustls::RootCertStore::empty();
            match rustls_native_certs::load_native_certs() {
                result if result.certs.is_empty() => {
                    // No system store: fall back to the roots compiled in, so
                    // a minimal container still connects.
                    tracing::warn!(
                        errors = result.errors.len(),
                        "no system certificates found; using the built-in roots"
                    );
                    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
                }
                result => {
                    for cert in result.certs {
                        let _ = roots.add(cert);
                    }
                }
            }

            std::sync::Arc::new(
                rustls::ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth(),
            )
        })
        .clone()
}
