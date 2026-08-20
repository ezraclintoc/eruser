use super::*;

// -------------------------------------------------------------------
// Rate limiting
// -------------------------------------------------------------------

#[test]
fn requests_are_allowed_up_to_the_limit_then_refused() {
    let limiter = RateLimiter::new(3, Duration::from_secs(60));

    assert!(limiter.allow("GET /"));
    assert!(limiter.allow("GET /"));
    assert!(limiter.allow("GET /"));
    assert!(
        !limiter.allow("GET /"),
        "the fourth request is over the limit"
    );
}

#[test]
fn each_key_has_its_own_budget() {
    let limiter = RateLimiter::new(1, Duration::from_secs(60));

    assert!(limiter.allow("GET /"));
    assert!(!limiter.allow("GET /"));
    assert!(
        limiter.allow("GET /brokers"),
        "one page's traffic must not starve another"
    );
}

#[test]
fn a_budget_refills_once_the_window_passes() {
    let limiter = RateLimiter::new(1, Duration::from_millis(20));

    assert!(limiter.allow("GET /"));
    assert!(!limiter.allow("GET /"));

    std::thread::sleep(Duration::from_millis(30));
    assert!(limiter.allow("GET /"), "the window should have rolled over");
}

/// Go swept expired entries from a background goroutine. Doing it on the
/// recording pass means a long-lived server cannot accumulate one entry per
/// client that ever connected.
#[test]
fn stale_keys_are_swept_rather_than_accumulating() {
    let limiter = RateLimiter::new(5, Duration::from_millis(20));
    limiter.allow("GET /one");
    limiter.allow("GET /two");
    assert_eq!(limiter.tracked_keys(), 2);

    std::thread::sleep(Duration::from_millis(30));
    limiter.allow("GET /three");
    assert_eq!(limiter.tracked_keys(), 1, "the expired keys should be gone");
}

#[test]
fn a_poisoned_lock_does_not_take_the_limiter_down() {
    let limiter = RateLimiter::new(5, Duration::from_secs(60));

    let poisoner = limiter.clone();
    let _ = std::thread::spawn(move || {
        let _guard = poisoner.lock();
        panic!("poison the mutex");
    })
    .join();

    assert!(limiter.allow("GET /"));
}

// -------------------------------------------------------------------
// CSRF tokens
// -------------------------------------------------------------------

