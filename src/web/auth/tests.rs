//! Which paths are reachable without signing in.

use super::*;

#[test]
fn the_sign_in_pages_are_public() {
    assert!(is_public("/login"));
    assert!(is_public("/first-run"));
    assert!(is_public("/logout"));
}

#[test]
fn the_health_check_is_public_so_a_supervisor_can_use_it() {
    assert!(is_public("/health"));
}

#[test]
fn stylesheets_and_scripts_are_public() {
    assert!(is_public("/static/css/app.css"));
    assert!(is_public("/static/js/htmx.min.js"));
}

/// The list is exact, so a path that merely starts the same way is not
/// public. Otherwise `/loginsomething` would let anyone in.
#[test]
fn a_path_that_only_looks_like_a_public_one_is_not() {
    assert!(!is_public("/logins"));
    assert!(!is_public("/login/extra"));
    assert!(!is_public("/first-run-now"));
}

/// Everything the tool actually does is behind the sign-in. This is the test
/// that fails if a new page is added to the public list by accident.
#[test]
fn every_working_page_needs_a_sign_in() {
    for path in [
        "/",
        "/brokers",
        "/history",
        "/pipeline",
        "/tasks",
        "/forms",
        "/settings",
        "/setup",
        "/setup/email",
        "/api/stats",
        "/api/send-all",
        "/api/history",
    ] {
        assert!(!is_public(path), "{path} should need a sign-in");
    }
}

/// A near-miss on the static prefix is not a way past the check.
#[test]
fn the_static_prefix_has_to_be_a_directory() {
    assert!(!is_public("/static"));
    assert!(!is_public("/staticky/app.css"));
}
