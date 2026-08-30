//! Knowing who is asking.
//!
//! New in the Rust version. Until now the interface bound to localhost with
//! no password and every row belonged to user 1; with several people on one
//! instance, every request has to say who it is for.

use axum::extract::{FromRequestParts, OptionalFromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};

use super::error::WebError;
use super::security::cookie_value;
use super::session::COOKIE_NAME;
use super::state::AppState;
use crate::history::User;

/// Paths reachable without signing in.
///
/// Deliberately a short list: everything not named here needs a session, so
/// forgetting to guard a new page fails closed.
const PUBLIC_PATHS: &[&str] = &["/login", "/logout", "/first-run", "/health"];

/// Whether a path can be reached by someone not signed in.
pub fn is_public(path: &str) -> bool {
    path.starts_with("/static/") || PUBLIC_PATHS.contains(&path)
}

/// The signed-in person, as handlers see them.
///
/// Extracting this is what proves a request is authenticated: a handler that
/// takes one cannot run for a visitor who is not signed in, so authorisation
/// is not something a handler can forget to check.
#[derive(Debug, Clone)]
pub struct CurrentUser(pub User);

impl CurrentUser {
    pub fn id(&self) -> i64 {
        self.0.id
    }

    pub fn username(&self) -> &str {
        &self.0.username
    }
}

impl std::ops::Deref for CurrentUser {
    type Target = User;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S: Send + Sync> FromRequestParts<S> for CurrentUser {
    type Rejection = WebError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<CurrentUser>()
            .cloned()
            .ok_or(WebError::NotSignedIn)
    }
}

impl<S: Send + Sync> OptionalFromRequestParts<S> for CurrentUser {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> Result<Option<Self>, Self::Rejection> {
        Ok(parts.extensions.get::<CurrentUser>().cloned())
    }
}

/// Attach the signed-in person to the request, and turn away anyone who is
/// not signed in.
///
/// A browser asking for a page is redirected to the login form; anything
/// under `/api` gets a 401, because redirecting a fetch to an HTML login page
/// produces a confusing parse error rather than a useful one.
pub async fn require_sign_in(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, WebError> {
    let path = request.uri().path().to_string();

    // An instance nobody has claimed yet has no one to sign in as. Send
    // everything to the page that creates the first account, so an upgrade
    // does not present a login form with no valid answer.
    if !state.store.has_any_password().await? {
        if path == "/first-run" || path.starts_with("/static/") {
            return Ok(next.run(request).await);
        }
        return Ok(Redirect::to("/first-run").into_response());
    }

    if is_public(&path) {
        return Ok(next.run(request).await);
    }

    let Some(user) = signed_in_user(&state, request.headers()).await? else {
        if path.starts_with("/api/") {
            return Err(WebError::NotSignedIn);
        }
        return Ok(Redirect::to("/login").into_response());
    };

    let mut request = request;
    request.extensions_mut().insert(CurrentUser(user));
    Ok(next.run(request).await)
}

/// Who this request's session belongs to, if anyone.
///
/// Takes the headers rather than the whole request: a request owns a body,
/// which is not `Sync`, so borrowing the request across the database lookup
/// below would make this future unable to move between threads.
async fn signed_in_user(
    state: &AppState,
    headers: &axum::http::HeaderMap,
) -> Result<Option<User>, WebError> {
    let Some(session_id) = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookies| cookie_value(cookies, COOKIE_NAME))
    else {
        return Ok(None);
    };

    let Some(session) = state.sessions.get(&session_id) else {
        return Ok(None);
    };
    let Some(user_id) = session.user_id else {
        // A session exists — the setup wizard uses one — but nobody has
        // signed in on it.
        return Ok(None);
    };

    // Read the account fresh rather than trusting the session: an account
    // removed while its session was open must stop working immediately.
    let user = state.store.user(user_id).await?;
    if user.is_none() {
        state.sessions.delete(&session_id);
    }

    Ok(user)
}

/// The status a rejected request gets.
pub fn not_signed_in_status() -> StatusCode {
    StatusCode::UNAUTHORIZED
}

#[cfg(test)]
mod tests;
