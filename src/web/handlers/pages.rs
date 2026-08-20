//! The pages a person navigates to.

use axum::extract::{Path, Query, Request, State};
use axum::response::{IntoResponse, Redirect, Response};

use super::{csrf_of, is_htmx, read_form, render, require_setup};
use crate::history::{FormStatus, ResponseFilter, ResponseType, TaskFilter, TaskStatus, TaskType};
use crate::web::error::WebError;
use crate::web::state::AppState;
use crate::web::views::{
    BrokerFilters, BrokerWithStatus, HistoryRow, PipelineStats, Stats, unique_values,
};

/// How many history rows the dashboard shows.
const DASHBOARD_HISTORY: i64 = 10;
/// How many the history page loads.
const HISTORY_PAGE_LIMIT: i64 = 1000;

pub async fn dashboard(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, WebError> {
    if let Some(redirect) = require_setup(&state) {
        return Ok(redirect);
    }

    let config = state.config().unwrap_or_default();
    let stats = Stats::new(
        state.brokers.brokers.len(),
        state.store.stats(state.user_id).await?,
    );
    let recent: Vec<HistoryRow> = state
        .store
        .recent_requests(state.user_id, DASHBOARD_HISTORY)
        .await?
        .into_iter()
        .map(HistoryRow::from)
        .collect();

    render(
        &state,
        csrf_of(&request).as_ref(),
        "dashboard.html",
        minijinja::context! {
            title => "Dashboard",
            profile => config.profile,
            broker_count => state.brokers.brokers.len(),
            recent_history => recent,
            stats => stats,
            pipeline_stats => pipeline_stats(&state).await?,
        },
    )
}

pub async fn brokers(
    State(state): State<AppState>,
    Query(filters): Query<BrokerFilters>,
    request: Request,
) -> Result<Response, WebError> {
    let filters = filters.normalized();
    let rows = broker_rows(&state, &filters).await?;

    let context = minijinja::context! {
        title => "Data Brokers",
        brokers => &rows,
        categories => unique_values(&state.brokers.brokers, |b| &b.category),
        regions => unique_values(&state.brokers.brokers, |b| &b.region),
        search => &filters.search,
        category => &filters.category,
        region => &filters.region,
        status => &filters.status,
        total => state.brokers.brokers.len(),
        filtered => rows.len(),
    };

    // HTMX asks for just the table when a filter changes.
    let template = if is_htmx(&request) {
        "partials/broker-list.html"
    } else {
        "brokers.html"
    };

    render(&state, csrf_of(&request).as_ref(), template, context)
}

pub async fn history(
    State(state): State<AppState>,
    Query(filters): Query<BrokerFilters>,
    request: Request,
) -> Result<Response, WebError> {
    let filter = filters.normalized().status;

    let mut rows: Vec<HistoryRow> = state
        .store
        .recent_requests(state.user_id, HISTORY_PAGE_LIMIT)
        .await?
        .into_iter()
        .map(HistoryRow::from)
        .collect();

    if matches!(filter.as_str(), "sent" | "failed") {
        rows.retain(|row| row.status == filter);
    }

    let context = minijinja::context! {
        title => "History",
        history => rows,
        status_filter => filter,
    };

    let template = if is_htmx(&request) {
        "partials/history-list.html"
    } else {
        "history.html"
    };

    render(&state, csrf_of(&request).as_ref(), template, context)
}

pub async fn settings(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, WebError> {
    render(
        &state,
        csrf_of(&request).as_ref(),
        "settings.html",
        minijinja::context! {
            title => "Settings",
            config => state.config(),
        },
    )
}

/// Turn on inbox monitoring from the settings page.
pub async fn save_inbox_settings(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, WebError> {
    let csrf = csrf_of(&request);
    let form: InboxForm = read_form(request).await?;

    let message = match apply_inbox_settings(&state, &form) {
        Ok(()) => (
            "Inbox monitoring is on. Replies will be picked up from now on.",
            true,
        ),
        Err(problem) => (problem, false),
    };

    render(
        &state,
        csrf.as_ref(),
        "settings.html",
        minijinja::context! {
            title => "Settings",
            config => state.config(),
            inbox_message => message.0,
            inbox_success => message.1,
        },
    )
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct InboxForm {
    inbox_email: String,
    inbox_password: String,
}

/// Validate and store the inbox settings, returning what to tell the user.
fn apply_inbox_settings(state: &AppState, form: &InboxForm) -> Result<(), &'static str> {
    if form.inbox_email.trim().is_empty() {
        return Err("Enter the email address to watch.");
    }
    if form.inbox_password.is_empty() {
        return Err("Enter an app password. Your normal account password will not work.");
    }

    let mut config = state.config().unwrap_or_default();
    config.inbox = crate::config::InboxConfig {
        enabled: true,
        provider: "gmail".to_string(),
        email: form.inbox_email.trim().to_string(),
        password: form.inbox_password.clone(),
        ..Default::default()
    };
    // Fills in the IMAP host and port that the provider implies.
    config.apply_defaults();

    state
        .save_config(config)
        .map_err(|_| "Could not save the settings. Check the terminal running eruser.")
}

pub async fn pipeline(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, WebError> {
    if let Some(redirect) = require_setup(&state) {
        return Ok(redirect);
    }

    let tasks = state
        .store
        .tasks(
            state.user_id,
            TaskFilter {
                status: Some(TaskStatus::Pending),
                ..Default::default()
            },
        )
        .await?;

    let inbox_configured = state
        .config()
        .is_some_and(|config| config.validate_inbox().is_ok());

    render(
        &state,
        csrf_of(&request).as_ref(),
        "pipeline.html",
        minijinja::context! {
            title => "Pipeline",
            pipeline_stats => pipeline_stats(&state).await?,
            pending_tasks => tasks,
            inbox_configured => inbox_configured,
            recent_responses => state
                .store
                .broker_responses(state.user_id, ResponseFilter { limit: Some(20), ..Default::default() })
                .await?,
        },
    )
}

pub async fn tasks(State(state): State<AppState>, request: Request) -> Result<Response, WebError> {
    if let Some(redirect) = require_setup(&state) {
        return Ok(redirect);
    }

    let open_tasks = state
        .store
        .tasks(
            state.user_id,
            TaskFilter {
                status: Some(TaskStatus::Pending),
                ..Default::default()
            },
        )
        .await?;
    let completed_tasks = state
        .store
        .tasks(
            state.user_id,
            TaskFilter {
                status: Some(TaskStatus::Completed),
                ..Default::default()
            },
        )
        .await?;
    let forms = state.store.forms_with_status(state.user_id).await?;
    let review_items = state
        .store
        .broker_responses(
            state.user_id,
            ResponseFilter {
                needs_review: true,
                limit: Some(100),
                ..Default::default()
            },
        )
        .await?;

    let forms_needing_action: Vec<_> = forms
        .iter()
        .filter(|form| form.status == FormStatus::Pending)
        .cloned()
        .collect();
    let captcha_forms: Vec<_> = forms
        .iter()
        .filter(|form| form.status == FormStatus::Captcha)
        .cloned()
        .collect();
    let filled_forms = forms
        .iter()
        .filter(|form| form.status == FormStatus::Filled)
        .count();

    // Go raised a $hasItems flag from inside three template loops. A Jinja
    // {% set %} does not escape a loop, so the flag is computed here — where
    // the data is anyway.
    let total_action_items =
        forms_needing_action.len() + captcha_forms.len() + open_tasks.len() + review_items.len();

    render(
        &state,
        csrf_of(&request).as_ref(),
        "tasks.html",
        minijinja::context! {
            title => "Tasks",
            tasks => open_tasks,
            completed_tasks => completed_tasks.len(),
            filled_forms => filled_forms,
            completed_tasks_list => completed_tasks,
            forms => forms,
            forms_needing_action => forms_needing_action,
            pending_forms => forms_needing_action.len(),
            captcha_forms => captcha_forms,
            needs_review => review_items.len(),
            review_items => review_items,
            total_action_items => total_action_items,
            has_items => total_action_items > 0,
        },
    )
}

pub async fn forms(State(state): State<AppState>, request: Request) -> Result<Response, WebError> {
    if let Some(redirect) = require_setup(&state) {
        return Ok(redirect);
    }

    let forms = state.store.forms_with_status(state.user_id).await?;

    render(
        &state,
        csrf_of(&request).as_ref(),
        "forms.html",
        minijinja::context! {
            title => "Forms",
            forms => &forms,
            total_forms => forms.len(),
            stats => state.store.form_stats(state.user_id).await?,
        },
    )
}

pub async fn task_detail(
    State(state): State<AppState>,
    Path(task_id): Path<i64>,
    request: Request,
) -> Result<Response, WebError> {
    let task = state
        .store
        .task_by_id(state.user_id, task_id)
        .await?
        .ok_or(WebError::NotFound)?;

    render(
        &state,
        csrf_of(&request).as_ref(),
        "task-detail.html",
        minijinja::context! {
            title => format!("Task: {}", task.broker_name),
            task => task,
        },
    )
}

/// The helper page: the broker's form, alongside the details to paste in.
pub async fn task_helper(
    State(state): State<AppState>,
    Path(task_id): Path<i64>,
    request: Request,
) -> Result<Response, WebError> {
    let task = state
        .store
        .task_by_id(state.user_id, task_id)
        .await?
        .ok_or(WebError::NotFound)?;

    // Record that the user has seen it, so "opened but not finished" is
    // distinguishable from "never looked at".
    state.store.mark_task_opened(state.user_id, task_id).await?;

    let config = state.config().unwrap_or_default();

    render(
        &state,
        csrf_of(&request).as_ref(),
        "task-helper.html",
        minijinja::context! {
            title => format!("Fill in: {}", task.broker_name),
            task => task,
            ordered_profile => profile_fields(&config.profile),
        },
    )
}

/// The profile as label/value pairs, in the order a form usually asks for
/// them, so each can be copied one at a time.
fn profile_fields(profile: &crate::config::Profile) -> Vec<ProfileField> {
    let candidates = [
        ("First name", profile.first_name.clone()),
        ("Last name", profile.last_name.clone()),
        ("Email", profile.email.clone()),
        ("Phone", profile.phone.clone()),
        ("Address", profile.address.clone()),
        ("City", profile.city.clone()),
        ("State", profile.state.clone()),
        ("ZIP code", profile.zip_code.clone()),
        ("Country", profile.country.clone()),
        ("Date of birth", profile.date_of_birth.clone()),
    ];

    candidates
        .into_iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(label, value)| ProfileField {
            label: label.to_string(),
            value,
        })
        .collect()
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProfileField {
    pub label: String,
    pub value: String,
}

pub async fn complete_task(
    State(state): State<AppState>,
    Path(task_id): Path<i64>,
) -> Result<Response, WebError> {
    finish_task(&state, task_id, TaskStatus::Completed).await
}

pub async fn skip_task(
    State(state): State<AppState>,
    Path(task_id): Path<i64>,
) -> Result<Response, WebError> {
    finish_task(&state, task_id, TaskStatus::Skipped).await
}

/// Mark a task done or skipped, 404ing if it does not exist.
///
/// Go's handler reported success either way, so a stale page could show a
/// task being completed that had already been deleted.
async fn finish_task(
    state: &AppState,
    task_id: i64,
    status: TaskStatus,
) -> Result<Response, WebError> {
    if !state
        .store
        .complete_task(state.user_id, task_id, status)
        .await?
    {
        return Err(WebError::NotFound);
    }
    Ok(Redirect::to("/tasks").into_response())
}

/// Mark a broker's form dealt with, without there being a task row for it.
pub async fn complete_form(
    State(state): State<AppState>,
    Path(broker_id): Path<String>,
) -> Result<Response, WebError> {
    advance_form(
        &state,
        &broker_id,
        crate::history::PipelineStatus::FormFilled,
    )
    .await
}

pub async fn skip_form(
    State(state): State<AppState>,
    Path(broker_id): Path<String>,
) -> Result<Response, WebError> {
    advance_form(&state, &broker_id, crate::history::PipelineStatus::Rejected).await
}

async fn advance_form(
    state: &AppState,
    broker_id: &str,
    status: crate::history::PipelineStatus,
) -> Result<Response, WebError> {
    if !state
        .store
        .update_pipeline_status(state.user_id, broker_id, status)
        .await?
    {
        return Err(WebError::NotFound);
    }
    Ok(Redirect::to("/forms").into_response())
}

/// Gather the pipeline counts every page that shows them needs.
pub async fn pipeline_stats(state: &AppState) -> Result<PipelineStats, WebError> {
    let stages = state.store.pipeline_stats(state.user_id).await?;
    let tasks = state.store.task_stats(state.user_id).await?;
    let forms = state.store.form_stats(state.user_id).await?;
    let needs_review = state
        .store
        .broker_responses(
            state.user_id,
            ResponseFilter {
                needs_review: true,
                limit: Some(1000),
                ..Default::default()
            },
        )
        .await?
        .len() as i64;

    Ok(PipelineStats::new(
        &stages,
        tasks.pending,
        forms.pending,
        needs_review,
    ))
}

/// Names used by the task pages, kept here so the values the templates
/// compare against cannot drift from the enum.
pub fn task_type_label(task_type: TaskType) -> &'static str {
    match task_type {
        TaskType::Captcha => "CAPTCHA",
        TaskType::ManualForm => "Form",
        TaskType::Review => "Review",
        TaskType::Confirm => "Confirmation",
    }
}

pub fn response_type_label(response_type: ResponseType) -> &'static str {
    match response_type {
        ResponseType::FormRequired => "Form required",
        ResponseType::ConfirmationRequired => "Confirmation required",
        ResponseType::Success => "Removed",
        ResponseType::Rejected => "Refused",
        ResponseType::Pending => "In progress",
        ResponseType::Bounced => "Address bounced",
        ResponseType::Unknown => "Needs a look",
    }
}

/// Broker rows for the table, filtered and in database order.
async fn broker_rows(
    state: &AppState,
    filters: &BrokerFilters,
) -> Result<Vec<BrokerWithStatus>, WebError> {
    let statuses = state.store.all_broker_statuses(state.user_id).await?;

    Ok(state
        .brokers
        .brokers
        .iter()
        .map(|broker| BrokerWithStatus::new(broker.clone(), statuses.get(&broker.id)))
        .filter(|row| row.matches(filters))
        .collect())
}

#[cfg(test)]
mod tests;
