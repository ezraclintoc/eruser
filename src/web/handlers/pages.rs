//! The pages a person navigates to.

use axum::extract::{Path, Query, Request, State};
use axum::response::{IntoResponse, Redirect, Response};

use super::{csrf_of, is_htmx, read_form, render, require_setup};
use crate::history::{FormStatus, ResponseFilter, ResponseType, TaskFilter, TaskStatus, TaskType};
use crate::web::auth::CurrentUser;
use crate::web::error::WebError;
use crate::web::state::AppState;
use crate::web::views::{
    BrokerFilters, BrokerWithStatus, HistoryRow, PipelineStats, unique_values,
};

/// How many the history page loads.
const HISTORY_PAGE_LIMIT: i64 = 1000;

/// The dashboard is gone: the mail view is the home page now. Kept as a
/// redirect so old bookmarks and the error page's "back to the dashboard"
/// link still land somewhere real.
pub async fn dashboard(_state: State<AppState>, _user: CurrentUser) -> Response {
    axum::response::Redirect::to("/").into_response()
}

pub async fn brokers(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(filters): Query<BrokerFilters>,
    request: Request,
) -> Result<Response, WebError> {
    let filters = filters.normalized();
    let rows = broker_rows(&state, user.id(), &filters).await?;

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

    render(&state, &user, csrf_of(&request).as_ref(), template, context)
}

pub async fn history(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(filters): Query<BrokerFilters>,
    request: Request,
) -> Result<Response, WebError> {
    let filter = filters.normalized().status;

    let mut rows: Vec<HistoryRow> = state
        .store
        .recent_requests(user.id(), HISTORY_PAGE_LIMIT)
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

    render(&state, &user, csrf_of(&request).as_ref(), template, context)
}

pub async fn settings(
    State(state): State<AppState>,
    user: CurrentUser,
    request: Request,
) -> Result<Response, WebError> {
    render(
        &state,
        &user,
        csrf_of(&request).as_ref(),
        "settings.html",
        minijinja::context! {
            title => "Settings",
            config => user.config(),
        },
    )
}

