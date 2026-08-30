//! End-to-end tests over the router.
//!
//! These drive the assembled `Router` the same way a browser does, so
//! middleware, routing, and rendering are all covered. Nothing here touches
//! a network or a real mailbox.

use std::sync::{Arc, RwLock};

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode, header};
use axum::response::Response;
use tower::ServiceExt;

use super::*;
use crate::broker::Broker;
use crate::config::{Config, EmailConfig, Profile, SmtpConfig};
use crate::history::{NewRecord, Store};
use crate::web::security::{CSRF_COOKIE, CSRF_HEADER, cookie_value};

const PORT: u16 = 8080;

/// The account every test signs in as.
const TEST_USER: &str = "tester";
const TEST_PASSWORD: &str = "test-password";

/// A session id the tests can present without having to read it back out of
/// a Set-Cookie header first.
const TEST_SESSION: &str = "0123456789abcdef0123456789abcdef";

fn broker(id: &str, category: &str, region: &str) -> Broker {
    Broker {
        id: id.to_string(),
        name: format!("Broker {id}"),
        email: format!("privacy@{id}.example"),
        website: String::new(),
        opt_out_url: String::new(),
        region: region.to_string(),
        category: category.to_string(),
        notes: String::new(),
        requires_id: false,
        tags: Vec::new(),
    }
}

