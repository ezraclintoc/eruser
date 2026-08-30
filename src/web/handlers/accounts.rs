//! The mailboxes requests are sent from.
//!
//! New in the Rust version. Go kept one set of SMTP settings in the config
//! file, so one provider's daily cap was the whole tool's daily cap. Several
//! accounts can be registered here, and a run rolls over to the next when
//! one is spent.

use axum::extract::{Path, Request, State};
use axum::response::{IntoResponse, Redirect, Response};

use super::{csrf_of, read_form, render};
use crate::history::{AccountScope, DEFAULT_DAILY_LIMIT, NewSenderAccount};
use crate::web::auth::CurrentUser;
use crate::web::error::WebError;
use crate::web::state::AppState;

/// What the add form sends.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct AccountForm {
    pub label: String,
    pub from_address: String,
    pub provider: String,
    pub smtp_host: String,
    pub smtp_port: String,
    pub smtp_username: String,
    pub smtp_password: String,
    pub api_key: String,
    pub daily_limit: String,
    /// Present only when the box is ticked, as browsers do with checkboxes.
    pub family: Option<String>,
}

pub async fn accounts(
    State(state): State<AppState>,
    user: CurrentUser,
    request: Request,
) -> Result<Response, WebError> {
    show(&state, &user, csrf_of(&request).as_ref(), None).await
}

pub async fn add_account(
    State(state): State<AppState>,
    user: CurrentUser,
    request: Request,
) -> Result<Response, WebError> {
    let csrf = csrf_of(&request);
    let form: AccountForm = read_form(request).await?;

    let account = match build(user.id(), &form) {
        Ok(account) => account,
        Err(problem) => return show(&state, &user, csrf.as_ref(), Some(problem)).await,
    };

    state.store.add_sender_account(&account).await?;
    Ok(Redirect::to("/accounts").into_response())
}

pub async fn set_enabled(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, enabled)): Path<(i64, bool)>,
) -> Result<Response, WebError> {
    let changed = state
        .store
        .set_sender_account_enabled(user.id(), id, enabled)
        .await?;

    if !changed {
        return Err(WebError::NotFound);
    }
    Ok(Redirect::to("/accounts").into_response())
}

pub async fn delete_account(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<i64>,
) -> Result<Response, WebError> {
    if !state.store.delete_sender_account(user.id(), id).await? {
        return Err(WebError::NotFound);
    }
    Ok(Redirect::to("/accounts").into_response())
}

/// Render the page, optionally with something that went wrong.
async fn show(
    state: &AppState,
    user: &CurrentUser,
    csrf: Option<&crate::web::security::CsrfToken>,
    problem: Option<String>,
) -> Result<Response, WebError> {
    let capacity = state.store.account_capacity(user.id()).await?;
    let total: i64 = capacity
        .iter()
        .filter(|entry| entry.is_available())
        .map(|entry| entry.remaining)
        .sum();

    // Whose account each row is, so a shared one shows who owns it and is
    // not offered for deletion to someone who does not.
    let owners: std::collections::HashMap<i64, String> = state
        .store
        .users()
        .await?
        .into_iter()
        .map(|owner| (owner.id, owner.username))
        .collect();

    let rows: Vec<_> = capacity
        .iter()
        .map(|entry| {
            minijinja::context! {
                account => &entry.account,
                sent_today => entry.sent_today,
                remaining => entry.remaining,
                shared => entry.account.scope.is_shared(),
                mine => entry.account.user_id == user.id(),
                owner => owners
                    .get(&entry.account.user_id)
                    .cloned()
                    .unwrap_or_else(|| "someone else".to_string()),
            }
        })
        .collect();

    render(
        state,
        user,
        csrf,
        "accounts.html",
        minijinja::context! {
            title => "Sending accounts",
            accounts => rows,
            has_accounts => !capacity.is_empty(),
            total_today => total,
            default_daily_limit => DEFAULT_DAILY_LIMIT,
            error => problem,
        },
    )
}

/// Turn the form into an account, or say what is missing.
///
/// Kept separate from the handler so the rules are testable without a
/// request.
pub(super) fn build(user_id: i64, form: &AccountForm) -> Result<NewSenderAccount, String> {
    let from_address = form.from_address.trim().to_string();
    if from_address.is_empty() {
        return Err("Enter the address to send from.".into());
    }
    if !from_address.contains('@') {
        return Err(format!("{from_address} is not an email address."));
    }

    let provider = match form.provider.trim().to_lowercase().as_str() {
        "" | "smtp" => "smtp".to_string(),
        known @ ("resend" | "sendgrid") => known.to_string(),
        other => return Err(format!("{other} is not a way of sending.")),
    };

    let mut account = NewSenderAccount {
        user_id,
        label: form.label.trim().to_string(),
        scope: if form.family.is_some() {
            AccountScope::Family
        } else {
            // Not shared unless the box was ticked. Sharing an account lets
            // someone else send mail as its owner.
            AccountScope::Personal
        },
        provider: provider.clone(),
        from_address: from_address.clone(),
        daily_limit: parse_limit(&form.daily_limit)?,
        ..NewSenderAccount::default()
    };

    if provider == "smtp" {
        let host = if form.smtp_host.trim().is_empty() {
            crate::email::smtp_host_for(&from_address)
                .ok_or_else(|| {
                    format!(
                        "There is no SMTP server known for {from_address}. \
                         Fill in the server your provider gave you."
                    )
                })?
                .to_string()
        } else {
            form.smtp_host.trim().to_string()
        };

        if form.smtp_password.is_empty() {
            return Err(
                "Enter an app password. Your normal account password will not work.".into(),
            );
        }

        account.smtp = crate::config::SmtpConfig {
            host,
            port: parse_port(&form.smtp_port)?,
            username: if form.smtp_username.trim().is_empty() {
                from_address
            } else {
                form.smtp_username.trim().to_string()
            },
            password: form.smtp_password.clone(),
            use_tls: true,
        };
    } else {
        if form.api_key.trim().is_empty() {
            return Err(format!("Enter the API key for {provider}."));
        }
        account.api_key = form.api_key.trim().to_string();
    }

    Ok(account)
}

/// An empty box means the default rather than an error: most people have no
/// reason to pick a number.
fn parse_limit(raw: &str) -> Result<i64, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(DEFAULT_DAILY_LIMIT);
    }

    match raw.parse::<i64>() {
        Ok(limit) if limit > 0 => Ok(limit),
        _ => Err("The daily limit has to be a number above zero.".into()),
    }
}

fn parse_port(raw: &str) -> Result<u16, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        // Implicit TLS, which is what every provider here wants.
        return Ok(465);
    }

    raw.parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .ok_or_else(|| "The port has to be a number between 1 and 65535.".into())
}

#[cfg(test)]
mod tests;
