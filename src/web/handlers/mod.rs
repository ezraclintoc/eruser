//! Request handlers.
//!
//! Ported from the handler half of `internal/web/server.go`, split by area
//! rather than living in one 2,500-line file.

use axum::extract::Request;
use axum::response::{Html, IntoResponse, Redirect, Response};
use minijinja::Value;

use super::error::WebError;
use super::security::CsrfToken;
use super::state::AppState;

pub mod api;
pub mod pages;
pub mod setup;
pub mod sign_in;

/// Render a page template with the shared values every page needs.
///
/// `context` is merged over the defaults, so a page can override `title`.
///
/// Signed-out pages — the login form and first run — go through
/// [`render_signed_out`] instead, which leaves the navigation off.
pub fn render(
    state: &AppState,
    user: &super::auth::CurrentUser,
    csrf: Option<&CsrfToken>,
    template: &str,
    context: Value,
) -> Result<Response, WebError> {
    render_inner(
        state,
        csrf,
        template,
        minijinja::context! { ..context, ..minijinja::context! {
            signed_in_as => user.username(),
            show_nav => true,
        } },
    )
}

/// Render a page for someone who is not signed in.
pub fn render_signed_out(
    state: &AppState,
    csrf: Option<&CsrfToken>,
    template: &str,
    context: Value,
) -> Result<Response, WebError> {
    render_inner(state, csrf, template, context)
}

fn render_inner(
    state: &AppState,
    csrf: Option<&CsrfToken>,
    template: &str,
    context: Value,
) -> Result<Response, WebError> {
    let template = state
        .templates
        .get_template(template)
        .map_err(|_| WebError::NotFound)?;

    let token = csrf.map(CsrfToken::as_str).unwrap_or_default();

    let base = minijinja::context! {
        csrf_token => token,
        // The whole hidden input, so a form only has to name the field once,
        // here, rather than getting it wrong in each template.
        csrf_field => Value::from_safe_string(csrf_input(token)),
        configured => state.is_configured(),
        version => env!("CARGO_PKG_VERSION"),
    };

    let html = template.render(minijinja::context! { ..context, ..base })?;
    Ok(Html(html).into_response())
}

/// The hidden field a plain form post needs to carry its CSRF token.
///
/// The token is hex, so it needs no escaping, but the field name has to
/// match `security::CSRF_FIELD`, which is easier to guarantee here than in
/// every template.
fn csrf_input(token: &str) -> String {
    if token.is_empty() {
        return String::new();
    }
    format!(
        r#"<input type="hidden" name="{}" value="{token}">"#,
        crate::web::security::CSRF_FIELD
    )
}

/// Pull the CSRF token the middleware minted for this request.
pub fn csrf_of(request: &Request) -> Option<CsrfToken> {
    request.extensions().get::<CsrfToken>().cloned()
}

/// Send an unconfigured visitor to the setup wizard.
///
/// Go checked `config == nil || Profile.FirstName == ""` inline in each
/// handler that needed it, and several handlers forgot.
pub fn require_setup(state: &AppState) -> Option<Response> {
    let has_profile = state
        .config()
        .is_some_and(|config| !config.profile.first_name.is_empty());

    (!has_profile).then(|| Redirect::to("/setup").into_response())
}

/// Whether this request came from HTMX, which wants a fragment rather than a
/// whole page.
pub fn is_htmx(request: &Request) -> bool {
    request
        .headers()
        .get("hx-request")
        .is_some_and(|value| value == "true")
}

/// Anything that did not match a route.
pub async fn not_found() -> WebError {
    WebError::NotFound
}

/// Read a urlencoded form body into a struct.
///
/// The cap matches the one the CSRF middleware uses: every form here is a
/// handful of short fields.
pub async fn read_form<T: serde::de::DeserializeOwned>(request: Request) -> Result<T, WebError> {
    let body = axum::body::to_bytes(request.into_body(), 64 * 1024)
        .await
        .map_err(|_| WebError::BadRequest("the form was too large to read".into()))?;

    serde_urlencoded::from_bytes(&body)
        .map_err(|e| WebError::BadRequest(format!("the form could not be read: {e}")))
}