fn configured() -> Config {
    Config {
        profile: Profile {
            first_name: "Jane".into(),
            last_name: "Doe".into(),
            email: "jane@example.com".into(),
            ..Default::default()
        },
        email: EmailConfig {
            provider: "smtp".into(),
            from: "jane@example.com".into(),
            smtp: SmtpConfig {
                host: "smtp.example.com".into(),
                port: 465,
                username: "jane@example.com".into(),
                password: "app-password".into(),
                use_tls: true,
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

/// A router backed by an in-memory store, with a scratch config path.
async fn app_with(config: Option<Config>) -> (Router, AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let store = Store::open_in_memory().await.expect("an in-memory store");

    let user = store
        .claim_first_user(TEST_USER, TEST_PASSWORD)
        .await
        .expect("the first account should be claimable");

    let sessions = SessionStore::new(session::DEFAULT_TTL);
    sessions.seed(TEST_SESSION, user.id);

    let state = AppState {
        config: Arc::new(RwLock::new(config)),
        config_path: dir.path().join("config.yaml"),
        brokers: Arc::new(crate::broker::BrokerDatabase {
            brokers: vec![
                broker("acme", "marketing", "us"),
                broker("globex", "people-search", "eu"),
            ],
        }),
        store,
        engine: Arc::new(crate::template::Engine::new().expect("email templates")),
        sessions,
        rate_limiter: RateLimiter::new(10_000, std::time::Duration::from_secs(60)),
        jobs: JobManager::new(),
        job_persistence: JobPersistence::new(dir.path()),
        templates: Arc::new(templates::build().expect("page templates")),
        port: PORT,
    };

    (router(state.clone()), state, dir)
}

async fn app() -> (Router, AppState, tempfile::TempDir) {
    app_with(Some(configured())).await
}

/// A wizard session that is also signed in.
///
/// The wizard keeps its answers in a session, and that same session is the
/// one carrying the sign-in, so a test driving the wizard needs both on one
/// id.
fn signed_in_session(state: &AppState) -> String {
    let id = state.sessions.create();
    state.sessions.update(&id, |session| {
        session.user_id = Some(crate::history::DEFAULT_USER_ID);
    });
    id
}

/// The cookie that says a request is signed in.
fn session_cookie() -> String {
    format!("{}={TEST_SESSION}", session::COOKIE_NAME)
}

/// The session cookie alongside another one, since a browser sends them all
/// in a single header and the server only reads the first.
fn with_session(cookie: &str) -> String {
    format!("{cookie}; {}", session_cookie())
}

async fn get(app: &Router, path: &str) -> Response {
    app.clone()
        .oneshot(
            HttpRequest::builder()
                .uri(path)
                .header(header::COOKIE, session_cookie())
                .body(Body::empty())
                .expect("a valid request"),
        )
        .await
        .expect("the router should answer")
}

async fn body_of(response: Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("a readable body");
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The CSRF token minted on a GET, which a POST has to echo back.
async fn csrf_pair(app: &Router) -> (String, String) {
    token_from(get(app, "/settings").await)
}

/// The same, from a page reachable without signing in — the login form and
/// the first-run page mint their own token, and `/settings` would only
/// redirect.
async fn csrf_pair_at(app: &Router, path: &str) -> (String, String) {
    token_from(get_signed_out(app, path).await)
}

fn token_from(response: Response) -> (String, String) {
    // A response may carry several Set-Cookie headers; find ours.
    let cookie = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with(CSRF_COOKIE))
        .expect("a GET should mint a CSRF cookie")
        .to_string();

    let token = cookie_value(&cookie, CSRF_COOKIE).expect("the cookie should carry a token");
    (cookie, token)
}

// -------------------------------------------------------------------
// Pages
// -------------------------------------------------------------------

#[tokio::test]
async fn the_dashboard_renders_for_a_configured_install() {
    let (app, _state, _dir) = app().await;
    let response = get(&app, "/").await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_of(response).await;
    assert!(body.contains("Welcome back, Jane"));
    assert!(body.contains("<!DOCTYPE html>"));
}

/// Go checked for a missing config inline in each handler, and several
/// forgot, so an unconfigured install could reach a half-working page.
#[tokio::test]
async fn an_unconfigured_install_is_sent_to_the_wizard() {
    let (app, _state, _dir) = app_with(None).await;

    for path in ["/", "/pipeline", "/tasks", "/forms"] {
        let response = get(&app, path).await;
        assert_eq!(
            response.status(),
            StatusCode::SEE_OTHER,
            "{path} should redirect"
        );
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            "/setup",
            "{path} should point at the wizard"
        );
    }
}

#[tokio::test]
async fn every_page_renders() {
    let (app, _state, _dir) = app().await;

    for path in [
        "/",
        "/brokers",
        "/history",
        "/settings",
        "/pipeline",
        "/tasks",
        "/forms",
    ] {
        let response = get(&app, path).await;
        assert_eq!(response.status(), StatusCode::OK, "{path} failed to render");
    }
}

#[tokio::test]
async fn the_broker_page_lists_the_database() {
    let (app, _state, _dir) = app().await;
    let body = body_of(get(&app, "/brokers").await).await;

    assert!(body.contains("Broker acme"));
    assert!(body.contains("Broker globex"));
}

#[tokio::test]
async fn broker_filters_apply_from_the_query_string() {
    let (app, _state, _dir) = app().await;

    let body = body_of(get(&app, "/brokers?search=acme").await).await;
    assert!(body.contains("Broker acme"));
    assert!(!body.contains("Broker globex"));

    let body = body_of(get(&app, "/brokers?region=eu").await).await;
    assert!(body.contains("Broker globex"));
    assert!(!body.contains("Broker acme"));
}

/// HTMX swaps the table in place, so a filter change should return the rows
/// alone rather than a whole page nested inside the old one.
#[tokio::test]
async fn an_htmx_request_gets_the_fragment_not_the_whole_page() {
    let (app, _state, _dir) = app().await;

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri("/brokers?search=acme")
                .header("hx-request", "true")
                .header(header::COOKIE, session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = body_of(response).await;
    assert!(body.contains("Broker acme"));
    assert!(!body.contains("<!DOCTYPE html>"), "a fragment, not a page");
}

#[tokio::test]
async fn history_shows_what_was_sent_and_can_be_filtered() {
    let (app, state, _dir) = app().await;

    state
        .store
        .add_record(&NewRecord::sent(
            "acme",
            "Broker acme",
            "privacy@acme.example",
            "gdpr",
            "<id@example.com>",
        ))
        .await
        .unwrap();
    state
        .store
        .add_record(&NewRecord::failed(
            "globex",
            "Broker globex",
            "privacy@globex.example",
            "gdpr",
            "the mail server rejected the recipient",
        ))
        .await
        .unwrap();

    let body = body_of(get(&app, "/history").await).await;
    assert!(body.contains("Broker acme"));
    assert!(body.contains("Broker globex"));

    let body = body_of(get(&app, "/history?status=failed").await).await;
    assert!(body.contains("Broker globex"));
    assert!(!body.contains("Broker acme"));
}

#[tokio::test]
async fn an_unknown_page_is_a_not_found() {
    let (app, _state, _dir) = app().await;
    assert_eq!(
        get(&app, "/nothing-here").await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_task_that_does_not_exist_is_a_not_found() {
    let (app, _state, _dir) = app().await;
    assert_eq!(
        get(&app, "/tasks/9999").await.status(),
        StatusCode::NOT_FOUND
    );
}

// -------------------------------------------------------------------
// Security headers and assets
// -------------------------------------------------------------------

#[tokio::test]
async fn security_headers_are_on_every_response() {
    let (app, _state, _dir) = app().await;
    let response = get(&app, "/").await;
    let headers = response.headers();

    assert_eq!(headers.get("x-frame-options").unwrap(), "DENY");
    assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
    assert!(headers.get("content-security-policy").is_some());
}

/// The pages show a home address and a send history.
#[tokio::test]
async fn pages_are_not_cached() {
    let (app, _state, _dir) = app().await;
    let response = get(&app, "/").await;

    let cache = response
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert!(cache.contains("no-store"), "{cache}");
}

/// The policy is what stops a page from quietly loading a CDN again.
#[tokio::test]
async fn the_content_security_policy_allows_no_third_party_hosts() {
    let (app, _state, _dir) = app().await;
    let response = get(&app, "/").await;

    let csp = response
        .headers()
        .get("content-security-policy")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();

    assert!(csp.contains("default-src 'self'"));
    assert!(!csp.contains("cdn."), "{csp}");
    assert!(!csp.contains("unpkg"), "{csp}");
    assert!(!csp.contains("googleapis"), "{csp}");
}

#[tokio::test]
async fn static_files_are_served_from_the_binary() {
    let (app, _state, _dir) = app().await;

    let response = get(&app, "/static/css/app.css").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "text/css"
    );

    assert_eq!(
        get(&app, "/static/js/htmx.min.js").await.status(),
        StatusCode::OK
    );
}

// -------------------------------------------------------------------
// CSRF
// -------------------------------------------------------------------

#[tokio::test]
async fn a_get_mints_a_csrf_cookie() {
    let (app, _state, _dir) = app().await;
    let (cookie, token) = csrf_pair(&app).await;

    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Strict"));
    assert_eq!(token.len(), 64);
}

#[tokio::test]
async fn a_post_without_a_token_is_refused() {
    let (app, _state, _dir) = app().await;

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/send-all")
                .header(header::COOKIE, session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_post_with_the_wrong_token_is_refused() {
    let (app, _state, _dir) = app().await;
    let (cookie, _token) = csrf_pair(&app).await;

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/send-all")
                .header(header::COOKIE, with_session(&cookie))
                .header(CSRF_HEADER, "0".repeat(64))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_post_with_the_matching_token_is_allowed_through() {
    let (app, _state, _dir) = app().await;
    let (cookie, token) = csrf_pair(&app).await;

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("DELETE")
                .uri("/api/history/failed")
                .header(header::COOKIE, with_session(&cookie))
                .header(CSRF_HEADER, token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

/// A browser attaches Origin to a cross-site form post, so this is caught
/// before the token is even considered.
#[tokio::test]
async fn a_post_from_another_origin_is_refused() {
    let (app, _state, _dir) = app().await;
    let (cookie, token) = csrf_pair(&app).await;

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("DELETE")
                .uri("/api/history/failed")
                .header(header::COOKIE, with_session(&cookie))
                .header(CSRF_HEADER, token)
                .header(header::ORIGIN, "http://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn static_files_do_not_need_a_token() {
    let (app, _state, _dir) = app().await;
    assert_eq!(
        get(&app, "/static/css/app.css").await.status(),
        StatusCode::OK
    );
}

// -------------------------------------------------------------------
// API
// -------------------------------------------------------------------

#[tokio::test]
async fn the_stats_endpoint_counts_the_database_and_the_history() {
    let (app, state, _dir) = app().await;
    state
        .store
        .add_record(&NewRecord::sent(
            "acme",
            "Broker acme",
            "a@b.example",
            "gdpr",
            "",
        ))
        .await
        .unwrap();

    let body = body_of(get(&app, "/api/stats").await).await;
    let stats: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(stats["total_brokers"], 2);
    assert_eq!(stats["sent"], 1);
    assert_eq!(stats["pending"], 1);
}

#[tokio::test]
async fn the_broker_endpoint_returns_json_rows() {
    let (app, _state, _dir) = app().await;
    let body = body_of(get(&app, "/api/brokers?region=us").await).await;
    let rows: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["id"], "acme");
    assert_eq!(rows[0]["status"], "never");
}

/// A hand-edited URL should not be able to ask for the whole table.
#[tokio::test]
async fn the_history_endpoint_clamps_its_limit() {
    let (app, _state, _dir) = app().await;

    for query in ["?limit=100000", "?limit=0", "?limit=-5"] {
        let response = get(&app, &format!("/api/history{query}")).await;
        assert_eq!(response.status(), StatusCode::OK, "{query}");
    }
}

#[tokio::test]
async fn asking_about_a_job_that_does_not_exist_is_a_not_found() {
    let (app, _state, _dir) = app().await;
    assert_eq!(
        get(&app, "/api/job/nope/status").await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_active_job_endpoint_reports_when_nothing_is_running() {
    let (app, _state, _dir) = app().await;
    let body = body_of(get(&app, "/api/job/active").await).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(json["active"], false);
}

/// Scanning needs somewhere to scan. The message should name the missing
/// setting rather than failing as an unexplained server error.
#[tokio::test]
async fn scanning_without_inbox_settings_explains_what_is_missing() {
    let (app, _state, _dir) = app().await;
    let (cookie, token) = csrf_pair(&app).await;

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/inbox/scan")
                .header(header::COOKIE, with_session(&cookie))
                .header(CSRF_HEADER, token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(body_of(response).await.contains("inbox"));
}

/// Re-reading stored replies fetches nothing, so it works with no mailbox
/// settings at all — which is the point, since it is what you reach for after
/// the mailbox has been cleared.
#[tokio::test]
async fn stored_replies_can_be_reclassified_without_a_mailbox() {
    let (app, state, _dir) = app().await;
    let (cookie, token) = csrf_pair(&app).await;

    state
        .store
        .upsert_broker_response(&crate::history::NewBrokerResponse {
            broker_id: "acme".into(),
            broker_name: "Broker acme".into(),
            response_type: crate::history::ResponseType::Unknown,
            email_subject: "Your Request Has Been Received".into(),
            needs_review: true,
            ..Default::default()
        })
        .await
        .unwrap();

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/inbox/reclassify")
                .header(header::COOKIE, with_session(&cookie))
                .header(CSRF_HEADER, token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body_of(response).await).unwrap();
    assert_eq!(json["reclassified"], 1);

    let stored = state
        .store
        .find_response_by_subject(
            crate::history::DEFAULT_USER_ID,
            "acme",
            "Your Request Has Been Received",
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.response_type, crate::history::ResponseType::Pending);
}

/// Two runs would both count against the same daily limit and interleave
/// their progress.
#[tokio::test]
async fn only_one_send_runs_at_a_time() {
    let (app, state, _dir) = app().await;
    let (cookie, token) = csrf_pair(&app).await;

    // Occupy the slot without sending anything.
    let _running = state.jobs.create(10, None);

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/send-all")
                .header(header::COOKIE, with_session(&cookie))
                .header(CSRF_HEADER, token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn sending_before_setup_is_refused() {
    let (app, _state, _dir) = app_with(None).await;
    let (cookie, token) = csrf_pair(&app).await;

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/send-all")
                .header(header::COOKIE, with_session(&cookie))
                .header(CSRF_HEADER, token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
}

// -------------------------------------------------------------------
// Setup wizard
// -------------------------------------------------------------------

#[tokio::test]
async fn the_wizard_steps_all_render() {
    let (app, _state, _dir) = app_with(None).await;

    for path in [
        "/setup/welcome",
        "/setup/profile",
        "/setup/email",
        "/setup/test",
    ] {
        let response = get(&app, path).await;
        assert_eq!(response.status(), StatusCode::OK, "{path} failed to render");
    }
}

#[tokio::test]
async fn the_wizard_root_redirects_into_the_first_step() {
    let (app, _state, _dir) = app_with(None).await;
    let response = get(&app, "/setup").await;

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get(header::LOCATION).unwrap(),
        "/setup/welcome"
    );
}

#[tokio::test]
async fn a_wizard_page_starts_a_session() {
    let (app, state, _dir) = app_with(None).await;
    let response = get(&app, "/setup/profile").await;

    let cookies: Vec<_> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect();

    assert!(
        cookies.iter().any(|c| c.contains(session::COOKIE_NAME)),
        "expected a session cookie, got {cookies:?}"
    );
    assert_eq!(state.sessions.count(), 1);
}

#[tokio::test]
async fn a_submitted_profile_is_kept_and_moves_to_the_next_step() {
    let (app, state, _dir) = app_with(None).await;
    let (csrf_cookie, token) = csrf_pair(&app).await;
    let session_id = signed_in_session(&state);

    let form = format!("first_name=Jane&last_name=Doe&email=jane%40example.com&csrf_token={token}");

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/setup/profile")
                .header(
                    header::COOKIE,
                    format!("{csrf_cookie}; {}={session_id}", session::COOKIE_NAME),
                )
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers().get(header::LOCATION).unwrap(),
        "/setup/email"
    );

    let session = state
        .sessions
        .get(&session_id)
        .expect("the session survives");
    assert_eq!(session.profile.first_name, "Jane");
    assert_eq!(session.profile.email, "jane@example.com");
    assert_eq!(session.step, "email");
}

/// Rejecting the form should not throw away what was already typed.
#[tokio::test]
async fn an_incomplete_profile_comes_back_with_the_answers_intact() {
    let (app, state, _dir) = app_with(None).await;
    let (csrf_cookie, token) = csrf_pair(&app).await;
    let session_id = signed_in_session(&state);

    let form = format!("first_name=Jane&last_name=&email=nonsense&csrf_token={token}");

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/setup/profile")
                .header(
                    header::COOKIE,
                    format!("{csrf_cookie}; {}={session_id}", session::COOKIE_NAME),
                )
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK, "it should re-render");
    let body = body_of(response).await;
    assert!(
        body.contains("Jane"),
        "the typed name should still be there"
    );

    // Nothing was accepted into the session.
    assert_eq!(
        state.sessions.get(&session_id).unwrap().profile.first_name,
        ""
    );
}

/// The wizard holds the SMTP password until the last step; it must never be
/// echoed back into the page.
#[tokio::test]
async fn the_email_step_does_not_send_the_password_back_to_the_browser() {
    let (app, state, _dir) = app_with(None).await;
    let session_id = signed_in_session(&state);
    state.sessions.update(&session_id, |session| {
        session.email = configured().email;
    });

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri("/setup/email")
                .header(
                    header::COOKIE,
                    format!("{}={session_id}", session::COOKIE_NAME),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = body_of(response).await;
    assert!(
        !body.contains("app-password"),
        "the password reached the page"
    );
    assert!(body.contains("smtp.example.com"), "the host should show");
}

#[tokio::test]
async fn finishing_the_wizard_writes_the_config_and_forgets_the_session() {
    let (app, state, dir) = app_with(None).await;
    let session_id = signed_in_session(&state);
    let ready = configured();
    state.sessions.update(&session_id, |session| {
        session.profile = ready.profile.clone();
        session.email = ready.email.clone();
    });

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri("/setup/complete")
                .header(
                    header::COOKIE,
                    format!("{}={session_id}", session::COOKIE_NAME),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        dir.path().join("config.yaml").exists(),
        "the config should be on disk"
    );
    assert!(state.is_configured());
    assert!(
        state.sessions.get(&session_id).is_none(),
        "the session held the password and should be gone"
    );
}

/// Finishing with a config that cannot send just moves the failure somewhere
/// less obvious.
#[tokio::test]
async fn the_wizard_refuses_to_finish_with_an_unusable_config() {
    let (app, state, dir) = app_with(None).await;
    let session_id = signed_in_session(&state);
    state.sessions.update(&session_id, |session| {
        session.profile.first_name = "Jane".into();
        // No last name, no email, no transport.
    });

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri("/setup/complete")
                .header(
                    header::COOKIE,
                    format!("{}={session_id}", session::COOKIE_NAME),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(!dir.path().join("config.yaml").exists());
}

#[tokio::test]
async fn saving_inbox_settings_requires_an_address_and_a_password() {
    let (app, state, _dir) = app().await;
    let (cookie, token) = csrf_pair(&app).await;

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/settings/inbox")
                .header(header::COOKIE, with_session(&cookie))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "inbox_email=&inbox_password=&csrf_token={token}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(body_of(response).await.contains("Enter the email address"));
    assert!(
        !state.config().unwrap().inbox.enabled,
        "nothing should have been turned on"
    );
}

#[tokio::test]
async fn saving_inbox_settings_stores_them_and_fills_in_the_server() {
    let (app, state, _dir) = app().await;
    let (cookie, token) = csrf_pair(&app).await;

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/settings/inbox")
                .header(header::COOKIE, with_session(&cookie))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "inbox_email=jane%40gmail.com&inbox_password=secret&csrf_token={token}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let config = state.config().unwrap();
    assert!(config.inbox.enabled);
    assert_eq!(config.inbox.email, "jane@gmail.com");
    assert_eq!(
        config.inbox.server, "imap.gmail.com",
        "the provider should imply the server"
    );
    assert_eq!(config.inbox.port, 993);
}

// -------------------------------------------------------------------
// Server construction
// -------------------------------------------------------------------

#[tokio::test]
async fn the_server_binds_and_shuts_down_cleanly() {
    let store = Store::open_in_memory().await.unwrap();
    let dir = tempfile::tempdir().unwrap();

    let server = Server::new(
        "127.0.0.1",
        // 0 asks the OS for a free port, so the test cannot collide with
        // anything already listening.
        0,
        Some(configured()),
        dir.path().join("config.yaml"),
        crate::broker::BrokerDatabase::default(),
        store,
        crate::template::Engine::new().unwrap(),
    )
    .expect("the server should build");

    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let serving = tokio::spawn(async move {
        server
            .serve(async {
                let _ = stopped.await;
            })
            .await
    });

    // Let it reach the accept loop, then ask it to stop.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let _ = stop.send(());

    let result = tokio::time::timeout(std::time::Duration::from_secs(5), serving)
        .await
        .expect("the server should stop promptly")
        .expect("the serving task should not panic");
    assert!(result.is_ok(), "clean shutdown, got {result:?}");
}

#[tokio::test]
async fn a_port_already_in_use_is_reported_clearly() {
    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = occupied.local_addr().unwrap().port();

    let store = Store::open_in_memory().await.unwrap();
    let dir = tempfile::tempdir().unwrap();

    let server = Server::new(
        "127.0.0.1",
        port,
        Some(configured()),
        dir.path().join("config.yaml"),
        crate::broker::BrokerDatabase::default(),
        store,
        crate::template::Engine::new().unwrap(),
    )
    .unwrap();

    let error = server
        .serve(std::future::pending())
        .await
        .expect_err("binding an occupied port should fail");

    let message = error.to_string();
    assert!(message.contains(&port.to_string()), "{message}");
    assert!(message.contains("another program"), "{message}");
}

// -------------------------------------------------------------------
// Resuming an interrupted send
// -------------------------------------------------------------------

fn interrupted_run(remaining: &[&str]) -> crate::web::job::PendingJob {
    crate::web::job::PendingJob {
        id: "job-1".into(),
        status: crate::web::job::JobStatus::Running,
        sent: 250,
        failed: 3,
        total: 400,
        started_at: chrono::Utc::now(),
        remaining_brokers: remaining.iter().map(|id| (*id).to_string()).collect(),
        search: String::new(),
        category: String::new(),
        region: String::new(),
        status_filter: String::new(),
        daily_limit: Some(250),
    }
}

#[tokio::test]
async fn nothing_is_pending_on_a_fresh_install() {
    let (app, _state, _dir) = app().await;
    let body = body_of(get(&app, "/api/job/pending").await).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(json["pending"], false);
}

/// The file was already being written on every broker; until now nothing
/// ever read it back, so an interrupted run was simply lost.
#[tokio::test]
async fn an_interrupted_run_is_reported_as_pending() {
    let (app, state, _dir) = app().await;
    state
        .job_persistence
        .save(&interrupted_run(&["acme", "globex"]))
        .unwrap();

    let body = body_of(get(&app, "/api/job/pending").await).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(json["pending"], true);
    assert_eq!(json["remaining"], 2);
    assert_eq!(json["sent"], 250);
}

#[tokio::test]
async fn a_finished_run_is_not_reported_as_pending() {
    let (app, state, _dir) = app().await;
    state.job_persistence.save(&interrupted_run(&[])).unwrap();

    let body = body_of(get(&app, "/api/job/pending").await).await;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["pending"], false);
}

async fn post(app: &Router, path: &str) -> Response {
    let (cookie, token) = csrf_pair(app).await;
    app.clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri(path)
                .header(header::COOKIE, with_session(&cookie))
                .header(CSRF_HEADER, token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn resuming_with_nothing_pending_says_so() {
    let (app, _state, _dir) = app().await;
    let response = post(&app, "/api/job/resume").await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(body_of(response).await.contains("nothing to resume"));
}

/// Resuming carries the earlier totals across, so the progress bar continues
/// rather than restarting and reading as though the work was lost.
#[tokio::test]
async fn resuming_continues_the_earlier_totals() {
    let (app, state, _dir) = app().await;
    state
        .job_persistence
        .save(&interrupted_run(&["acme", "globex"]))
        .unwrap();

    let response = post(&app, "/api/job/resume").await;
    assert_eq!(response.status(), StatusCode::OK);

    let snapshot: serde_json::Value = serde_json::from_str(&body_of(response).await).unwrap();
    assert_eq!(snapshot["sent"], 250);
    assert_eq!(snapshot["failed"], 3);
    assert_eq!(
        snapshot["total"], 255,
        "the two remaining brokers plus what was already done"
    );
}

/// A broker taken out of brokers.yaml since the run started should be
/// skipped, not stop the resume.
#[tokio::test]
async fn a_broker_that_no_longer_exists_is_skipped_when_resuming() {
    let (app, state, _dir) = app().await;
    state
        .job_persistence
        .save(&interrupted_run(&["acme", "deleted-since"]))
        .unwrap();

    let response = post(&app, "/api/job/resume").await;
    assert_eq!(response.status(), StatusCode::OK);

    let snapshot: serde_json::Value = serde_json::from_str(&body_of(response).await).unwrap();
    // 253 already handled, plus the one broker still in the database.
    assert_eq!(snapshot["total"], 254);
}

#[tokio::test]
async fn resuming_a_run_of_brokers_that_all_vanished_reports_it() {
    let (app, state, _dir) = app().await;
    state
        .job_persistence
        .save(&interrupted_run(&["gone", "also-gone"]))
        .unwrap();

    let response = post(&app, "/api/job/resume").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(body_of(response).await.contains("still in the database"));

    // The file is cleared, so it does not linger forever.
    assert!(state.job_persistence.load().is_none());
}

#[tokio::test]
async fn resuming_while_a_send_is_already_running_is_refused() {
    let (app, state, _dir) = app().await;
    state
        .job_persistence
        .save(&interrupted_run(&["acme"]))
        .unwrap();
    let _running = state.jobs.create(10, None);

    assert_eq!(
        post(&app, "/api/job/resume").await.status(),
        StatusCode::CONFLICT
    );
}

// -------------------------------------------------------------------
// Signing in
// -------------------------------------------------------------------

/// An instance with no session presented at all.
async fn get_signed_out(app: &Router, path: &str) -> Response {
    app.clone()
        .oneshot(
            HttpRequest::builder()
                .uri(path)
                .body(Body::empty())
                .expect("a valid request"),
        )
        .await
        .expect("the router should answer")
}

#[tokio::test]
async fn a_page_asked_for_without_a_session_goes_to_the_login_form() {
    let (app, _state, _dir) = app().await;

    let response = get_signed_out(&app, "/").await;

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("/login")
    );
}

/// A fetch cannot follow a redirect to an HTML page and do anything sensible
/// with it, so the API says plainly that the request was not authorised.
#[tokio::test]
async fn an_api_call_without_a_session_is_unauthorized_rather_than_redirected() {
    let (app, _state, _dir) = app().await;

    let response = get_signed_out(&app, "/api/stats").await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_login_form_is_reachable_without_signing_in() {
    let (app, _state, _dir) = app().await;

    let response = get_signed_out(&app, "/login").await;

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_of(response).await;
    assert!(body.contains("Sign in"));
    // The navigation is for someone who is already in. The stylesheet still
    // defines its look, so look for a link rather than the class name.
    assert!(!body.contains("href=\"/brokers\""));
    assert!(!body.contains("Sign out"));
}

/// A session cookie naming a session that never existed is not a way in.
#[tokio::test]
async fn an_invented_session_id_does_not_sign_anyone_in() {
    let (app, _state, _dir) = app().await;

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri("/")
                .header(
                    header::COOKIE,
                    format!("{}=deadbeefdeadbeefdeadbeefdeadbeef", session::COOKIE_NAME),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
}

/// The setup wizard opens a session before anyone signs in. That session
/// must not count as a sign-in.
#[tokio::test]
async fn a_session_with_nobody_signed_in_on_it_is_not_enough() {
    let (app, state, _dir) = app().await;
    let session_id = state.sessions.create();

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri("/")
                .header(
                    header::COOKIE,
                    format!("{}={session_id}", session::COOKIE_NAME),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn the_right_password_signs_you_in_and_sets_a_session_cookie() {
    let (app, state, _dir) = app().await;
    let (cookie, token) = csrf_pair(&app).await;
    let before = state.sessions.count();

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/login")
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "username={TEST_USER}&password={TEST_PASSWORD}&csrf_token={token}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("/")
    );

    let issued = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| value.starts_with(session::COOKIE_NAME));
    assert!(issued, "signing in should set a session cookie");
    assert_eq!(state.sessions.count(), before + 1);
}

#[tokio::test]
async fn the_wrong_password_comes_back_to_the_form_without_a_session() {
    let (app, state, _dir) = app().await;
    let (cookie, token) = csrf_pair(&app).await;
    let before = state.sessions.count();

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/login")
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "username={TEST_USER}&password=not-the-password&csrf_token={token}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state.sessions.count(), before, "no session was opened");

    let body = body_of(response).await;
    assert!(body.contains("do not match an account"));
    // The name is kept so it does not have to be typed again; the password
    // never is.
    assert!(body.contains(TEST_USER));
    assert!(!body.contains("not-the-password"));
}

/// An unknown name and a wrong password must be indistinguishable, or the
/// form becomes a way to find out who has an account here.
#[tokio::test]
async fn an_unknown_name_gives_the_same_answer_as_a_wrong_password() {
    let (app, _state, _dir) = app().await;
    let (cookie, token) = csrf_pair(&app).await;

    let mut bodies = Vec::new();
    for credentials in [
        format!("username={TEST_USER}&password=wrong-one&csrf_token={token}"),
        format!("username=nobody&password=wrong-one&csrf_token={token}"),
    ] {
        let response = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/login")
                    .header(header::COOKIE, cookie.clone())
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(credentials))
                    .unwrap(),
            )
            .await
            .unwrap();
        bodies.push(body_of(response).await);
    }

    let wrong_password = bodies[0].replace(TEST_USER, "NAME");
    let unknown_name = bodies[1].replace("nobody", "NAME");
    assert_eq!(wrong_password, unknown_name);
}

#[tokio::test]
async fn signing_out_drops_the_session() {
    let (app, state, _dir) = app().await;
    let (cookie, token) = csrf_pair(&app).await;

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/logout")
                .header(header::COOKIE, with_session(&cookie))
                .header(CSRF_HEADER, token)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(
        state.sessions.get(TEST_SESSION).is_none(),
        "the session should be gone"
    );

    // And the page it was reaching is now out of reach.
    let after = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri("/")
                .header(header::COOKIE, session_cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(after.status(), StatusCode::SEE_OTHER);
}

/// An account removed while its session was open stops working at once,
/// rather than lasting until the session expires.
#[tokio::test]
async fn a_session_belonging_to_a_deleted_account_stops_working() {
    let (app, state, _dir) = app().await;
    state
        .store
        .create_user("second", "another-password")
        .await
        .expect("a second account");
    assert!(
        state
            .store
            .delete_user(crate::history::DEFAULT_USER_ID)
            .await
            .expect("the delete should run")
    );

    let response = get(&app, "/").await;

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn the_signed_in_name_is_shown_with_a_way_out() {
    let (app, _state, _dir) = app().await;

    let body = body_of(get(&app, "/").await).await;

    assert!(body.contains(TEST_USER));
    assert!(body.contains("Sign out"));
    assert!(body.contains(r#"action="/logout""#));
}

// -------------------------------------------------------------------
// Claiming a fresh instance
// -------------------------------------------------------------------

/// An instance nobody has claimed: no password anywhere, no session.
async fn unclaimed_app() -> (Router, AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let store = Store::open_in_memory().await.expect("an in-memory store");

    let state = AppState {
        config: Arc::new(RwLock::new(Some(configured()))),
        config_path: dir.path().join("config.yaml"),
        brokers: Arc::new(crate::broker::BrokerDatabase {
            brokers: vec![broker("acme", "marketing", "us")],
        }),
        store,
        engine: Arc::new(crate::template::Engine::new().expect("email templates")),
        sessions: SessionStore::new(session::DEFAULT_TTL),
        rate_limiter: RateLimiter::new(10_000, std::time::Duration::from_secs(60)),
        jobs: JobManager::new(),
        job_persistence: JobPersistence::new(dir.path()),
        templates: Arc::new(templates::build().expect("page templates")),
        port: PORT,
    };

    (router(state.clone()), state, dir)
}

#[tokio::test]
async fn an_unclaimed_instance_sends_everything_to_the_first_run_page() {
    let (app, _state, _dir) = unclaimed_app().await;

    for path in ["/", "/settings", "/login"] {
        let response = get_signed_out(&app, path).await;
        assert_eq!(
            response.status(),
            StatusCode::SEE_OTHER,
            "{path} should redirect"
        );
        assert_eq!(
            response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok()),
            Some("/first-run"),
            "{path} should go to the first-run page"
        );
    }
}

/// The single-user version left history behind. Whoever claims the instance
/// inherits it, and the page says so rather than looking like a fresh start.
#[tokio::test]
async fn the_first_run_page_mentions_history_that_is_already_there() {
    let (app, state, _dir) = unclaimed_app().await;
    state
        .store
        .add_record(&NewRecord::sent(
            "acme",
            "Broker acme",
            "privacy@acme.example",
            "gdpr",
            "<id@example.com>",
        ))
        .await
        .expect("a stored request");

    let body = body_of(get_signed_out(&app, "/first-run").await).await;

    assert!(body.contains("1 removal request"));
    assert!(body.contains("Nothing is discarded"));
}

#[tokio::test]
async fn claiming_the_instance_takes_over_the_existing_rows() {
    let (app, state, _dir) = unclaimed_app().await;
    let (cookie, token) = csrf_pair_at(&app, "/first-run").await;

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/first-run")
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "username=owner&password=a-good-password&confirm_password=a-good-password&csrf_token={token}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    // The row that was already there belongs to the account just created,
    // rather than a second one appearing beside it.
    let users = state.store.users().await.expect("the accounts");
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].username, "owner");
    assert_eq!(users[0].id, crate::history::DEFAULT_USER_ID);

    // And they are signed in already, without having to type it again.
    let issued = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| value.starts_with(session::COOKIE_NAME));
    assert!(issued, "claiming should sign the new account in");
}

#[tokio::test]
async fn two_different_passwords_do_not_claim_anything() {
    let (app, state, _dir) = unclaimed_app().await;
    let (cookie, token) = csrf_pair_at(&app, "/first-run").await;

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/first-run")
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "username=owner&password=a-good-password&confirm_password=a-typo&csrf_token={token}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_of(response).await;
    assert!(body.contains("not the same"));
    assert!(!state.store.has_any_password().await.expect("the check"));
}

#[tokio::test]
async fn a_short_password_is_refused_with_the_name_kept() {
    let (app, state, _dir) = unclaimed_app().await;
    let (cookie, token) = csrf_pair_at(&app, "/first-run").await;

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/first-run")
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "username=owner&password=short&confirm_password=short&csrf_token={token}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();

    let body = body_of(response).await;
    assert!(body.contains("at least 8 characters"));
    assert!(body.contains("owner"));
    assert!(!state.store.has_any_password().await.expect("the check"));
}

/// Once someone has claimed it, the first-run page is not a second chance.
#[tokio::test]
async fn a_claimed_instance_sends_the_first_run_page_to_the_login_form() {
    let (app, _state, _dir) = app().await;

    let response = get_signed_out(&app, "/first-run").await;

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("/login")
    );
}

/// Every form has to carry a token, or posting it is refused. The templates
/// came from the Go version, where the field was written by gorilla; here it
/// comes from `csrf_field`, and a template that forgets it fails silently.
#[tokio::test]
async fn every_form_carries_a_csrf_field() {
    let (app, state, _dir) = app().await;
    let session_id = signed_in_session(&state);

    for path in [
        "/settings",
        "/setup/profile",
        "/setup/email",
        "/forms",
        "/tasks",
    ] {
        let response = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri(path)
                    .header(
                        header::COOKIE,
                        format!("{}={session_id}", session::COOKIE_NAME),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = body_of(response).await;
        if !body.contains("<form") {
            continue;
        }
        assert!(
            body.contains(r#"name="csrf_token""#),
            "{path} has a form with no CSRF field"
        );
    }
}

/// A form posted the plain way, with the token from the page rather than a
/// header, has to be accepted — that is what a browser does when HTMX is not
/// involved.
#[tokio::test]
async fn a_plain_form_post_with_the_rendered_token_is_accepted() {
    let (app, _state, _dir) = app().await;
    let (cookie, token) = csrf_pair(&app).await;

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri("/settings")
                .header(header::COOKIE, with_session(&cookie))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let page = body_of(response).await;
    assert!(
        page.contains(&format!(r#"name="csrf_token" value="{token}""#)),
        "the page should render the token it was given"
    );

    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/settings/inbox")
                .header(header::COOKIE, with_session(&cookie))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "address=jane@example.com&password=secret&csrf_token={token}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_ne!(
        response.status(),
        StatusCode::FORBIDDEN,
        "the token from the page should satisfy the check"
    );
}
