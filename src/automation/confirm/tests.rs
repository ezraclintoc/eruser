use super::*;

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

fn confirmer() -> Confirmer {
    Confirmer::new(&[
        broker("acme", "privacy@acme.example", "https://www.acme.example"),
        broker("globex", "privacy@globex.example", ""),
    ])
    .expect("the client should build")
}

// -------------------------------------------------------------------
// Domain checking
// -------------------------------------------------------------------

#[test]
fn a_link_to_a_known_broker_is_trusted() {
    let confirmer = confirmer();

    assert!(
        confirmer
            .is_broker_domain("https://acme.example/confirm?t=1")
            .unwrap()
    );
    assert!(
        confirmer
            .is_broker_domain("https://globex.example/verify")
            .unwrap()
    );
}

/// Brokers routinely send confirmation links from a mail subdomain.
#[test]
fn a_subdomain_of_a_known_broker_is_trusted() {
    let confirmer = confirmer();

    assert!(
        confirmer
            .is_broker_domain("https://links.acme.example/c/1")
            .unwrap()
    );
    assert!(
        confirmer
            .is_broker_domain("https://email.globex.example/v")
            .unwrap()
    );
}

#[test]
fn www_is_ignored_when_matching() {
    assert!(
        confirmer()
            .is_broker_domain("https://www.acme.example/confirm")
            .unwrap()
    );
}

/// A domain that merely ends with the same letters is a different company.
#[test]
fn a_lookalike_domain_is_not_trusted() {
    let confirmer = confirmer();

    assert!(
        !confirmer
            .is_broker_domain("https://notacme.example/confirm")
            .unwrap()
    );
    assert!(
        !confirmer
            .is_broker_domain("https://acme.example.evil.test/x")
            .unwrap()
    );
    assert!(
        !confirmer
            .is_broker_domain("https://evil.test/acme.example")
            .unwrap()
    );
}

#[test]
fn a_link_that_is_not_a_url_is_rejected() {
    assert!(matches!(
        confirmer().is_broker_domain("not a url"),
        Err(Error::BadUrl { .. })
    ));
}

/// These URLs come out of email. Following an arbitrary one from a message
/// that merely looked like a broker reply is how a phishing link gets
/// clicked automatically.
#[tokio::test]
async fn an_untrusted_link_is_refused_before_it_is_fetched() {
    let error = confirmer()
        .confirm("https://evil.test/steal", true)
        .await
        .unwrap_err();

    let message = error.to_string();
    assert!(message.contains("evil.test"), "{message}");
    assert!(message.contains("--no-validate-domain"), "{message}");
}

#[test]
fn broker_domains_come_from_the_address_the_site_and_the_opt_out_link() {
    let mut with_optout = broker("acme", "privacy@acme.example", "https://acme.example");
    with_optout.opt_out_url = "https://optout.acmedata.example/remove".into();

    let domains = domains_of(&[with_optout]);
    assert!(domains.contains("acme.example"));
    assert!(domains.contains("optout.acmedata.example"));
}

#[test]
fn the_shipped_database_yields_a_large_trusted_set() {
    let db = crate::broker::BrokerDatabase::embedded().unwrap();
    let domains = domains_of(&db.brokers);

    assert!(
        domains.len() > 400,
        "only {} domains from {} brokers",
        domains.len(),
        db.brokers.len()
    );
}

// -------------------------------------------------------------------
// Reading the response
// -------------------------------------------------------------------

#[test]
fn a_page_saying_it_worked_is_a_confirmation() {
    for body in [
        "<p>Your opt-out request has been confirmed.</p>",
        "<p>You have been successfully unsubscribed.</p>",
        "<p>Verification complete.</p>",
        "<p>Your information has been removed.</p>",
    ] {
        assert_eq!(read_outcome(200, body), Outcome::Confirmed, "for {body}");
    }
}

/// Go checked success wording before failure wording, and expiry pages
/// routinely thank you somewhere on them — so "This link has expired. Thank
/// you for your interest." was recorded as a successful confirmation.
#[test]
fn an_expiry_page_that_also_thanks_you_is_not_a_confirmation() {
    let body = "<p>This link has expired. Thank you for your interest.</p>";
    assert_eq!(read_outcome(200, body), Outcome::Expired);
}

