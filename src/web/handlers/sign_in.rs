//! Signing in, signing out, and claiming a fresh instance.

use axum::extract::{Request, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Redirect, Response};

use super::{csrf_of, read_form, render_signed_out};
use crate::history::AccountError;
use crate::web::error::WebError;
use crate::web::security::cookie_header;
use crate::web::session::COOKIE_NAME;
use crate::web::state::AppState;

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct Credentials {
    pub username: String,
    pub password: String,
    /// Only on the first-run form, to catch a typo before it locks anyone out.
    pub confirm_password: String,
}

/// The sign-in form.
pub async fn show_login(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, WebError> {
    // An instance nobody has claimed has nobody to sign in as.
    if !state.store.has_any_password().await? {
        return Ok(Redirect::to("/first-run").into_response());
    }

    render_signed_out(
        &state,
        csrf_of(&request).as_ref(),
        "login.html",
        minijinja::context! { title => "Sign in" },
    )
}

pub async fn sign_in(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, WebError> {
    let csrf = csrf_of(&request);
    let credentials: Credentials = read_form(request).await?;

    let user = match state
        .store
        .verify_password(&credentials.username, &credentials.password)
        .await
    {
        Ok(user) => user,
        Err(error) => {
            // Re-render with the name filled in, but never the password.
            return render_signed_out(
                &state,
                csrf.as_ref(),
                "login.html",
                minijinja::context! {
                    title => "Sign in",
                    username => credentials.username,
                    error => sign_in_message(&error),
                },
            );
        }
    };

    // A fresh session id on every sign-in, so a session id someone else may
    // have seen cannot be used to ride in on the new sign-in.
    let session_id = state.sessions.create();
    state.sessions.update(&session_id, |session| {
        session.user_id = Some(user.id);
    });

    let mut response = Redirect::to("/").into_response();
    attach_session(&mut response, &session_id);
    Ok(response)
}

pub async fn sign_out(State(state): State<AppState>, request: Request) -> Response {
    if let Some(session_id) = request
        .headers()
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookies| crate::web::security::cookie_value(cookies, COOKIE_NAME))
    {
        state.sessions.delete(&session_id);
    }

    let mut response = Redirect::to("/login").into_response();
    clear_session(&mut response);
    response
}

/// The page shown on an instance nobody has claimed yet.
pub async fn show_first_run(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, WebError> {
    if state.store.has_any_password().await? {
        return Ok(Redirect::to("/login").into_response());
    }

    // An upgrade from the single-user version arrives here with history
    // already in place, so say so rather than looking like a fresh install
    // that is about to discard it.
    let existing = state.store.stats(crate::history::DEFAULT_USER_ID).await?;

    render_signed_out(
        &state,
        csrf_of(&request).as_ref(),
        "first-run.html",
        minijinja::context! {
            title => "Set up eruser",
            existing_requests => existing.total,
        },
    )
}

/// Claim the instance by setting the first password.
pub async fn first_run(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, WebError> {
    let csrf = csrf_of(&request);
    let credentials: Credentials = read_form(request).await?;

    let problem = if credentials.password != credentials.confirm_password {
        Some("Those two passwords are not the same.".to_string())
    } else {
        state
            .store
            .claim_first_user(&credentials.username, &credentials.password)
            .await
            .err()
            .map(|error| crate::send::error_chain(&error))
    };

    if let Some(problem) = problem {
        let existing = state.store.stats(crate::history::DEFAULT_USER_ID).await?;
        return render_signed_out(
            &state,
            csrf.as_ref(),
            "first-run.html",
            minijinja::context! {
                title => "Set up eruser",
                username => credentials.username,
                existing_requests => existing.total,
                error => problem,
            },
        );
    }

    // Sign them straight in; asking for the password again immediately after
    // choosing it is a pointless step.
    let user = state
        .store
        .verify_password(&credentials.username, &credentials.password)
        .await?;

    let session_id = state.sessions.create();
    state.sessions.update(&session_id, |session| {
        session.user_id = Some(user.id);
    });

    let mut response = Redirect::to("/").into_response();
    attach_session(&mut response, &session_id);
    Ok(response)
}

/// What to tell someone whose sign-in did not work.
///
/// A wrong password and an unknown name give the same answer, so the form
/// cannot be used to find out which accounts exist.
fn sign_in_message(error: &crate::history::Error) -> String {
    match error {
        crate::history::Error::Account(AccountError::NoPasswordSet) => {
            "That account has no password set yet.".to_string()
        }
        crate::history::Error::Account(_) => {
            "That name and password do not match an account.".to_string()
        }
        _ => "Something went wrong signing in. Check the terminal running eruser.".to_string(),
    }
}

fn attach_session(response: &mut Response, session_id: &str) {
    if let Ok(value) = HeaderValue::from_str(&cookie_header(COOKIE_NAME, session_id)) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
}

fn clear_session(response: &mut Response) {
    if let Ok(value) = HeaderValue::from_str(&format!(
        "{COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0"
    )) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
}
