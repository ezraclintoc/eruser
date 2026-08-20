//! Clicking the confirmation links brokers send.
//!
//! Ported from `internal/browser/confirm.go`. This is plain HTTP: the links
//! are ordinary GETs, and driving a whole browser to fetch one would be
//! wasted machinery.

use std::collections::HashSet;
use std::time::Duration;

use super::captcha::{self, Captcha};
use crate::broker::Broker;

/// Cap on how much of a response is read. These pages are a paragraph of
/// text; anything larger is not a confirmation page.
const MAX_BODY: usize = 64 * 1024;

/// How long to wait on a broker's server.
const TIMEOUT: Duration = Duration::from_secs(30);

/// A recent Chrome, because some brokers refuse anything that admits to being
/// a script — including, at times, the very confirmation link they sent.
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
     AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// Wording that means the confirmation went through.
const SUCCESS_MARKERS: &[&str] = &[
    "successfully",
    "confirmed",
    "verification complete",
    "verified",
    "opt-out complete",
    "removal complete",
    "request received",
    "request confirmed",
    "has been removed",
    "been deleted",
    "been processed",
    "unsubscribed",
    "opted out",
];

/// Wording that means it did not.
const FAILURE_MARKERS: &[&str] = &[
    "link expired",
    "link has expired",
    "link invalid",
    "invalid link",
    "no longer valid",
    "error occurred",
    "something went wrong",
    "could not",
    "unable to",
];

/// Wording that means it had already been done.
const ALREADY_MARKERS: &[&str] = &[
    "already confirmed",
    "already been confirmed",
    "already processed",
    "already removed",
    "already opted out",
];

/// How a confirmation attempt turned out.
///
/// Not Eq: a Captcha carries a confidence, and comparing floats for exact
/// equality is not a comparison worth offering.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The broker acknowledged it.
    Confirmed,
    /// It had already been confirmed — which is the same outcome, arrived at
    /// earlier. Go filed this as a failure, so re-running `confirm` turned
    /// finished work into reported errors.
    AlreadyConfirmed,
    /// The link had a lifetime and it ran out.
    Expired,
    /// The link was rejected outright.
    Invalid,
    /// The page is behind a challenge, so a person has to open it.
    Blocked(Captcha),
    /// It answered, but said nothing either way.
    Unclear,
    /// It could not be fetched at all.
    Failed(String),
}

impl Outcome {
    /// Whether the confirmation can be considered done.
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Confirmed | Self::AlreadyConfirmed)
    }

    /// Whether a person now has to do something.
    pub fn needs_a_person(&self) -> bool {
        matches!(self, Self::Blocked(_) | Self::Unclear)
    }

    /// One line for the terminal or the UI.
    pub fn summary(&self) -> String {
        match self {
            Self::Confirmed => "confirmed".to_string(),
            Self::AlreadyConfirmed => "already confirmed".to_string(),
            Self::Expired => "the link had expired".to_string(),
            Self::Invalid => "the link was rejected".to_string(),
            Self::Blocked(captcha) => {
                format!("blocked by a challenge — {}", captcha.instructions())
            }
            Self::Unclear => "the page said nothing either way — worth a look".to_string(),
            Self::Failed(reason) => format!("could not open the link: {reason}"),
        }
    }
}

/// What happened when a link was followed.
#[derive(Debug, Clone)]
pub struct Confirmation {
    pub url: String,
    /// Where the redirects ended up.
    pub final_url: String,
    pub status: Option<u16>,
    pub outcome: Outcome,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{url} is not a valid link")]
    BadUrl { url: String },

    #[error(
        "{host} does not belong to any broker in the database.\n\n\
         This link may not have come from a broker at all. Pass \
         --no-validate-domain to follow it anyway."
    )]
    UntrustedDomain { host: String },

    #[error("could not build an HTTP client")]
    Client(#[source] reqwest::Error),
}

/// Follows confirmation links, optionally checking they lead somewhere known.
pub struct Confirmer {
    client: reqwest::Client,
    /// Domains belonging to brokers in the database.
    broker_domains: HashSet<String>,
}

impl std::fmt::Debug for Confirmer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Confirmer")
            .field("broker_domains", &self.broker_domains.len())
            .finish()
    }
}

impl Confirmer {
    pub fn new(brokers: &[Broker]) -> Result<Self, Error> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(TIMEOUT)
            // Brokers chain several redirects between the link and the
            // confirmation page; ten is generous and stops a loop.
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .map_err(Error::Client)?;

