//! The sending page: accounts, pace, the mailbox, and how replies are read.
//!
//! The accounts page covered the first of these; the rest were spread across
//! settings.html. The mockup puts them on one screen, which is honestly one
//! screen's worth of decisions: what sends, how fast, and what happens to
//! the answers.

use std::collections::HashMap;

use axum::extract::{Request, State};
use axum::response::Response;

use super::{csrf_of, render};
use crate::web::auth::CurrentUser;
use crate::web::error::WebError;
use crate::web::state::AppState;

/// GET /sending — the rotation, the pace, the mailbox, the sorter.
pub async fn sending(
    State(state): State<AppState>,
    user: CurrentUser,
    request: Request,
) -> Result<Response, WebError> {
    if let Some(redirect) = super::require_setup(&user) {
        return Ok(redirect);
    }

    let capacity = state.store.account_capacity(user.id()).await?;

    // Whose account each row is, so a shared one says who owns it.
    let owners: HashMap<i64, String> = state
        .store
        .users()
        .await?
        .into_iter()
        .map(|owner| (owner.id, owner.username))
        .collect();

    let rows: Vec<_> = capacity
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let account = &entry.account;
            minijinja::context! {
                order => index + 1,
                id => account.id,
                label => account.display_name(),
                provider => account.provider,
                from => account.from_address,
                scope => if account.scope.is_shared() { "family" } else { "me" },
                host => account.smtp.host,
                port => account.smtp.port,
                daily_limit => account.daily_limit,
                enabled => account.enabled,
                sent_today => entry.sent_today,
                remaining => entry.remaining,
                paused => !entry.account.enabled,
                mine => account.user_id == user.id(),
                owner => owners
                    .get(&account.user_id)
                    .cloned()
                    .unwrap_or_else(|| "someone else".to_string()),
                is_smtp => account.provider == "smtp",
            }
        })
        .collect();

    let config = user.config();
    let total_today: i64 = capacity
        .iter()
        .filter(|entry| entry.is_available())
        .map(|entry| entry.remaining)
        .sum();

    // A full run at today's capacity, in days.
    let total_brokers = state.brokers.brokers.len() as i64;
    let days_for_full_run = if total_today > 0 {
        (total_brokers + total_today - 1) / total_today
    } else {
        0
    };

    let pipeline = state.pipeline();

    render(
        &state,
        &user,
        csrf_of(&request).as_ref(),
        "sending.html",
        minijinja::context! {
            title => "Sending",
            accounts => rows,
            has_accounts => !capacity.is_empty(),
            total_today => total_today,
            total_brokers => total_brokers,
            days_for_full_run => days_for_full_run,
            default_daily_limit => crate::history::DEFAULT_DAILY_LIMIT,
            // Pace.
            rate_limit_ms => config.options.rate_limit_ms,
            rate_seconds => config.options.rate_limit_ms as f64 / 1000.0,
            // The mailbox replies are read from.
            inbox => minijinja::context! {
                enabled => config.inbox.enabled,
                email => config.inbox.email,
                server => config.inbox.server,
                port => config.inbox.port,
                folder => config.inbox.folder,
            },
            // How replies get sorted: rules always run; the model is the
            // machine's opt-in.
            sorter => minijinja::context! {
                ai_enabled => pipeline.ai.enabled,
                endpoint => pipeline.ai.endpoint,
                model => pipeline.ai.model,
                timeout_sec => pipeline.ai.timeout_sec,
                max_drafts => pipeline.ai.max_drafts_per_run,
            },
        },
    )
}
