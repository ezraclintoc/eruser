//! Pulling URLs and addresses out of a broker's reply.
//!
//! Ported from `internal/inbox/parser.go`. The scoring tables are carried
//! over unchanged — they encode a lot of hard-won knowledge about what a real
//! broker opt-out link looks like.

use std::sync::LazyLock;

use regex::Regex;

use super::Email;

/// Clearly an opt-out form.
const STRONG_FORM_PATTERNS: &[&str] = &[
    "opt-out",
    "optout",
    "opt_out",
    "do-not-sell",
    "donotsell",
    "do_not_sell",
    "removal-request",
    "removal-form",
    "removalrequest",
    "remove-my-info",
    "remove-listing",
    "remove-record",
    "data-request",
    "dsar",
    "data-subject",
    "ccpa-request",
    "gdpr-request",
    "privacy-request",
    "privacy-form",
    "/optout",
    "/opt-out",
    "/removal",
    "/remove-me",
];

/// Probably an opt-out form.
const MODERATE_FORM_PATTERNS: &[&str] = &[
    "suppress",
    "suppression",
    "ccpa",
    "gdpr",
    "/remove",
    "/delete",
];

/// Only meaningful alongside a stronger signal.
const WEAK_FORM_PATTERNS: &[&str] = &["remove", "removal", "delete", "deletion", "unsubscribe"];

/// Disqualifying: a policy page, a login screen, a PDF, a social link.
const NOT_FORM_PATTERNS: &[&str] = &[
    // Policy and legal pages, which explain rights rather than exercise them.
    "privacy-policy",
    "privacy_policy",
    "privacypolicy",
    "terms-of-service",
    "terms_of_service",
    "termsofservice",
    "terms-and-conditions",
    "terms_and_conditions",
    "cookie-policy",
    "cookie_policy",
    "cookiepolicy",
    "/tos",
    "/terms",
    "/legal",
    "/policy",
    // Help and information.
    "/about",
    "/contact",
    "/help",
    "/faq",
    "/support",
    "/how-to",
    "/howto",
    "/learn",
    "/info",
    // Sign-in walls.
    "/login",
    "/signin",
    "/register",
    "/signup",
    "/auth",
    // Account settings, which are not removal forms.
    "/account",
    "/settings",
    "/preferences",
    "/profile",
    "/unsubscribe-preferences",
    "/email-preferences",
    "/manage-preferences",
    "/communication-preferences",
    // Marketing.
    "/marketing",
    "/newsletter",
    "/subscribe",
    // Documents.
    ".pdf",
    ".doc",
    ".docx",
    // Social.
    "facebook.com",
    "twitter.com",
    "linkedin.com",
    "instagram.com",
    // Shorteners and unrelated destinations.
    "google.com",
    "bit.ly",
    "tinyurl.com",
];

/// Looks like a link that confirms an identity or an address.
const CONFIRM_PATTERNS: &[&str] = &[
    "confirm",
    "verification",
    "verify",
    "activate",
    "validate",
    "click-here",
    "clickhere",
    "token=",
    "code=",
    "approve",
    "accept",
];

/// Open-tracking pixels and beacons, which are not links to anywhere.
const TRACKING_PATTERNS: &[&str] = &[
    "track",
    "pixel",
    "beacon",
    "open.gif",
    "spacer.gif",
    "1x1",
    "unsubscribe-tracking",
];

/// Score at or below which a URL is disqualified outright.
const DISQUALIFIED: i32 = -20;
/// Bonus for a URL on the broker's own domain.
const OWN_DOMAIN_BONUS: i32 = 20;

static URL_IN_TEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"https?://[^\s<>"']+"#).expect("the URL pattern is valid"));

/// `href="..."` in HTML. Go used goquery; a full HTML parse is more than this
/// needs, and goquery's own fallback was this same regex.
static HREF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)<a[^>]+href\s*=\s*["']([^"']+)["']"#).expect("the href pattern is valid")
});

static HTML_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<[^>]+>").expect("the tag pattern is valid"));

static EMAIL_ADDRESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}")
        .expect("the address pattern is valid")
});

/// URLs found in an email, sorted into what they appear to be for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtractedUrls {
    pub form_urls: Vec<String>,
    pub confirmation_urls: Vec<String>,
    pub unsubscribe_urls: Vec<String>,
    pub all_urls: Vec<String>,
}

impl ExtractedUrls {
    pub fn is_empty(&self) -> bool {
        self.all_urls.is_empty()
    }
}