        Ok(Self {
            client,
            broker_domains: domains_of(brokers),
        })
    }

    pub fn known_domains(&self) -> usize {
        self.broker_domains.len()
    }

    /// Whether a link leads to a domain belonging to a known broker.
    ///
    /// Subdomains count: brokers routinely send confirmation links from
    /// `links.acme.example` when the database lists `acme.example`.
    pub fn is_broker_domain(&self, url: &str) -> Result<bool, Error> {
        let host = host_of(url)?;

        Ok(self.broker_domains.contains(&host)
            || self
                .broker_domains
                .iter()
                .any(|domain| host.ends_with(&format!(".{domain}"))))
    }

    /// Follow a confirmation link.
    ///
    /// `validate_domain` should stay on. These URLs come out of email, and
    /// following an arbitrary one from a message that merely looked like a
    /// broker reply is how a phishing link gets clicked automatically.
    pub async fn confirm(&self, url: &str, validate_domain: bool) -> Result<Confirmation, Error> {
        let host = host_of(url)?;

        if validate_domain && !self.is_broker_domain(url)? {
            return Err(Error::UntrustedDomain { host });
        }

        let response = match self.client.get(url).send().await {
            Ok(response) => response,
            Err(error) => {
                return Ok(Confirmation {
                    url: url.to_string(),
                    final_url: url.to_string(),
                    status: None,
                    outcome: Outcome::Failed(describe(&error)),
                });
            }
        };

        let status = response.status();
        let final_url = response.url().to_string();
        let body = read_capped(response).await;

        Ok(Confirmation {
            url: url.to_string(),
            final_url,
            status: Some(status.as_u16()),
            outcome: read_outcome(status.as_u16(), &body),
        })
    }
}

/// Decide what a response says.
///
/// Failure wording is checked before success wording. Go had it the other way
/// round, and expiry pages routinely say "thank you" somewhere on them — so
/// "This link has expired. Thank you for your interest." was reported as a
/// successful confirmation.
pub fn read_outcome(status: u16, body: &str) -> Outcome {
    let lower = body.to_lowercase();

    // A challenge page is neither a success nor a failure: nothing was
    // confirmed, and a person can still finish it by hand.
    if let Some(found) = captcha::detect_in_html(&lower)
        && found.blocks_automation()
    {
        return Outcome::Blocked(found);
    }

    if ALREADY_MARKERS.iter().any(|marker| lower.contains(marker)) {
        return Outcome::AlreadyConfirmed;
    }

    if lower.contains("expired") || lower.contains("no longer valid") {
        return Outcome::Expired;
    }
    if lower.contains("invalid link") || lower.contains("link invalid") {
        return Outcome::Invalid;
    }

    if !(200..400).contains(&status) {
        return match status {
            404 => Outcome::Invalid,
            410 => Outcome::Expired,
            other => Outcome::Failed(format!("the server answered {other}")),
        };
    }

    if FAILURE_MARKERS.iter().any(|marker| lower.contains(marker)) {
        return Outcome::Failed("the page reported a problem".to_string());
    }
    if SUCCESS_MARKERS.iter().any(|marker| lower.contains(marker)) {
        return Outcome::Confirmed;
    }

    // Some brokers just return an empty 200 and consider the job done. That
    // is probably a success, but not certainly, so say so rather than
    // recording a confirmation that may never have happened.
    Outcome::Unclear
}

/// Every domain a broker in the database is known by.
pub fn domains_of(brokers: &[Broker]) -> HashSet<String> {
    let mut domains = HashSet::new();

    for broker in brokers {
        if let Some((_, domain)) = broker.email.rsplit_once('@') {
            let domain = domain.trim().to_lowercase();
            if !domain.is_empty() {
                domains.insert(domain);
            }
        }
        for url in [&broker.website, &broker.opt_out_url] {
            if let Ok(host) = host_of(url) {
                domains.insert(host);
            }
        }
    }

    domains
}

/// The host of a URL, lowercased and without `www.`.
fn host_of(url: &str) -> Result<String, Error> {
    let parsed = url::Url::parse(url.trim()).map_err(|_| Error::BadUrl {
        url: url.to_string(),
    })?;

    let host = parsed
        .host_str()
        .ok_or_else(|| Error::BadUrl {
            url: url.to_string(),
        })?
        .to_lowercase();

    Ok(host.trim_start_matches("www.").to_string())
}

/// Read at most [`MAX_BODY`] bytes of a response.
async fn read_capped(response: reqwest::Response) -> String {
    match response.bytes().await {
        Ok(bytes) => {
            let end = bytes.len().min(MAX_BODY);
            String::from_utf8_lossy(&bytes[..end]).into_owned()
        }
        Err(_) => String::new(),
    }
}

/// Describe a transport failure without repeating the URL back.
fn describe(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "the server did not answer in time".to_string()
    } else if error.is_connect() {
        "the server could not be reached".to_string()
    } else if error.is_redirect() {
        "the link redirected too many times".to_string()
    } else {
        "the request failed".to_string()
    }
}

#[cfg(test)]
mod tests;