/// Turn on inbox monitoring from the settings page.
pub async fn save_inbox_settings(
    State(state): State<AppState>,
    user: CurrentUser,
    request: Request,
) -> Result<Response, WebError> {
    let csrf = csrf_of(&request);
    let form: InboxForm = read_form(request).await?;

    let message = match apply_inbox_settings(&state, &user, &form).await {
        Ok(()) => (
            "Inbox monitoring is on. Replies will be picked up from now on.",
            true,
        ),
        Err(problem) => (problem, false),
    };

    render(
        &state,
        &user,
        csrf.as_ref(),
        "settings.html",
        minijinja::context! {
            title => "Settings",
            config => user.config(),
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
///
/// The mailbox belongs to the person, not the install: two people on one
/// instance watch two different inboxes.
async fn apply_inbox_settings(
    state: &AppState,
    user: &CurrentUser,
    form: &InboxForm,
) -> Result<(), &'static str> {
    let mut config = user.config().clone();
    config.inbox = inbox_from(form)?;
    // Fills in the IMAP host and port that the provider implies.
    config.apply_defaults();

    state
        .store
        .save_settings(user.id(), &config.options, &config.inbox)
        .await
        .map_err(|_| "Could not save the settings. Check the terminal running eruser.")
}

/// The inbox settings a filled-in form describes.
fn inbox_from(form: &InboxForm) -> Result<crate::config::InboxConfig, &'static str> {
    if form.inbox_email.trim().is_empty() {
        return Err("Enter the email address to watch.");
    }
    if form.inbox_password.is_empty() {
        return Err("Enter an app password. Your normal account password will not work.");
    }

    Ok(crate::config::InboxConfig {
        enabled: true,
        provider: "gmail".to_string(),
        email: form.inbox_email.trim().to_string(),
        password: form.inbox_password.clone(),
        ..Default::default()
    })
}

pub async fn pipeline(
    State(state): State<AppState>,
    user: CurrentUser,
    request: Request,
) -> Result<Response, WebError> {
    if let Some(redirect) = require_setup(&user) {
        return Ok(redirect);
    }

    let tasks = state
        .store
        .tasks(
            user.id(),
            TaskFilter {
                status: Some(TaskStatus::Pending),
                ..Default::default()
            },
        )
        .await?;

    let inbox_configured = user.config().validate_inbox().is_ok();

    render(
        &state,
        &user,
        csrf_of(&request).as_ref(),
        "pipeline.html",
        minijinja::context! {
            title => "Pipeline",
            pipeline_stats => pipeline_stats(&state, user.id()).await?,
            pending_tasks => tasks,
            inbox_configured => inbox_configured,
            recent_responses => state
                .store
                .broker_responses(user.id(), ResponseFilter { limit: Some(20), ..Default::default() })
                .await?,
        },
    )
}

pub async fn tasks(
    State(state): State<AppState>,
    user: CurrentUser,
    request: Request,
) -> Result<Response, WebError> {
    if let Some(redirect) = require_setup(&user) {
        return Ok(redirect);
    }

    let open_tasks = state
        .store
        .tasks(
            user.id(),
            TaskFilter {
                status: Some(TaskStatus::Pending),
                ..Default::default()
            },
        )
        .await?;
    let completed_tasks = state
        .store
        .tasks(
            user.id(),
            TaskFilter {
                status: Some(TaskStatus::Completed),
                ..Default::default()
            },
        )
        .await?;
    let forms = state.store.forms_with_status(user.id()).await?;
    let review_items = state
        .store
        .broker_responses(
            user.id(),
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
        &user,
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

pub async fn forms(
    State(state): State<AppState>,
    user: CurrentUser,
    request: Request,
) -> Result<Response, WebError> {
    if let Some(redirect) = require_setup(&user) {
        return Ok(redirect);
    }

    let forms = state.store.forms_with_status(user.id()).await?;

    render(
        &state,
        &user,
        csrf_of(&request).as_ref(),
        "forms.html",
        minijinja::context! {
            title => "Forms",
            forms => &forms,
            total_forms => forms.len(),
            stats => state.store.form_stats(user.id()).await?,
        },
    )
}

pub async fn task_detail(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(task_id): Path<i64>,
    request: Request,
) -> Result<Response, WebError> {
    let task = state
        .store
        .task_by_id(user.id(), task_id)
        .await?
        .ok_or(WebError::NotFound)?;

    // A draft task carries the draft id in its browser_state field; load it
    // so the page can show the model's words and offer send / discard.
    let draft = if task.task_type == TaskType::DraftReply
        && let Ok(draft_id) = task.browser_state.parse::<i64>()
    {
        state.store.draft(user.id(), draft_id).await.ok()
    } else {
        None
    };

    render(
        &state,
        &user,
        csrf_of(&request).as_ref(),
        "task-detail.html",
        minijinja::context! {
            title => format!("Task: {}", task.broker_name),
            task => task,
            draft => draft,
        },
    )
}

/// Send a draft reply to its broker. The rails live in `reply::auto_send`:
/// the whitelist is re-checked there, identity requests never pass, and the
/// draft is re-validated against the current rules before anything goes on
/// the wire. This handler is the manual path, but it goes through the same
/// door — a person pressing send is not a reason to skip the checks.
pub async fn send_draft(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(task_id): Path<i64>,
) -> Result<Response, WebError> {
    let task = state
        .store
        .task_by_id(user.id(), task_id)
        .await?
        .ok_or(WebError::NotFound)?;
    let draft_id = task
        .browser_state
        .parse::<i64>()
        .ok()
        .ok_or(WebError::NotFound)?;
    let draft = state
        .store
        .draft(user.id(), draft_id)
        .await
        .map_err(|_| WebError::NotFound)?;

    let config = user.config().clone();
    let whitelist = state.pipeline().ai.auto_send;

    // The person's own address as the From, through the ordinary sender.
    let sender = crate::email::new_sender(&config.email)?;
    match crate::reply::auto_send::send_draft(
        &state.store,
        sender.as_ref(),
        &config.email.from,
        draft,
        &whitelist,
        &config.profile,
    )
    .await
    {
        Ok(_) => Ok(Redirect::to(&format!("/tasks/{task_id}")).into_response()),
        Err(problem) => Err(WebError::BadRequest(problem.to_string())),
    }
}

/// A person read the draft and did not want it.
pub async fn discard_draft(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(task_id): Path<i64>,
) -> Result<Response, WebError> {
    let task = state
        .store
        .task_by_id(user.id(), task_id)
        .await?
        .ok_or(WebError::NotFound)?;
    let draft_id = task
        .browser_state
        .parse::<i64>()
        .ok()
        .ok_or(WebError::NotFound)?;

    state
        .store
        .discard_draft(user.id(), draft_id)
        .await
        .map_err(|_| WebError::NotFound)?;

    // The task has nothing left to wait for.
    state
        .store
        .complete_task(user.id(), task_id, TaskStatus::Completed)
        .await?;

    Ok(Redirect::to("/tasks").into_response())
}

/// The helper page: the broker's form, alongside the details to paste in.
pub async fn task_helper(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(task_id): Path<i64>,
    request: Request,
) -> Result<Response, WebError> {
    let task = state
        .store
        .task_by_id(user.id(), task_id)
        .await?
        .ok_or(WebError::NotFound)?;

    // Record that the user has seen it, so "opened but not finished" is
    // distinguishable from "never looked at".
    state.store.mark_task_opened(user.id(), task_id).await?;

    let config = user.config().clone();

    render(
        &state,
        &user,
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
    user: CurrentUser,
    Path(task_id): Path<i64>,
) -> Result<Response, WebError> {
    finish_task(&state, user.id(), task_id, TaskStatus::Completed).await
}

pub async fn skip_task(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(task_id): Path<i64>,
) -> Result<Response, WebError> {
    finish_task(&state, user.id(), task_id, TaskStatus::Skipped).await
}

/// Mark a task done or skipped, 404ing if it does not exist.
///
/// Go's handler reported success either way, so a stale page could show a
/// task being completed that had already been deleted.
async fn finish_task(
    state: &AppState,
    user_id: i64,
    task_id: i64,
    status: TaskStatus,
) -> Result<Response, WebError> {
    if !state.store.complete_task(user_id, task_id, status).await? {
        return Err(WebError::NotFound);
    }
    Ok(Redirect::to("/tasks").into_response())
}

/// Mark a broker's form dealt with, without there being a task row for it.
pub async fn complete_form(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(broker_id): Path<String>,
) -> Result<Response, WebError> {
    advance_form(
        &state,
        user.id(),
        &broker_id,
        crate::history::PipelineStatus::FormFilled,
    )
    .await
}

pub async fn skip_form(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(broker_id): Path<String>,
) -> Result<Response, WebError> {
    advance_form(
        &state,
        user.id(),
        &broker_id,
        crate::history::PipelineStatus::Rejected,
    )
    .await
}

async fn advance_form(
    state: &AppState,
    user_id: i64,
    broker_id: &str,
    status: crate::history::PipelineStatus,
) -> Result<Response, WebError> {
    if !state
        .store
        .update_pipeline_status(user_id, broker_id, status)
        .await?
    {
        return Err(WebError::NotFound);
    }
    Ok(Redirect::to("/forms").into_response())
}

/// Gather the pipeline counts every page that shows them needs.
pub async fn pipeline_stats(state: &AppState, user_id: i64) -> Result<PipelineStats, WebError> {
    let stages = state.store.pipeline_stats(user_id).await?;
    let tasks = state.store.task_stats(user_id).await?;
    let forms = state.store.form_stats(user_id).await?;
    let needs_review = state
        .store
        .broker_responses(
            user_id,
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
        TaskType::DraftReply => "Draft reply",
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
    user_id: i64,
    filters: &BrokerFilters,
) -> Result<Vec<BrokerWithStatus>, WebError> {
    let statuses = state.store.all_broker_statuses(user_id).await?;

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
