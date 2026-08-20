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
        },
        ..Default::default()
    }
}

/// A router backed by an in-memory store, with a scratch config path.
async fn app_with(config: Option<Config>) -> (Router, AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let store = Store::open_in_memory().await.expect("an in-memory store");

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
        sessions: SessionStore::new(session::DEFAULT_TTL),
        rate_limiter: RateLimiter::new(10_000, std::time::Duration::from_secs(60)),
        jobs: JobManager::new(),
        job_persistence: JobPersistence::new(dir.path()),
        templates: Arc::new(templates::build().expect("page templates")),
        port: PORT,
        user_id: DEFAULT_USER_ID,
    };

    (router(state.clone()), state, dir)
}

async fn app() -> (Router, AppState, tempfile::TempDir) {
    app_with(Some(configured())).await
}

async fn get(app: &Router, path: &str) -> Response {
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

async fn body_of(response: Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("a readable body");
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The CSRF token minted on a GET, which a POST has to echo back.
async fn csrf_pair(app: &Router) -> (String, String) {
    let response = get(app, "/settings").await;
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
                .header(header::COOKIE, cookie)
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
                .header(header::COOKIE, cookie)
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
                .header(header::COOKIE, cookie)
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
                .header(header::COOKIE, cookie)
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
                .header(header::COOKIE, cookie)
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
                .header(header::COOKIE, cookie)
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
                .header(header::COOKIE, cookie)
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
    let session_id = state.sessions.create();

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
    let session_id = state.sessions.create();

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
    let session_id = state.sessions.create();
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
    let session_id = state.sessions.create();
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
    let session_id = state.sessions.create();
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
                .header(header::COOKIE, cookie)
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
                .header(header::COOKIE, cookie)
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