/// Extract and categorise every URL in an email.
pub fn parse_email_urls(email: &Email) -> ExtractedUrls {
    let mut candidates = Vec::new();

    if !email.body.is_empty() {
        candidates.extend(urls_in_text(&email.body));
    }
    if !email.html_body.is_empty() {
        candidates.extend(urls_in_html(&email.html_body));
    }

    let mut result = ExtractedUrls::default();
    let mut seen = std::collections::HashSet::new();

    for raw in candidates {
        let Some(url) = clean_url(&raw) else {
            continue;
        };
        if !seen.insert(url.clone()) {
            continue;
        }
        if is_tracking_url(&url) {
            continue;
        }

        let lower = url.to_lowercase();
        result.all_urls.push(url.clone());

        if is_form_url(&lower) {
            result.form_urls.push(url.clone());
        }
        if is_confirmation_url(&lower) {
            result.confirmation_urls.push(url.clone());
        }
        if lower.contains("unsubscribe") {
            result.unsubscribe_urls.push(url);
        }
    }

    result
}

fn urls_in_text(text: &str) -> Vec<String> {
    URL_IN_TEXT
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect()
}

/// Every `href`, plus any bare URL in the visible text.
fn urls_in_html(html: &str) -> Vec<String> {
    let mut urls: Vec<String> = HREF
        .captures_iter(html)
        .filter_map(|caps| caps.get(1).map(|m| m.as_str().to_string()))
        .collect();

    urls.extend(urls_in_text(&strip_tags(html)));
    urls
}

/// Normalise a URL, rejecting anything that is not http(s) with a host.
fn clean_url(raw: &str) -> Option<String> {
    // Trailing punctuation gets swept up by the text pattern when a URL ends
    // a sentence.
    let trimmed = raw.trim_end_matches(['.', ',', ';', ':', '!', '?', ')']);
    let parsed = url::Url::parse(trimmed).ok()?;

    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return None;
    }

    Some(parsed.to_string())
}

/// How much a URL looks like an opt-out form.
///
/// A disqualifying pattern short-circuits to [`DISQUALIFIED`]: a link to a
/// privacy policy is not made into a form by also containing "remove".
pub fn score_form_url(lower_url: &str) -> i32 {
    if NOT_FORM_PATTERNS
        .iter()
        .any(|pattern| lower_url.contains(pattern))
    {
        return DISQUALIFIED;
    }

    let mut score = 0;
    for pattern in STRONG_FORM_PATTERNS {
        if lower_url.contains(pattern) {
            score += 10;
        }
    }
    for pattern in MODERATE_FORM_PATTERNS {
        if lower_url.contains(pattern) {
            score += 5;
        }
    }

    // Weak signals only count when something stronger already matched, so a
    // bare "/delete-account" does not read as an opt-out form.
    if score > 0 {
        for pattern in WEAK_FORM_PATTERNS {
            if lower_url.contains(pattern) {
                score += 2;
            }
        }
    }

    score
}

pub fn is_form_url(lower_url: &str) -> bool {
    score_form_url(lower_url) > 0
}

pub fn is_confirmation_url(lower_url: &str) -> bool {
    CONFIRM_PATTERNS
        .iter()
        .any(|pattern| lower_url.contains(pattern))
}

/// Whether a URL is an open-tracking pixel rather than a link.
pub fn is_tracking_url(url: &str) -> bool {
    let lower = url.to_lowercase();

    if TRACKING_PATTERNS
        .iter()
        .any(|pattern| lower.contains(pattern))
    {
        return true;
    }

    // Go's version read `a.ends_with(".gif") || a.ends_with(".png") &&
    // contains("pixel")`, where && binds tighter than ||, so the "pixel"
    // check silently applied to .png alone. Both extensions are treated the
    // same way here, which is what the comment there said it meant.
    lower.ends_with(".gif") || (lower.ends_with(".png") && lower.contains("pixel"))
}

/// The single most likely opt-out form among the candidates.
pub fn primary_form_url(urls: &ExtractedUrls, broker_domain: &str) -> Option<String> {
    urls.form_urls
        .iter()
        .filter_map(|url| {
            let lower = url.to_lowercase();
            let mut score = score_form_url(&lower);
            if score < 0 {
                return None;
            }
            // A link back to the broker's own site beats a generic one.
            if !broker_domain.is_empty() && lower.contains(&broker_domain.to_lowercase()) {
                score += OWN_DOMAIN_BONUS;
            }
            Some((score, url))
        })
        // max_by_key returns the last maximum; reversing keeps the first URL
        // in the email, which is the one the broker led with.
        .rev()
        .max_by_key(|(score, _)| *score)
        .map(|(_, url)| url.clone())
}