#[test]
fn tokens_are_long_and_unique() {
    let a = CsrfToken::generate();
    let b = CsrfToken::generate();

    assert_ne!(a, b);
    assert_eq!(a.as_str().len(), 64, "256 bits, hex encoded");
    assert!(a.as_str().chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn a_token_matches_only_itself() {
    let token = CsrfToken::generate();

    assert!(token.matches(token.as_str()));
    assert!(!token.matches(CsrfToken::generate().as_str()));
    assert!(!token.matches(""));
}

/// A prefix must not compare equal, and neither must a longer string that
/// starts with the token.
#[test]
fn a_partial_token_does_not_match() {
    let token = CsrfToken::from_string("abcdef".into());

    assert!(!token.matches("abcde"));
    assert!(!token.matches("abcdefg"));
    assert!(token.matches("abcdef"));
}

#[test]
fn constant_time_comparison_agrees_with_ordinary_equality() {
    assert!(constant_time_eq(b"same", b"same"));
    assert!(!constant_time_eq(b"same", b"different"));
    assert!(!constant_time_eq(b"abc", b"abd"));
    assert!(constant_time_eq(b"", b""));
}

// -------------------------------------------------------------------
// Method classification
// -------------------------------------------------------------------

#[test]
fn only_state_changing_methods_need_a_token() {
    assert!(!is_state_changing(&Method::GET));
    assert!(!is_state_changing(&Method::HEAD));
    assert!(!is_state_changing(&Method::OPTIONS));

    assert!(is_state_changing(&Method::POST));
    assert!(is_state_changing(&Method::PUT));
    assert!(is_state_changing(&Method::DELETE));
    assert!(is_state_changing(&Method::PATCH));
}

// -------------------------------------------------------------------
// Cookies
// -------------------------------------------------------------------

#[test]
fn a_named_cookie_is_found_among_others() {
    let header = "other=1; eruser_csrf=abc123; another=2";
    assert_eq!(cookie_value(header, CSRF_COOKIE).as_deref(), Some("abc123"));
    assert_eq!(cookie_value(header, "other").as_deref(), Some("1"));
    assert!(cookie_value(header, "missing").is_none());
}

#[test]
fn cookie_parsing_tolerates_spacing_and_junk() {
    assert_eq!(
        cookie_value("  eruser_csrf = abc  ", CSRF_COOKIE).as_deref(),
        Some("abc")
    );
    assert!(cookie_value("", CSRF_COOKIE).is_none());
    assert!(cookie_value("novalue", CSRF_COOKIE).is_none());
}

/// A name that merely contains the one we want must not match.
#[test]
fn a_similarly_named_cookie_is_not_confused_for_ours() {
    let header = "not_eruser_csrf=wrong; eruser_csrf=right";
    assert_eq!(cookie_value(header, CSRF_COOKIE).as_deref(), Some("right"));
}

#[test]
fn the_cookie_is_http_only_and_same_site_strict() {
    let header = cookie_header(CSRF_COOKIE, "abc");

    assert!(header.starts_with("eruser_csrf=abc"));
    assert!(header.contains("HttpOnly"));
    assert!(header.contains("SameSite=Strict"));
    // Secure would mean the cookie is never sent back over plain HTTP on
    // localhost, which is all this server speaks.
    assert!(!header.contains("Secure"));
}

// -------------------------------------------------------------------
// Origin checking
// -------------------------------------------------------------------

#[test]
fn local_origins_on_the_right_port_are_trusted() {
    for origin in [
        "http://localhost:8080",
        "http://127.0.0.1:8080",
        "http://localhost",
        "https://localhost:8080",
    ] {
        assert!(
            origin_is_trusted(origin, 8080),
            "{origin} should be trusted"
        );
    }
}

#[test]
fn other_sites_and_other_ports_are_not_trusted() {
    for origin in [
        "http://evil.example",
        "http://localhost:9999",
        "http://localhost.evil.example:8080",
        "http://notlocalhost:8080",
        "localhost:8080",
        "null",
        "",
    ] {
        assert!(
            !origin_is_trusted(origin, 8080),
            "{origin} should be rejected"
        );
    }
}

/// A path on the end of an Origin is unusual but must not turn a foreign
/// host into a trusted one.
#[test]
fn a_path_after_the_authority_is_ignored() {
    assert!(origin_is_trusted("http://localhost:8080/some/path", 8080));
    assert!(!origin_is_trusted(
        "http://evil.example/http://localhost:8080",
        8080
    ));
}

// -------------------------------------------------------------------
// Form parsing, for tokens on a plain form post
// -------------------------------------------------------------------

#[test]
fn a_form_field_is_found_among_others() {
    let body = b"first_name=Jane&csrf_token=abc123&last_name=Doe";
    assert_eq!(form_field(body, CSRF_FIELD).as_deref(), Some("abc123"));
    assert_eq!(form_field(body, "first_name").as_deref(), Some("Jane"));
    assert!(form_field(body, "missing").is_none());
}

#[test]
fn form_values_are_percent_decoded() {
    let body = b"address=123+Main+St&city=San%20Francisco&email=a%40b.example";
    assert_eq!(form_field(body, "address").as_deref(), Some("123 Main St"));
    assert_eq!(form_field(body, "city").as_deref(), Some("San Francisco"));
    assert_eq!(form_field(body, "email").as_deref(), Some("a@b.example"));
}

#[test]
fn a_malformed_escape_is_left_alone_rather_than_dropping_the_field() {
    assert_eq!(percent_decode("100%"), "100%");
    assert_eq!(percent_decode("%zz"), "%zz");
    assert_eq!(percent_decode("caf%C3%A9"), "café");
}

#[test]
fn an_empty_or_junk_body_yields_nothing() {
    assert!(form_field(b"", CSRF_FIELD).is_none());
    assert!(form_field(b"novalue", CSRF_FIELD).is_none());
    assert!(form_field(&[0xff, 0xfe], CSRF_FIELD).is_none());
}
