//! The people who can sign in to this instance.
//!
//! New in the Rust version. The model is a household rather than an
//! organisation: everyone here can add someone, and the account that claimed
//! the instance is the one that can remove people. There are no roles or
//! permissions beyond that, because there is nothing here worth building
//! them for — each person's requests and mailboxes are already their own.

use axum::extract::{Path, Request, State};
use axum::response::{IntoResponse, Redirect, Response};

use super::{csrf_of, read_form, render};
use crate::history::MINIMUM_PASSWORD_LENGTH;
use crate::web::auth::CurrentUser;
use crate::web::error::WebError;
use crate::web::state::AppState;

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct NewPersonForm {
    pub username: String,
    pub password: String,
    pub confirm_password: String,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct PasswordForm {
    pub current_password: String,
    pub new_password: String,
    pub confirm_password: String,
}

pub async fn people(
    State(state): State<AppState>,
    user: CurrentUser,
    request: Request,
) -> Result<Response, WebError> {
    show(&state, &user, csrf_of(&request).as_ref(), None, None).await
}

pub async fn add_person(
    State(state): State<AppState>,
    user: CurrentUser,
    request: Request,
) -> Result<Response, WebError> {
    let csrf = csrf_of(&request);
    let form: NewPersonForm = read_form(request).await?;

    if form.password != form.confirm_password {
        return show(
            &state,
            &user,
            csrf.as_ref(),
            Some("Those two passwords are not the same.".into()),
            None,
        )
        .await;
    }

    match state
        .store
        .create_user(form.username.trim(), &form.password)
        .await
    {
        Ok(_) => Ok(Redirect::to("/people").into_response()),
        Err(error) => {
            show(
                &state,
                &user,
                csrf.as_ref(),
                Some(crate::send::error_chain(&error)),
                None,
            )
            .await
        }
    }
}

pub async fn change_password(
    State(state): State<AppState>,
    user: CurrentUser,
    request: Request,
) -> Result<Response, WebError> {
    let csrf = csrf_of(&request);
    let form: PasswordForm = read_form(request).await?;

    if form.new_password != form.confirm_password {
        return show(
            &state,
            &user,
            csrf.as_ref(),
            None,
            Some(Err("Those two passwords are not the same.".into())),
        )
        .await;
    }

    let outcome = state
        .store
        .change_password(user.id(), &form.current_password, &form.new_password)
        .await;

    let message = match outcome {
        Ok(()) => Ok("Your password has been changed."),
        Err(error) => Err(crate::send::error_chain(&error)),
    };

    show(&state, &user, csrf.as_ref(), None, Some(message)).await
}

/// Remove someone, and everything they have sent.
///
/// Only the owner may, and not themselves: an instance whose owner deleted
/// their own account would leave nobody able to remove anyone.
pub async fn remove_person(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> Result<Response, WebError> {
    if !is_owner(&state, &user).await? || id == user.id() {
        return Err(WebError::NotFound);
    }

    if !state.store.delete_user(id).await? {
        return Err(WebError::NotFound);
    }

    // Any session they had is now pointing at an account that is gone; the
    // sign-in check reads the account fresh, so it stops working at once.
    Ok(Redirect::to("/people").into_response())
}

/// Whether this is the account that claimed the instance.
///
/// The first row is the one the single-user version left behind, or the one
/// `/first-run` created, so the lowest id is the owner. Nothing here is
/// worth a real permission system.
async fn is_owner(state: &AppState, user: &CurrentUser) -> Result<bool, WebError> {
    let users = state.store.users().await?;
    Ok(users.first().is_some_and(|first| first.id == user.id()))
}

async fn show(
    state: &AppState,
    user: &CurrentUser,
    csrf: Option<&crate::web::security::CsrfToken>,
    add_error: Option<String>,
    password_message: Option<Result<&str, String>>,
) -> Result<Response, WebError> {
    let owner = is_owner(state, user).await?;
    let users = state.store.users().await?;

    let rows: Vec<_> = users
        .iter()
        .map(|person| {
            minijinja::context! {
                user => person,
                you => person.id == user.id(),
                // The owner cannot be removed, and nobody can remove
                // themselves out of the interface.
                removable => owner && person.id != user.id(),
            }
        })
        .collect();

    render(
        state,
        user,
        csrf,
        "people.html",
        minijinja::context! {
            title => "People",
            people => rows,
            is_owner => owner,
            minimum_password_length => MINIMUM_PASSWORD_LENGTH,
            add_error => add_error,
            password_success => password_message.as_ref().and_then(|m| m.as_ref().ok()),
            password_error => password_message.as_ref().and_then(|m| m.as_ref().err()),
        },
    )
}
