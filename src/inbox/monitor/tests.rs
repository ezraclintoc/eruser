//! Monitor tests.
//!
//! Nothing here opens a connection. What is worth testing is the part that
//! decides which broker a reply came from, and the part that turns a raw
//! message into an [`Email`] — the IMAP conversation itself is the crate's
//! job, not this module's.

use super::*;
use crate::config::ValidationError;

fn broker(id: &str, email: &str, website: &str) -> Broker {
    Broker {
        id: id.to_string(),
        name: format!("Broker {id}"),
        email: email.to_string(),
        website: website.to_string(),
        opt_out_url: String::new(),
        region: "us".to_string(),
        category: String::new(),
        notes: String::new(),
        requires_id: false,
        tags: Vec::new(),
    }
}

fn inbox_config() -> InboxConfig {
    InboxConfig {
        enabled: true,
        provider: "gmail".into(),
        server: "imap.gmail.com".into(),
        port: 993,
        email: "jane@gmail.com".into(),
        password: "app-password".into(),
        ..Default::default()
    }
}

// -------------------------------------------------------------------
// Matching a reply to a broker
// -------------------------------------------------------------------

#[test]
fn a_broker_is_indexed_by_its_contact_domain() {
    let brokers = [broker("acme", "privacy@acme.example", "")];
    let monitor = Monitor::new(inbox_config(), &brokers);

    assert_eq!(
        monitor
            .broker_for_domain("acme.example")
            .map(|b| b.id.as_str()),
        Some("acme")
    );
    assert!(monitor.broker_for_domain("nobody.example").is_none());
}

/// Plenty of brokers receive at one address and reply from another, so the
/// website domain is indexed too. Matching only the contact address files
/// those replies as coming from nobody.
#[test]
fn a_broker_is_also_indexed_by_its_website_domain() {
    let brokers = [broker(
        "acme",
        "privacy@acme-data-holdings.example",
        "https://www.acme.example/",
    )];
    let monitor = Monitor::new(inbox_config(), &brokers);

    assert_eq!(
        monitor
            .broker_for_domain("acme.example")
            .map(|b| b.id.as_str()),
        Some("acme")
    );
    assert_eq!(
        monitor
            .broker_for_domain("acme-data-holdings.example")
            .map(|b| b.id.as_str()),
        Some("acme")
    );
}

#[test]
fn domain_matching_ignores_case() {
    let brokers = [broker("acme", "Privacy@ACME.Example", "")];
    let monitor = Monitor::new(inbox_config(), &brokers);

    assert!(monitor.broker_for_domain("ACME.example").is_some());
    assert!(monitor.broker_for_domain("acme.example").is_some());
}

#[test]
fn a_website_url_reduces_to_its_host() {
    assert_eq!(
        domain_of("https://www.acme.example/optout").as_deref(),
        Some("acme.example")
    );
    assert_eq!(
        domain_of("http://acme.example").as_deref(),
        Some("acme.example")
    );
    assert_eq!(
        domain_of("https://ACME.example:8443/x").as_deref(),
        Some("acme.example")
    );
    assert_eq!(
        domain_of("acme.example/path").as_deref(),
        Some("acme.example")
    );
}

#[test]
fn a_blank_website_yields_no_domain() {
    assert!(domain_of("").is_none());
    assert!(domain_of("   ").is_none());
    assert!(domain_of("https://").is_none());
}

/// Two brokers can share a parent company's domain; whichever came first in
/// the database wins, and it must not change between runs.
#[test]
fn a_shared_domain_resolves_to_the_first_broker_listed() {
    let brokers = [
        broker("first", "privacy@shared.example", ""),
        broker("second", "legal@shared.example", ""),
    ];

    let map = broker_domains(&brokers);
    assert_eq!(map["shared.example"].id, "first");

    for _ in 0..10 {
        assert_eq!(broker_domains(&brokers)["shared.example"].id, "first");
    }
}

#[test]
fn a_broker_with_no_address_or_website_is_not_indexed() {
    assert!(broker_domains(&[broker("blank", "", "")]).is_empty());
}

#[test]
fn the_whole_shipped_database_indexes_without_collapsing() {
    let db = crate::broker::BrokerDatabase::embedded().unwrap();
    let domains = broker_domains(&db.brokers);

    // Some brokers share a parent domain, so this is fewer than the broker
    // count — but it should still be most of them.
    assert!(
        domains.len() > db.brokers.len() / 2,
        "only {} domains for {} brokers",
        domains.len(),
        db.brokers.len()
    );
}

// -------------------------------------------------------------------
// Configuration
// -------------------------------------------------------------------

#[tokio::test]
async fn connecting_without_configuration_fails_before_touching_the_network() {
    let mut monitor = Monitor::new(InboxConfig::default(), &[]);

    let error = monitor.connect().await.unwrap_err();
    assert!(matches!(
        error,
        Error::NotConfigured(ValidationError::InboxDisabled)
    ));
    assert!(!monitor.is_connected());
}

#[tokio::test]
async fn connecting_without_a_password_fails_before_touching_the_network() {
    let config = InboxConfig {
        password: String::new(),
        ..inbox_config()
    };
    let mut monitor = Monitor::new(config, &[]);

    assert!(matches!(
        monitor.connect().await.unwrap_err(),
        Error::NotConfigured(ValidationError::MissingInboxPassword)
    ));
}

