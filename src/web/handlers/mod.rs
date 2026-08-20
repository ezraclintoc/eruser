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

/// Render a page template with the shared values every page needs.
///
/// `context` is merged over the defaults, so a page can override `title`.
pub fn render(
    state: &AppState,
    csrf: Option<&CsrfToken>,
    template: &str,
    context: Value,
) -> Result<Response, WebError> {
    let template = state
        .templates
        .get_template(template)
        .map_err(|_| WebError::NotFound)?;

    let base = minijinja::context! {
        csrf_token => csrf.map(CsrfToken::as_str).unwrap_or_default(),
        configured => state.is_configured(),
    };

    let html = template.render(minijinja::context! { ..context, ..base })?;
    Ok(Html(html).into_response())
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