#[test]
fn an_expired_link_is_reported_as_expired() {
    for body in [
        "<p>This confirmation link has expired.</p>",
        "<p>Sorry, that link is no longer valid.</p>",
    ] {
        assert_eq!(read_outcome(200, body), Outcome::Expired, "for {body}");
    }
    assert_eq!(read_outcome(410, ""), Outcome::Expired);
}

#[test]
fn an_invalid_link_is_reported_as_invalid() {
    assert_eq!(read_outcome(200, "<p>Invalid link.</p>"), Outcome::Invalid);
    assert_eq!(read_outcome(404, ""), Outcome::Invalid);
}

/// Re-running confirm over a list should not turn finished work into
/// reported errors, which is what Go's version did by filing this as a
/// failure.
#[test]
fn an_already_confirmed_link_counts_as_success() {
    let outcome = read_outcome(200, "<p>This request has already been confirmed.</p>");

    assert_eq!(outcome, Outcome::AlreadyConfirmed);
    assert!(outcome.is_success());
}

#[test]
fn a_page_reporting_a_problem_is_a_failure() {
    let outcome = read_outcome(200, "<p>An error occurred. We could not process that.</p>");
    assert!(matches!(outcome, Outcome::Failed(_)));
    assert!(!outcome.is_success());
}

/// A bare 200 with nothing on it is probably fine, but not certainly, and
/// recording a confirmation that may never have happened is worse than
/// asking someone to check.
#[test]
fn a_silent_page_is_reported_as_unclear() {
    let outcome = read_outcome(200, "<html><body></body></html>");

    assert_eq!(outcome, Outcome::Unclear);
    assert!(!outcome.is_success());
    assert!(outcome.needs_a_person());
}

#[test]
fn a_server_error_is_a_failure() {
    let outcome = read_outcome(503, "");
    assert!(matches!(outcome, Outcome::Failed(_)));
}

/// Nothing was confirmed, but a person can still finish it by hand — which
/// is neither a success nor a dead end.
#[test]
fn a_challenge_page_is_reported_as_blocked() {
    let outcome = read_outcome(200, r#"<div class="g-recaptcha" data-sitekey="x"></div>"#);

    match &outcome {
        Outcome::Blocked(captcha) => {
            assert_eq!(captcha.kind, crate::automation::CaptchaKind::RecaptchaV2)
        }
        other => panic!("expected a challenge, got {other:?}"),
    }
    assert!(!outcome.is_success());
    assert!(outcome.needs_a_person());
}

/// v3 does not ask the visitor anything, so a page carrying one can still be
/// read for what it says.
#[test]
fn an_invisible_challenge_does_not_block_the_reading() {
    let body = r#"<script src="https://www.google.com/recaptcha/api.js?render=KEY"></script>
                  <p>Your request has been confirmed.</p>"#;
    assert_eq!(read_outcome(200, body), Outcome::Confirmed);
}

// -------------------------------------------------------------------
// Reporting
// -------------------------------------------------------------------

#[test]
fn every_outcome_explains_itself() {
    let outcomes = [
        Outcome::Confirmed,
        Outcome::AlreadyConfirmed,
        Outcome::Expired,
        Outcome::Invalid,
        Outcome::Unclear,
        Outcome::Failed("the server could not be reached".into()),
        Outcome::Blocked(crate::automation::Captcha {
            kind: crate::automation::CaptchaKind::HCaptcha,
            confidence: 0.85,
            matched: "hcaptcha".into(),
        }),
    ];

    for outcome in outcomes {
        assert!(!outcome.summary().is_empty(), "{outcome:?}");
    }
}

#[test]
fn a_blocked_outcome_says_what_to_do_about_it() {
    let outcome = Outcome::Blocked(crate::automation::Captcha {
        kind: crate::automation::CaptchaKind::HCaptcha,
        confidence: 0.85,
        matched: "hcaptcha".into(),
    });

    assert!(outcome.summary().contains("images"));
}

#[test]
fn only_confirmations_count_as_success() {
    assert!(Outcome::Confirmed.is_success());
    assert!(Outcome::AlreadyConfirmed.is_success());

    assert!(!Outcome::Expired.is_success());
    assert!(!Outcome::Invalid.is_success());
    assert!(!Outcome::Unclear.is_success());
    assert!(!Outcome::Failed("x".into()).is_success());
}

/// Debug output ends up in logs.
#[test]
fn debug_output_prints_a_count_rather_than_every_domain() {
    let debug = format!("{:?}", confirmer());
    assert!(debug.contains("broker_domains"));
    assert!(!debug.contains("acme.example"), "{debug}");
}