/// Fetching without a connection should say so rather than panic.
#[tokio::test]
async fn fetching_before_connecting_is_an_error() {
    let mut monitor = Monitor::new(inbox_config(), &[]);
    assert!(monitor.recent_emails(7).await.is_err());
}

#[tokio::test]
async fn disconnecting_when_never_connected_is_harmless() {
    let mut monitor = Monitor::new(inbox_config(), &[]);
    monitor.disconnect().await;
    assert!(!monitor.is_connected());
}

/// The config holds an app password.
#[test]
fn debug_output_does_not_leak_the_password() {
    let monitor = Monitor::new(inbox_config(), &[]);
    let debug = format!("{monitor:?}");

    assert!(!debug.contains("app-password"), "{debug}");
    assert!(debug.contains("imap.gmail.com"));
}

// -------------------------------------------------------------------
// Header decoding
// -------------------------------------------------------------------

#[test]
fn a_plain_header_comes_through_unchanged() {
    assert_eq!(decode_header(Some(b"Re: your request")), "Re: your request");
    assert_eq!(decode_header(None), "");
}

/// Brokers outside the English-speaking world send encoded subjects, and an
/// undecoded "=?UTF-8?B?..." matches no classifier pattern at all.
#[test]
fn an_encoded_header_is_decoded() {
    // "Anfrage" base64-encoded, the way a German mail client would send it.
    assert_eq!(decode_header(Some(b"=?UTF-8?B?QW5mcmFnZQ==?=")), "Anfrage");
    // Quoted-printable, for an accented subject.
    assert_eq!(
        decode_header(Some(b"=?UTF-8?Q?Anfrage_erhalten?=")),
        "Anfrage erhalten"
    );
}

#[test]
fn an_invalid_encoding_falls_back_to_the_raw_text() {
    let decoded = decode_header(Some(b"=?NOTACHARSET?B?####?="));
    assert!(!decoded.is_empty(), "something readable should come back");
}

#[test]
fn invalid_utf8_in_a_header_does_not_lose_the_message() {
    let decoded = decode_header(Some(&[0x52, 0x65, 0x3a, 0x20, 0xff, 0xfe]));
    assert!(decoded.starts_with("Re: "));
}

// -------------------------------------------------------------------
// Message bodies
// -------------------------------------------------------------------

fn parse_body(raw: &str) -> Email {
    let mut email = Email::default();
    apply_body(&mut email, raw.as_bytes());
    email
}

#[test]
fn a_plain_text_message_yields_its_body() {
    let email = parse_body(
        "From: privacy@acme.example\r\n\
         Subject: Re: request\r\n\
         Content-Type: text/plain\r\n\
         \r\n\
         We have removed your data.\r\n",
    );

    assert!(email.body.contains("We have removed your data."));
    assert!(email.html_body.is_empty());
}

#[test]
fn a_multipart_message_yields_both_parts() {
    let email = parse_body(
        "From: privacy@acme.example\r\n\
         Subject: Re: request\r\n\
         Content-Type: multipart/alternative; boundary=\"sep\"\r\n\
         \r\n\
         --sep\r\n\
         Content-Type: text/plain\r\n\
         \r\n\
         Plain version.\r\n\
         --sep\r\n\
         Content-Type: text/html\r\n\
         \r\n\
         <p>HTML version.</p>\r\n\
         --sep--\r\n",
    );

    assert!(email.body.contains("Plain version."));
    assert!(email.html_body.contains("HTML version."));
}

#[test]
fn the_date_header_becomes_the_received_time() {
    let email = parse_body(
        "From: privacy@acme.example\r\n\
         Date: Wed, 19 Aug 2026 15:04:05 +0000\r\n\
         \r\n\
         Body.\r\n",
    );

    let received = email.received_at.expect("a date should be parsed");
    assert_eq!(received.format("%Y-%m-%d").to_string(), "2026-08-19");
}

/// The headers were readable even if the body was not; classifying on the
/// subject alone beats dropping the message.
#[test]
fn an_unparseable_body_leaves_the_rest_of_the_email_intact() {
    let mut email = Email {
        subject: "Re: your request".into(),
        ..Default::default()
    };
    apply_body(&mut email, &[0xff, 0xff, 0xff]);

    assert_eq!(email.subject, "Re: your request");
}

/// A reply is only useful once it has been classified, so the two halves
/// have to fit together.
#[test]
fn a_parsed_message_can_be_classified() {
    let mut email = parse_body(
        "From: privacy@acme.example\r\n\
         Content-Type: text/plain\r\n\
         \r\n\
         Please use our opt-out form: https://acme.example/opt-out\r\n",
    );
    email.subject = "Re: Personal Data Removal Request".into();
    email.from_domain = "acme.example".into();

    let classified = crate::inbox::classify(&email);
    assert_eq!(
        classified.response_type,
        crate::inbox::ResponseType::FormRequired
    );
    assert_eq!(
        classified.form_url.as_deref(),
        Some("https://acme.example/opt-out")
    );
}
