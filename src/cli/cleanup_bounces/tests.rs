use chrono::TimeZone;

use super::*;
use crate::inbox::Email;

fn broker(id: &str, email: &str) -> Broker {
    Broker {
        id: id.to_string(),
        name: format!("Broker {id}"),
        email: email.to_string(),
        website: String::new(),
        opt_out_url: String::new(),
        region: "us".to_string(),
        category: "marketing".to_string(),
        notes: String::new(),
        requires_id: false,
        tags: Vec::new(),
    }
}

fn database() -> BrokerDatabase {
    BrokerDatabase {
        brokers: vec![
            broker("acme", "privacy@acme.example"),
            broker("globex", "optout@globex.example"),
        ],
    }
}

fn bounce(subject: &str, body: &str) -> Email {
    Email {
        uid: 1,
        message_id: "<bounce@mail.example>".to_string(),
        from: "mailer-daemon@mail.example".to_string(),
        from_name: "Mail Delivery System".to_string(),
        from_domain: "mail.example".to_string(),
        subject: subject.to_string(),
        body: body.to_string(),
        html_body: String::new(),
        received_at: chrono::Utc.with_ymd_and_hms(2026, 5, 6, 7, 8, 9).single(),
        broker_id: String::new(),
        broker_name: String::new(),
    }
}

// -------------------------------------------------------------------
// Deciding what bounced
// -------------------------------------------------------------------

#[test]
fn a_bounce_naming_a_broker_address_is_matched_to_that_broker() {
    let emails = vec![bounce(
        "Undeliverable: Data deletion request",
        "Your message to privacy@acme.example could not be delivered.",
    )];

    let (bounced, unmatched) = sort_bounces(&emails, &database());

    assert_eq!(bounced.len(), 1);
    assert_eq!(bounced[0].address, "privacy@acme.example");
    assert_eq!(bounced[0].broker.id, "acme");
    assert!(unmatched.is_empty());
}

/// An address nobody here uses is reported, not silently ignored: it usually
/// means a typo, or a broker whose address was already changed.
#[test]
fn a_bounce_for_an_unknown_address_is_reported_separately() {
    let emails = vec![bounce(
        "Undeliverable",
        "Your message to someone@elsewhere.example could not be delivered.",
    )];

    let (bounced, unmatched) = sort_bounces(&emails, &database());

    assert!(bounced.is_empty());
    assert_eq!(
        unmatched,
        vec![Unmatched::UnknownBroker {
            address: "someone@elsewhere.example".to_string()
        }]
    );
}

#[test]
fn a_bounce_that_names_no_address_is_reported_separately() {
    let emails = vec![bounce("Delivery failed", "Something went wrong.")];

    let (bounced, unmatched) = sort_bounces(&emails, &database());

    assert!(bounced.is_empty());
    assert_eq!(
        unmatched,
        vec![Unmatched::NoAddress {
            subject: "Delivery failed".to_string()
        }]
    );
}

/// Monthly runs mean the same dead address bounces again and again. It is
/// still one broker to remove.
#[test]
fn the_same_address_bouncing_repeatedly_counts_once() {
    let emails = vec![
        bounce(
            "Undeliverable",
            "Your message to privacy@acme.example could not be delivered.",
        ),
        bounce(
            "Undeliverable",
            "Your message to privacy@acme.example could not be delivered.",
        ),
    ];

    let (bounced, _) = sort_bounces(&emails, &database());
    assert_eq!(bounced.len(), 1);
}

#[test]
fn two_different_addresses_are_both_reported() {
    let emails = vec![
        bounce(
            "Undeliverable",
            "Your message to privacy@acme.example could not be delivered.",
        ),
        bounce(
            "Undeliverable",
            "Your message to optout@globex.example could not be delivered.",
        ),
    ];

    let (bounced, _) = sort_bounces(&emails, &database());

    assert_eq!(bounced.len(), 2);
    assert_eq!(bounced[0].broker.id, "acme");
    assert_eq!(bounced[1].broker.id, "globex");
}

// -------------------------------------------------------------------
// What it says
// -------------------------------------------------------------------

#[test]
fn an_empty_report_says_nothing_looks_dead() {
    let out = format_report(&[], &[], false);
    assert!(out.contains("No broker addresses look dead"));
}

#[test]
fn a_dry_run_says_nothing_was_changed_and_how_to_change_it() {
    let emails = vec![bounce(
        "Undeliverable",
        "Your message to privacy@acme.example could not be delivered.",
    )];
    let (bounced, unmatched) = sort_bounces(&emails, &database());

    let out = format_report(&bounced, &unmatched, false);

    assert!(out.contains("privacy@acme.example"));
    assert!(out.contains("Broker acme"));
    assert!(out.contains("2026-05-06"));
    assert!(out.contains("Nothing has been changed"));
    assert!(out.contains("--remove"));
}

#[test]
fn removing_says_so_instead() {
    let emails = vec![bounce(
        "Undeliverable",
        "Your message to privacy@acme.example could not be delivered.",
    )];
    let (bounced, unmatched) = sort_bounces(&emails, &database());

    let out = format_report(&bounced, &unmatched, true);

    assert!(out.contains("Removing 1 address."));
    assert!(!out.contains("--remove"));
}

#[test]
fn the_count_is_worded_for_one_and_for_several() {
    let one = Bounced {
        address: "privacy@acme.example".into(),
        broker: broker("acme", "privacy@acme.example"),
        subject: "Undeliverable".into(),
        received_at: None,
    };
    let two = Bounced {
        address: "optout@globex.example".into(),
        broker: broker("globex", "optout@globex.example"),
        subject: "Undeliverable".into(),
        received_at: None,
    };

    assert!(format_report(std::slice::from_ref(&one), &[], false).contains("1 address look"));
    assert!(format_report(&[one, two], &[], false).contains("2 addresses look"));
}

#[test]
fn a_bounce_with_no_date_still_lists() {
    let entry = Bounced {
        address: "privacy@acme.example".into(),
        broker: broker("acme", "privacy@acme.example"),
        subject: "Undeliverable".into(),
        received_at: None,
    };

    let out = format_report(&[entry], &[], false);
    assert!(out.contains("an unknown date"));
}

// -------------------------------------------------------------------
// Shortening a subject
// -------------------------------------------------------------------

#[test]
fn a_short_subject_is_left_alone() {
    assert_eq!(truncate("Undeliverable", 60), "Undeliverable");
}

#[test]
fn a_long_subject_is_cut_with_an_ellipsis() {
    let long = "x".repeat(80);
    let short = truncate(&long, 20);

    assert_eq!(short.chars().count(), 20);
    assert!(short.ends_with('…'));
}

/// Go cut by byte index, which splits a multi-byte character in half and
/// prints a replacement glyph.
#[test]
fn cutting_does_not_split_a_character() {
    let subject = "é".repeat(80);
    let short = truncate(&subject, 20);

    assert_eq!(short.chars().count(), 20);
    assert!(short.starts_with('é'));
}
