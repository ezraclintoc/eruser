use super::*;
use crate::automation::captcha::{Captcha, CaptchaKind};
use crate::automation::filler::{FieldKind, FillPlan, FormField, PlannedFill};
use clap::Parser;

fn plan_with_fills() -> FillPlan {
    FillPlan {
        fills: vec![PlannedFill {
            field: FormField {
                selector: "#email".into(),
                name: "email".into(),
                ..Default::default()
            },
            kind: FieldKind::Email,
            value: "jane@example.com".into(),
            score: 140,
        }],
        ..Default::default()
    }
}

fn outcome(plan: FillPlan, captcha: Option<Captcha>, submitted: bool) -> FormOutcome {
    FormOutcome {
        url: "https://acme.example/opt-out".into(),
        final_url: "https://acme.example/opt-out".into(),
        title: "Opt out".into(),
        plan,
        captcha,
        submitted,
        screenshot: None,
    }
}

// -------------------------------------------------------------------
// Arguments
// -------------------------------------------------------------------

/// A form that submits the wrong thing cannot be un-submitted.
#[test]
fn fill_does_not_submit_unless_asked() {
    let cli = crate::cli::Cli::parse_from(["eruser", "fill"]);
    let crate::cli::Command::Fill(args) = cli.command else {
        panic!("expected the fill command");
    };

    assert!(!args.submit);
    assert!(!args.show_browser, "hidden by default");
    assert!(!args.no_screenshots);
}

#[test]
fn fill_accepts_its_flags() {
    let cli = crate::cli::Cli::parse_from([
        "eruser",
        "fill",
        "--submit",
        "--show-browser",
        "--screenshots",
        "/tmp/shots",
    ]);
    let crate::cli::Command::Fill(args) = cli.command else {
        panic!("expected the fill command");
    };

    assert!(args.submit);
    assert!(args.show_browser);
    assert_eq!(
        args.screenshots.as_deref(),
        Some(std::path::Path::new("/tmp/shots"))
    );
}

/// Asking for both a directory and no screenshots is a contradiction.
#[test]
fn a_screenshot_directory_and_no_screenshots_conflict() {
    assert!(
        crate::cli::Cli::try_parse_from([
            "eruser",
            "fill",
            "--screenshots",
            "/tmp/x",
            "--no-screenshots"
        ])
        .is_err()
    );
}

#[test]
fn screenshots_default_to_the_eruser_directory_and_can_be_turned_off() {
    assert!(screenshot_dir(&Args::default()).is_some());

    let off = Args {
        no_screenshots: true,
        ..Default::default()
    };
    assert!(screenshot_dir(&off).is_none());

    let custom = Args {
        screenshots: Some(PathBuf::from("/tmp/shots")),
        ..Default::default()
    };
    assert_eq!(screenshot_dir(&custom), Some(PathBuf::from("/tmp/shots")));
}

// -------------------------------------------------------------------
// Pipeline stages
// -------------------------------------------------------------------

#[test]
fn a_submitted_form_moves_the_broker_to_filled() {
    assert_eq!(
        stage_for(&outcome(plan_with_fills(), None, true)),
        PipelineStatus::FormFilled
    );
}

/// Filled but not sent is still waiting on a person to press the button, so
/// the broker stays where it was rather than being marked done.
#[test]
fn a_filled_but_unsent_form_still_needs_the_form_doing() {
    assert_eq!(
        stage_for(&outcome(plan_with_fills(), None, false)),
        PipelineStatus::FormRequired
    );
}

#[test]
fn a_challenge_moves_the_broker_to_awaiting_captcha() {
    let blocked = outcome(
        plan_with_fills(),
        Some(Captcha {
            kind: CaptchaKind::HCaptcha,
            confidence: 0.85,
            matched: "hcaptcha".into(),
        }),
        false,
    );

    assert_eq!(stage_for(&blocked), PipelineStatus::AwaitingCaptcha);
}

/// An invisible challenge does not stop anything, so it must not park the
/// broker in a stage that waits for a person who has nothing to do.
#[test]
fn an_invisible_challenge_does_not_park_the_broker() {
    let with_v3 = outcome(
        plan_with_fills(),
        Some(Captcha {
            kind: CaptchaKind::RecaptchaV3,
            confidence: 0.85,
            matched: "recaptcha".into(),
        }),
        true,
    );

    assert_eq!(stage_for(&with_v3), PipelineStatus::FormFilled);
}

// -------------------------------------------------------------------
// Output
// -------------------------------------------------------------------

#[test]
fn a_filled_form_is_reported_with_its_screenshot() {
    let mut result = outcome(plan_with_fills(), None, false);
    result.screenshot = Some(PathBuf::from("/tmp/shots/acme.png"));

    let out = format_one("Acme Data", &result);
    assert!(out.starts_with("ok  "));
    assert!(out.contains("Acme Data"));
    assert!(out.contains("/tmp/shots/acme.png"));
}

#[test]
fn a_form_needing_a_person_is_marked() {
    let blocked = outcome(
        plan_with_fills(),
        Some(Captcha {
            kind: CaptchaKind::HCaptcha,
            confidence: 0.85,
            matched: "hcaptcha".into(),
        }),
        false,
    );

    assert!(format_one("Acme Data", &blocked).starts_with("look"));
}

#[test]
fn a_dry_run_lists_what_it_would_fill() {
    let forms = vec![(
        "acme".to_string(),
        "Acme Data".to_string(),
        "https://acme.example/opt-out".to_string(),
    )];
    let out = format_dry_run(&forms);

    assert!(out.contains("Would fill 1"));
    assert!(out.contains("https://acme.example/opt-out"));
}

/// Without --submit nothing was sent, and the summary has to say so plainly
/// rather than reading like the work is finished.
#[test]
fn a_run_without_submit_says_nothing_was_sent() {
    let out = format_summary(5, 0, false);

    assert!(out.contains("5 filled"));
    assert!(out.contains("Nothing was sent"));
    assert!(out.contains("--submit"));
}

#[test]
fn a_run_with_submit_says_so() {
    let out = format_summary(5, 0, true);

    assert!(out.contains("5 submitted"));
    assert!(!out.contains("Nothing was sent"));
}

#[test]
fn forms_needing_a_person_are_pointed_at_the_tasks_page() {
    let out = format_summary(2, 3, true);

    assert!(out.contains("3 need you"));
    assert!(out.contains("eruser serve"));
}

#[test]
fn having_nothing_to_fill_points_at_the_monitor() {
    assert!(NOTHING_TO_FILL.contains("eruser monitor"));
}
