use super::*;
use crate::automation::CaptchaKind;

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
        solvers: Vec::new(),
    }
}

fn entry(kind: &str, url: &str) -> crate::config::SolverEntry {
    crate::config::SolverEntry {
        kind: kind.to_string(),
        url: url.to_string(),
        token: String::new(),
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
fn an_enabled_config_with_a_url_builds_a_solver() {
    let solver =
        from_config(&config(true, "http://localhost:9100/solve")).expect("a configured solver");
    assert_eq!(solver.name(), "pool");
}

/// A list is the same thing said at length: one solver, asked about
/// everything.
#[test]
fn the_shorthand_and_a_list_of_one_are_both_usable() {
    let mut listed = config(true, "");
    listed.solvers = vec![entry("", "http://localhost:9100/solve")];
    assert!(from_config(&listed).is_some());
}

/// Entries with no address are half-written lines, not requests to nowhere.
#[test]
fn a_list_with_no_usable_url_means_no_solver() {
    let mut listed = config(true, "");
    listed.solvers = vec![entry("hcaptcha", "  "), entry("recaptcha_v2", "")];
    assert!(from_config(&listed).is_none());
}

#[test]
fn a_list_is_used_when_both_forms_are_given() {
    let mut both = config(true, "http://localhost:9100/solve");
    both.solvers = vec![entry("hcaptcha", "http://localhost:9101/solve")];
    assert!(from_config(&both).is_some(), "the list wins, not neither");
}

// -------------------------------------------------------------------
// Several solvers, by kind
// -------------------------------------------------------------------

fn challenge(kind: CaptchaKind) -> Captcha {
    Captcha {
        kind,
        confidence: 0.9,
        matched: "for the test".to_string(),
    }
}

/// A solver that answers with whatever it was told, and remembers whether
/// it was asked at all.
struct Stub {
    name: &'static str,
    outcome: SolveOutcome,
    asked: std::sync::atomic::AtomicUsize,
}

impl Stub {
    fn new(name: &'static str, outcome: SolveOutcome) -> Arc<Stub> {
        Arc::new(Stub {
            name,
            outcome,
            asked: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    fn asked(&self) -> usize {
        self.asked.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl CaptchaSolver for Stub {
    async fn solve(&self, _challenge: &Captcha, _page_url: &str) -> SolveOutcome {
        self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.outcome.clone()
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

fn pool(entries: Vec<(&str, Arc<Stub>)>) -> PoolSolver {
    PoolSolver::new(
        entries
            .into_iter()
            .map(|(kind, solver)| (kind.to_string(), solver as Arc<dyn CaptchaSolver>))
            .collect(),
    )
}

#[tokio::test]
async fn a_solver_is_only_asked_about_the_kind_it_names() {
    let images = Stub::new("images", SolveOutcome::Solved);
    let tokens = Stub::new("tokens", SolveOutcome::Solved);
    let solver = pool(vec![
        ("image_captcha", images.clone()),
        ("cloudflare_turnstile", tokens.clone()),
    ]);

    let outcome = solver
        .solve(
            &challenge(CaptchaKind::Turnstile),
            "https://acme.example/opt-out",
        )
        .await;

    assert_eq!(outcome, SolveOutcome::Solved);
    assert_eq!(tokens.asked(), 1);
    assert_eq!(images.asked(), 0, "the image solver was not asked about it");
}

/// The order is the fallback: the first that reports a solve ends it.
#[tokio::test]
async fn the_first_solver_that_reports_a_solve_ends_the_attempt() {
    let first = Stub::new("first", SolveOutcome::Solved);
    let second = Stub::new("second", SolveOutcome::Solved);
    let solver = pool(vec![("", first.clone()), ("", second.clone())]);

    assert_eq!(
        solver.solve(&challenge(CaptchaKind::HCaptcha), "url").await,
        SolveOutcome::Solved
    );
    assert_eq!(first.asked(), 1);
    assert_eq!(second.asked(), 0, "nothing left to try");
}

#[tokio::test]
async fn a_solver_that_cannot_clear_it_hands_over_to_the_next() {
    let weak = Stub::new("weak", SolveOutcome::Failed("wrong clicks".into()));
    let strong = Stub::new("strong", SolveOutcome::Solved);
    let solver = pool(vec![("", weak.clone()), ("", strong.clone())]);

    assert_eq!(
        solver.solve(&challenge(CaptchaKind::HCaptcha), "url").await,
        SolveOutcome::Solved
    );
    assert_eq!(weak.asked(), 1);
    assert_eq!(strong.asked(), 1);
}

/// A sidecar that is not running should not stop the next one being tried —
/// that is the whole reason for having a list.
#[tokio::test]
async fn a_sidecar_that_is_not_running_does_not_end_the_attempt() {
    let down = Stub::new(
        "down",
        SolveOutcome::Unavailable("connection refused".into()),
    );
    let up = Stub::new("up", SolveOutcome::Solved);
    let solver = pool(vec![("", down.clone()), ("", up.clone())]);

    assert_eq!(
        solver.solve(&challenge(CaptchaKind::HCaptcha), "url").await,
        SolveOutcome::Solved
    );
    assert_eq!(down.asked(), 1);
    assert_eq!(up.asked(), 1);
}

/// What comes back when nobody could do it: the failure, not the silence.
#[tokio::test]
async fn the_failure_is_reported_when_no_solver_can_clear_it() {
    let down = Stub::new(
        "down",
        SolveOutcome::Unavailable("connection refused".into()),
    );
    let weak = Stub::new("weak", SolveOutcome::Failed("wrong clicks".into()));
    let solver = pool(vec![("", down.clone()), ("", weak.clone())]);

    let outcome = solver.solve(&challenge(CaptchaKind::HCaptcha), "url").await;
    assert_eq!(outcome, SolveOutcome::Failed("wrong clicks".into()));
}

#[tokio::test]
async fn a_challenge_no_solver_declares_is_reported_as_such() {
    let images = Stub::new("images", SolveOutcome::Solved);
    let solver = pool(vec![("image_captcha", images.clone())]);

    let outcome = solver
        .solve(
            &challenge(CaptchaKind::HCaptcha),
            "https://acme.example/opt-out",
        )
        .await;

    match outcome {
        SolveOutcome::Unavailable(detail) => {
            assert!(detail.contains("no solver is configured"), "{detail}");
        }
        other => panic!("expected unavailable, got {other:?}"),
    }
    assert_eq!(images.asked(), 0);
}

#[test]
fn a_failure_outranks_an_unreachable_sidecar() {
    let failed = SolveOutcome::Failed("wrong clicks".into());
    let down = SolveOutcome::Unavailable("connection refused".into());

    assert_eq!(more_informative(&failed, down.clone()), failed);
    assert_eq!(more_informative(&down, failed.clone()), failed);
    assert_eq!(more_informative(&down.clone(), down.clone()), down);
}
