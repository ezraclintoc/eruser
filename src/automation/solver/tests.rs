use super::*;

// -------------------------------------------------------------------
// Reading the sidecar's answer
// -------------------------------------------------------------------

/// A solved answer is the only thing that counts as one.
#[test]
fn a_solved_answer_reads_as_solved() {
    let outcome = parse_answer(200, r#"{"status":"solved"}"#);
    assert_eq!(outcome, SolveOutcome::Solved);
}

#[test]
fn a_failed_answer_carries_its_reason() {
    let outcome = parse_answer(200, r#"{"status":"failed","detail":"wrong clicks"}"#);
    assert_eq!(outcome, SolveOutcome::Failed("wrong clicks".to_string()));
}

/// A bare failure still says something a person can read.
#[test]
fn a_failed_answer_without_a_detail_gets_a_default_line() {
    let outcome = parse_answer(200, r#"{"status":"failed"}"#);
    match outcome {
        SolveOutcome::Failed(detail) => {
            assert!(detail.contains("could not clear"), "{detail}");
        }
        other => panic!("expected failed, got {other:?}"),
    }
}

#[test]
fn an_unknown_status_is_a_failure_not_a_success() {
    let outcome = parse_answer(200, r#"{"status":"maybe"}"#);
    assert!(matches!(outcome, SolveOutcome::Failed(_)));
}

/// A sidecar that answers HTML, or nothing, must never read as solved.
#[test]
fn an_unparseable_body_is_a_failure_not_a_success() {
    assert!(matches!(
        parse_answer(200, "<html>hi</html>"),
        SolveOutcome::Failed(_)
    ));
    assert!(matches!(parse_answer(200, ""), SolveOutcome::Failed(_)));
}

#[test]
fn an_error_status_is_unavailable_whatever_the_body_says() {
    assert!(matches!(
        parse_answer(500, r#"{"status":"solved"}"#),
        SolveOutcome::Unavailable(_)
    ));
    assert!(matches!(
        parse_answer(404, "not found"),
        SolveOutcome::Unavailable(_)
    ));
}

// -------------------------------------------------------------------
// The outcome wording
// -------------------------------------------------------------------

#[test]
fn every_outcome_says_something_a_person_can_read() {
    let outcomes = [
        SolveOutcome::Solved,
        SolveOutcome::Failed("the widget did not move".into()),
        SolveOutcome::Unavailable("connection refused".into()),
    ];
    for outcome in outcomes {
        assert!(!outcome.detail().is_empty(), "{outcome:?}");
    }
}

#[test]
fn an_unavailable_outcome_says_the_solver_was_unreachable() {
    let outcome = SolveOutcome::Unavailable("connection refused".to_string());
    assert!(outcome.detail().contains("could not be reached"));
    assert!(outcome.detail().contains("connection refused"));
}

// -------------------------------------------------------------------
// Trust, but verify
// -------------------------------------------------------------------

/// A solver's word is not evidence. The page is the evidence — and a page
/// that still carries a blocking challenge counts as not solved, whatever
/// the sidecar said.
#[test]
fn a_page_that_still_carries_a_challenge_counts_as_not_solved() {
    assert!(still_blocked_after(
        r#"<form><div class="h-captcha" data-sitekey="x"></div></form>"#
    ));
    assert!(still_blocked_after(
        r#"<div class="g-recaptcha" data-sitekey="abc"></div>"#
    ));
}

#[test]
fn a_page_with_no_challenge_left_counts_as_solved() {
    assert!(!still_blocked_after(
        r#"<form><input name="email" required></form><p>Thanks</p>"#
    ));
}

/// An invisible v3 script does not block a fill, so its presence after a
/// solver pass still counts as cleared.
#[test]
fn an_invisible_v3_script_does_not_count_as_still_blocked() {
    assert!(!still_blocked_after(
        r#"<script src="https://www.google.com/recaptcha/api.js?render=key"></script><form></form>"#
    ));
}

// -------------------------------------------------------------------
// Choosing a solver from the settings
// -------------------------------------------------------------------

fn config(enabled: bool, url: &str) -> crate::config::CaptchaSolverConfig {
    crate::config::CaptchaSolverConfig {
        enabled,
        url: url.to_string(),
        token: String::new(),
        timeout_sec: 30,
    }
}

#[test]
fn a_disabled_config_means_no_solver() {
    assert!(from_config(&config(false, "http://localhost:9100")).is_none());
}

/// Switched on but not pointed anywhere is a setup mistake, not a crash:
/// challenges are left for a person, which is where they would have gone
/// anyway.
#[test]
fn an_enabled_config_without_a_url_still_means_no_solver() {
    assert!(from_config(&config(true, "  ")).is_none());
}

#[test]
fn an_enabled_config_with_a_url_builds_a_sidecar() {
    let solver =
        from_config(&config(true, "http://localhost:9100/solve")).expect("a configured sidecar");
    assert_eq!(solver.name(), "sidecar");
}
