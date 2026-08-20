use super::*;
use crate::automation::captcha::{Captcha, CaptchaKind};
use clap::Parser;

fn confirmation(outcome: Outcome) -> Confirmation {
    Confirmation {
        url: "https://acme.example/confirm?t=1".into(),
        final_url: "https://acme.example/done".into(),
        status: Some(200),
        outcome,
    }
}

// -------------------------------------------------------------------
// Arguments
// -------------------------------------------------------------------

#[test]
fn confirm_validates_domains_by_default() {
    let cli = crate::cli::Cli::parse_from(["eruser", "confirm"]);
    let crate::cli::Command::Confirm(args) = cli.command else {
        panic!("expected the confirm command");
    };

    assert!(
        !args.no_validate_domain,
        "these links come out of email; following any of them unchecked is how \
         a phishing link gets clicked automatically"
    );
    assert!(!args.dry_run);
    assert!(args.url.is_none());
}

#[test]
fn confirm_accepts_a_single_link_and_a_broker_filter() {
    let cli = crate::cli::Cli::parse_from([
        "eruser",
        "confirm",
        "--url",
        "https://acme.example/c",
        "--dry-run",
    ]);
    let crate::cli::Command::Confirm(args) = cli.command else {
        panic!("expected the confirm command");
    };
    assert_eq!(args.url.as_deref(), Some("https://acme.example/c"));
    assert!(args.dry_run);

    let cli = crate::cli::Cli::parse_from(["eruser", "confirm", "--broker", "acme"]);
    let crate::cli::Command::Confirm(args) = cli.command else {
        panic!("expected the confirm command");
    };
    assert_eq!(args.broker.as_deref(), Some("acme"));
}

// -------------------------------------------------------------------
// Pipeline stages
// -------------------------------------------------------------------

#[test]
fn a_confirmation_moves_the_broker_to_confirmed() {
    assert_eq!(
        stage_for(&Outcome::Confirmed),
        Some(PipelineStatus::Confirmed)
    );
    assert_eq!(
        stage_for(&Outcome::AlreadyConfirmed),
        Some(PipelineStatus::Confirmed)
    );
}

#[test]
fn a_dead_link_moves_the_broker_to_failed() {
    assert_eq!(stage_for(&Outcome::Expired), Some(PipelineStatus::Failed));
    assert_eq!(stage_for(&Outcome::Invalid), Some(PipelineStatus::Failed));
}

/// A link that could not be reached can be tried again later. Marking the
/// broker failed would hide it from the next run.
#[test]
fn a_temporary_failure_leaves_the_stage_alone() {
    assert!(stage_for(&Outcome::Failed("timed out".into())).is_none());
    assert!(stage_for(&Outcome::Unclear).is_none());
    assert!(
        stage_for(&Outcome::Blocked(Captcha {
            kind: CaptchaKind::HCaptcha,
            confidence: 0.85,
            matched: "hcaptcha".into(),
        }))
        .is_none()
    );
}

// -------------------------------------------------------------------
// Output
// -------------------------------------------------------------------

#[test]
fn a_confirmed_link_is_reported_as_done() {
    let out = format_one("Acme Data", &confirmation(Outcome::Confirmed));

    assert!(out.starts_with("ok  "));
    assert!(out.contains("Acme Data"));
}

/// Where a person has to go and finish it, the link is worth repeating so it
/// can be pasted straight into a browser.
#[test]
fn a_link_needing_a_person_is_printed_alongside_the_reason() {
    let out = format_one(
        "Acme Data",
        &confirmation(Outcome::Blocked(Captcha {
            kind: CaptchaKind::HCaptcha,
            confidence: 0.85,
            matched: "hcaptcha".into(),
        })),
    );

    assert!(out.starts_with("look"));
    assert!(out.contains("https://acme.example/done"));
}

#[test]
fn a_dead_link_stands_out() {
    let out = format_one("Acme Data", &confirmation(Outcome::Expired));

    assert!(out.starts_with("FAIL"));
    assert!(out.contains("expired"));
}

/// A link given on the command line has no broker name to print.
#[test]
fn a_single_link_is_named_by_its_url() {
    let out = format_one("", &confirmation(Outcome::Confirmed));
    assert!(out.contains("https://acme.example/confirm?t=1"));
}

#[test]
fn a_dry_run_lists_what_it_would_follow() {
    let pending = vec![(
        "acme".to_string(),
        "Acme Data".to_string(),
        "https://acme.example/c".to_string(),
    )];
    let out = format_dry_run(&pending);

    assert!(out.contains("Would follow 1"));
    assert!(out.contains("Acme Data"));
    assert!(out.contains("https://acme.example/c"));
}

#[test]
fn the_summary_counts_only_what_happened() {
    let clean = format_summary(5, 0, 0);
    assert!(clean.contains("5 confirmed."));
    assert!(!clean.contains("need a look"));
    assert!(!clean.contains("failed"));

    let mixed = format_summary(3, 2, 1);
    assert!(mixed.contains("3 confirmed, 2 need a look, 1 failed."));
    assert!(mixed.contains("challenge"));
}

/// Links are found by reading replies, so an empty list should point at the
/// step that finds them.
#[test]
fn having_nothing_to_confirm_points_at_the_monitor() {
    assert!(NOTHING_TO_CONFIRM.contains("eruser monitor"));
}