/// The confirmation link to click, preferring the broker's own domain.
pub fn primary_confirmation_url(urls: &ExtractedUrls, broker_domain: &str) -> Option<String> {
    if !broker_domain.is_empty() {
        let domain = broker_domain.to_lowercase();
        if let Some(url) = urls
            .confirmation_urls
            .iter()
            .find(|url| url.to_lowercase().contains(&domain))
        {
            return Some(url.clone());
        }
    }

    urls.confirmation_urls.first().cloned()
}

/// Patterns that name the address a bounce is about.
static BOUNCED_RECIPIENT_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    const ADDRESS: &str = r"([a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,})";
    [
        format!(r"(?i)(?:the\s+following|these)\s+address(?:es)?\s+(?:had\s+permanent\s+)?(?:fatal\s+)?(?:errors?|failed)[:\s]+{ADDRESS}"),
        format!(r"(?i)delivery\s+to\s+(?:the\s+following\s+)?(?:recipient|address)(?:s)?\s+failed[:\s]+{ADDRESS}"),
        format!(r"(?i)(?:original|final)[\s-]?recipient[:\s]+(?:rfc822;)?{ADDRESS}"),
        format!(r"(?i)(?:failed|rejected)\s+recipient[:\s]+{ADDRESS}"),
        format!(r"(?i)undeliverable\s+to[:\s]+{ADDRESS}"),
        format!(r"(?i)message\s+could\s+not\s+be\s+delivered\s+to[:\s]+{ADDRESS}"),
        format!(r"(?i)<{ADDRESS}>.*(?:failed|rejected|undeliverable)"),
        format!(r"(?i)to[:\s]+<?{ADDRESS}>?\s+.*(?:failed|rejected|not\s+exist)"),
    ]
    .iter()
    .map(|pattern| Regex::new(pattern).expect("bounce patterns are valid"))
    .collect()
});

/// Addresses that belong to mail infrastructure or to the sender, not to the
/// broker whose address bounced.
const NOT_A_BOUNCED_RECIPIENT: &[&str] = &[
    "mailer-daemon",
    "postmaster",
    "noreply",
    "no-reply",
    "@gmail.com",
    "@yahoo.com",
    "@outlook.com",
    "@hotmail.com",
];

/// Which address a bounce notice is about.
///
/// This is what makes `cleanup-bounces` possible: an address that bounces is
/// one no request will ever reach, and it should come out of the database.
pub fn extract_bounced_recipient(email: &Email) -> Option<String> {
    let mut content = email.body.clone();
    if !email.html_body.is_empty() {
        content.push(' ');
        content.push_str(&strip_tags(&email.html_body));
    }
    content.push(' ');
    content.push_str(&email.subject);

    for pattern in BOUNCED_RECIPIENT_PATTERNS.iter() {
        if let Some(caps) = pattern.captures(&content)
            && let Some(address) = caps.get(1)
        {
            return Some(address.as_str().trim().to_string());
        }
    }

    // Nothing named it outright: take the first address that is not obviously
    // infrastructure or the sender's own.
    EMAIL_ADDRESS
        .find_iter(&content)
        .map(|m| m.as_str())
        .find(|address| {
            let lower = address.to_lowercase();
            !NOT_A_BOUNCED_RECIPIENT
                .iter()
                .any(|exclude| lower.contains(exclude))
        })
        .map(str::to_string)
}

/// The token in a confirmation link, if it carries one.
pub fn extract_confirmation_token(confirm_url: &str) -> Option<String> {
    let parsed = url::Url::parse(confirm_url).ok()?;

    const TOKEN_PARAMS: &[&str] = &["token", "code", "verify", "confirmation", "key", "id"];
    for name in TOKEN_PARAMS {
        if let Some((_, value)) = parsed.query_pairs().find(|(key, _)| key == name)
            && !value.is_empty()
        {
            return Some(value.into_owned());
        }
    }

    // Some brokers put it in the path: /confirm/<token>
    let segments: Vec<&str> = parsed.path().split('/').collect();
    segments.windows(2).find_map(|pair| {
        let [marker, candidate] = pair else {
            return None;
        };
        // A short segment after /confirm is another path part, not a token.
        (matches!(*marker, "confirm" | "verify") && candidate.len() > 10)
            .then(|| (*candidate).to_string())
    })
}

/// Remove HTML tags, leaving the text between them.
pub fn strip_tags(html: &str) -> String {
    HTML_TAG.replace_all(html, " ").into_owned()
}

#[cfg(test)]
mod tests;
