//! The mail view: every broker conversation in one of six mailboxes.
//!
//! This is the home page. It is not a real mailbox — nothing here opens an
//! IMAP connection — but a view over what eruser already knows: pending
//! tasks become `needs-you`, replies the classifier could not place become
//! `check`, and the run's history becomes `waiting`, `removed`, and
//! `no-record`. The mockup's names, wired to the store's tables.

use axum::extract::{Query, Request, State};
use axum::response::{IntoResponse, Redirect, Response};

use super::{csrf_of, render};
use crate::history::{
    PipelineStatus, ResponseFilter, ResponseType, TaskFilter, TaskStatus, TaskType,
};
use crate::web::auth::CurrentUser;
use crate::web::error::WebError;
use crate::web::state::AppState;
use crate::web::views::{stage_color, stage_label};

/// How many threads a mailbox lists before "+ n more".
const THREAD_LIMIT: usize = 50;

/// What a thread is, once assembled from whatever rows back it.
#[derive(Debug, Clone)]
struct Thread {
    /// `task:{id}` or `response:{id}` — where "open it" goes.
    key: String,
    /// Where in the app the detail lives.
    href: String,
    broker: String,
    broker_id: String,
    date: Option<chrono::DateTime<chrono::Utc>>,
    snippet: String,
    label: &'static str,
    color: &'static str,
    /// Which mailbox it sits in.
    folder: &'static str,
}

impl Thread {
    fn list_row(&self, selected: &str) -> crate::web::views::MailThread {
        crate::web::views::MailThread {
            href: self.href.clone(),
            broker: self.broker.clone(),
            date: self
                .date
                .map(|time| {
                    time.with_timezone(&chrono::Local)
                        .format("%b %-d")
                        .to_string()
                })
                .unwrap_or_default(),
            snippet: one_line(&self.snippet),
            code: self.label.to_string(),
            selected: self.key == selected,
        }
    }
}

/// The first line of a body, as the list shows it.
fn one_line(text: &str) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let mut out: String = line.trim().chars().take(160).collect();
    if out.len() < line.trim().len() {
        out.push('…');
    }
    out
}

/// GET / — the mail view.
pub async fn mail(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(folder): Query<FolderQuery>,
    request: Request,
) -> Result<Response, WebError> {
    if let Some(redirect) = super::require_setup(&user) {
        return Ok(redirect);
    }

    let user_id = user.id();
    let threads = collect_threads(&state, user_id).await?;

    // The selected thread: the one asked for, or the first in the folder.
    let wanted = folder.open.unwrap_or_default();
    let folder_id = match folder.folder.as_str() {
        "needs" | "check" | "waiting" | "removed" | "norecord" | "all" => folder.folder.as_str(),
        _ => "needs",
    };

    let in_folder: Vec<&Thread> = threads
        .iter()
        .filter(|thread| folder_id == "all" || thread.folder == folder_id)
        .collect();

    let selected = in_folder
        .iter()
        .find(|thread| thread.key == wanted)
        .or_else(|| in_folder.first())
        .copied();

    let rows: Vec<_> = in_folder
        .iter()
        .take(THREAD_LIMIT)
        .map(|thread| thread.list_row(selected.map(|s| s.key.as_str()).unwrap_or_default()))
        .collect();

    // Folder counts are over every thread, not just the page's slice.
    let folders = folder_rows(&threads, folder_id);

    // The message pane, when something is selected.
    let message = match selected {
        Some(thread) => Some(message_pane(&state, user_id, thread).await?),
        None => None,
    };
    let has_message = message.is_some();

    // A pending run changes the status line.
    let run = state.jobs.active().map(|job| job.snapshot());

    render(
        &state,
        &user,
        csrf_of(&request).as_ref(),
        "mail.html",
        minijinja::context! {
            title => "Mail",
            folders => folders,
            folder_id => folder_id,
            folder_label => folders.iter().find(|f| f.id == folder_id).map(|f| f.label).unwrap_or("all"),
            folder_summary => format!("{} threads", in_folder.len()),
            threads => rows,
            threads_empty => rows.is_empty(),
            has_more => in_folder.len() > THREAD_LIMIT,
            more_count => in_folder.len().saturating_sub(THREAD_LIMIT),
            has_message => has_message,
            message => message,
            run_active => run.is_some(),
            run_progress => run.as_ref().map(|snapshot| snapshot.progress).unwrap_or(0),
        },
    )
}

/// The folder list with counts, in the mockup's order.
fn folder_rows(threads: &[Thread], current: &str) -> Vec<crate::web::views::MailFolder> {
    let count = |folder: &str| {
        threads
            .iter()
            .filter(|thread| folder == "all" || thread.folder == folder)
            .count()
    };

    let defs: [(&'static str, &'static str, &'static str); 6] = [
        ("needs", "needs-you", "#e0703a"),
        ("check", "check", "#e0703a"),
        ("waiting", "waiting", "#a79f92"),
        ("removed", "removed", "#7fa88a"),
        ("norecord", "no-record", "#8a8276"),
        ("all", "all", "#e6dfd3"),
    ];

    defs.iter()
        .map(|(id, label, color)| crate::web::views::MailFolder {
            id,
            label,
            count: count(id),
            color,
        })
        .map(|folder| crate::web::views::MailFolder {
            id: folder.id,
            label: folder.label,
            count: folder.count,
            color: if folder.id == current {
                // The open folder's count takes the accent, matching the
                // selected row in the thread list.
                "#c2410c"
            } else {
                folder.color
            },
        })
        .collect()
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct FolderQuery {
    pub folder: String,
    /// The thread to open: `task:{id}` or `response:{id}`.
    pub open: Option<String>,
}

/// Every conversation, newest first, across all the sources.
async fn collect_threads(state: &AppState, user_id: i64) -> Result<Vec<Thread>, WebError> {
    let mut threads = Vec::new();

    // Pending tasks are the urgent half: forms to fill, captchas to solve,
    // confirmations to click, reviews a person was already asked for.
    let tasks = state
        .store
        .tasks(
            user_id,
            TaskFilter {
                status: Some(TaskStatus::Pending),
                ..Default::default()
            },
        )
        .await?;

    for task in tasks {
        let folder = match task.task_type {
            TaskType::Review => "check",
            _ => "needs",
        };
        let (label, color) = match task.task_type {
            TaskType::Captcha => ("NEEDS_YOU · captcha", "#e0703a"),
            TaskType::ManualForm => ("NEEDS_YOU · form", "#e0703a"),
            TaskType::Confirm => ("NEEDS_YOU · confirm", "#e0703a"),
            TaskType::DraftReply => ("NEEDS_YOU · draft", "#e0703a"),
            TaskType::Review => ("CHECK", "#e0703a"),
        };
        threads.push(Thread {
            key: format!("task:{}", task.id),
            href: format!("/tasks/{}", task.id),
            broker: task.broker_name.clone(),
            broker_id: task.broker_id.clone(),
            date: task.created_at,
            snippet: if task.notes.is_empty() {
                task.form_url.clone()
            } else {
                task.notes.clone()
            },
            label,
            color,
            folder,
        });
    }

    // Replies the classifier was unsure about, without a task yet. These are
    // what "check" exists for: someone has to say what the broker meant.
    let review = state
        .store
        .broker_responses(
            user_id,
            ResponseFilter {
                needs_review: true,
                limit: Some(200),
                ..Default::default()
            },
        )
        .await?;

    for response in review {
        // A task already covers this reply; the task row wins, so the same
        // conversation is not listed twice.
        let task_key = format!("task:{}", response.broker_id);
        if threads.iter().any(|thread| {
            thread.key == task_key
                || thread.broker_id == response.broker_id && thread.folder == "check"
        }) {
            continue;
        }

        threads.push(Thread {
            key: format!("response:{}", response.id),
            href: format!("/mail/thread/{}", response.id),
            broker: response.broker_name.clone(),
            broker_id: response.broker_id.clone(),
            date: response.received_at.or(response.created_at),
            snippet: if response.email_body.is_empty() {
                response.email_subject.clone()
            } else {
                response.email_body.clone()
            },
            label: "CHECK",
            color: "#e0703a",
            folder: "check",
        });
    }

    // The run's history: what was sent, what came back, what bounced.
    let records = state.store.recent_requests(user_id, 500).await?;
    for record in records {
        let (folder, label) = match record.pipeline_status {
            PipelineStatus::Confirmed => ("removed", "REMOVED"),
            PipelineStatus::Rejected => ("norecord", "NO_RECORD"),
            PipelineStatus::Failed => ("norecord", "BOUNCED"),
            PipelineStatus::FormRequired
            | PipelineStatus::AwaitingCaptcha
            | PipelineStatus::AwaitingConfirmation
            | PipelineStatus::FormFilled
            | PipelineStatus::CaptchaSolved => {
                // Live pipeline stages are already covered by tasks; listing
                // them again here would double-count the same conversation.
                continue;
            }
            PipelineStatus::EmailSent | PipelineStatus::AwaitingResponse => ("waiting", "WAITING"),
        };

        let snippet = match record.status.as_str() {
            "failed" => record.error.clone(),
            _ => format!(
                "Request to delete my personal information — sent as {}",
                record.template
            ),
        };

        threads.push(Thread {
            key: format!("record:{}", record.id),
            href: format!("/mail/record/{}", record.id),
            broker: record.broker_name.clone(),
            broker_id: record.broker_id.clone(),
            date: record.sent_at.or(record.created_at),
            snippet,
            label,
            color: stage_color(record.pipeline_status),
            folder,
        });
    }

    threads.sort_by_key(|thread| std::cmp::Reverse(thread.date));
    Ok(threads)
}

/// What the message pane shows for one thread.
///
/// Tasks route to their own detail page (the pane there carries the form
/// helper, the draft, the screenshot); stored replies render here in full,
/// which is the reading pane the design shows.
async fn message_pane(
    state: &AppState,
    user_id: i64,
    thread: &Thread,
) -> Result<minijinja::Value, WebError> {
    if let Some(response_id) = thread.key.strip_prefix("response:") {
        let id: i64 = response_id.parse().map_err(|_| WebError::NotFound)?;
        let response = state
            .store
            .broker_response(user_id, id)
            .await?
            .ok_or(WebError::NotFound)?;

        let guess = response.needs_review.then(|| {
            minijinja::context! {
                text => crate::web::views::response_guess(response.response_type),
                confidence => format!("{}% sure", (response.confidence * 100.0).round() as i64),
            }
        });

        let paras: Vec<String> = if response.email_body.is_empty() {
            vec![response.email_subject.clone()]
        } else {
            response
                .email_body
                .split("\n\n")
                .map(str::to_string)
                .collect()
        };

        // Resolving a review: say what the reply meant, and the classifier
        // records it like any other classified reply.
        let actions = vec![
            minijinja::context! {
                key => "y",
                label => "that's right — file it",
                href => format!("/mail/thread/{id}/confirm"),
                primary => true,
            },
            minijinja::context! {
                key => "w",
                label => "still waiting",
                href => format!("/mail/thread/{id}/pending"),
                primary => false,
            },
        ];

        return Ok(minijinja::context! {
            from => response.email_from.clone(),
            stamp => response
                .received_at
                .or(response.created_at)
                .map(|time| time.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default(),
            subject => response.email_subject.clone(),
            tag => "CHECK · unsure",
            color => "#e0703a",
            paras => paras,
            guess => guess,
            history => Vec::<minijinja::Value>::new(),
            actions => actions,
        });
    }

    // A task or a history row: the pane summarises and links out to the
    // page that can act on it.
    let record = match thread.key.strip_prefix("record:") {
        Some(record_id) => {
            let id: i64 = record_id.parse().map_err(|_| WebError::NotFound)?;
            state.store.record(user_id, id).await?
        }
        None => None,
    };

    let (subject, tag, color) = match &record {
        Some(record) => (
            format!("Request to {}", thread.broker),
            stage_label(record.pipeline_status).to_string(),
            stage_color(record.pipeline_status),
        ),
        None => (
            format!("Task: {}", thread.broker),
            thread.label.to_string(),
            thread.color,
        ),
    };

    let actions = vec![minijinja::context! {
        key => "⏎",
        label => "open it",
        href => thread.href.clone(),
        primary => true,
    }];

    Ok(minijinja::context! {
        from => format!("you → {}", thread.broker),
        stamp => thread
            .date
            .map(|time| time.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default(),
        subject => subject,
        tag => tag,
        color => color,
        paras => vec![thread.snippet.clone()],
        guess => Option::<minijinja::Value>::None,
        history => Vec::<minijinja::Value>::new(),
        actions => actions,
    })
}

/// POST /mail/thread/{id}/confirm — file an unsure reply as a confirmation
/// the broker really removed the records.
pub async fn confirm_thread(
    State(state): State<AppState>,
    user: CurrentUser,
    axum::extract::Path(response_id): axum::extract::Path<i64>,
) -> Result<Response, WebError> {
    file_response(&state, user.id(), response_id, ResponseType::Success).await
}

/// POST /mail/thread/{id}/pending — file an unsure reply as "no answer yet".
pub async fn pending_thread(
    State(state): State<AppState>,
    user: CurrentUser,
    axum::extract::Path(response_id): axum::extract::Path<i64>,
) -> Result<Response, WebError> {
    file_response(&state, user.id(), response_id, ResponseType::Pending).await
}

/// Record what a person decided an ambiguous reply meant.
///
/// The same update the classifier itself uses, so a human's ruling and a
/// rule's ruling leave exactly the same marks behind — including the move
/// of the broker's pipeline stage.
async fn file_response(
    state: &AppState,
    user_id: i64,
    response_id: i64,
    response_type: ResponseType,
) -> Result<Response, WebError> {
    let response = state
        .store
        .broker_response(user_id, response_id)
        .await?
        .ok_or(WebError::NotFound)?;

    state
        .store
        .update_response_classification(
            response_id,
            response_type,
            &response.form_url,
            &response.confirm_url,
            1.0,
            // A person looked at this and ruled; it is no longer ambiguous.
            false,
        )
        .await?;

    // The broker's pipeline stage follows the ruling, the same move a
    // reclassify would make. The classifier's own mapping is reused, so the
    // two paths cannot disagree about where a reply sends a broker.
    let classifier_type = match response_type {
        ResponseType::Success => crate::inbox::classifier::ResponseType::Success,
        ResponseType::Pending => crate::inbox::classifier::ResponseType::Pending,
        _ => unreachable!("the mail view only files success and pending"),
    };
    state
        .store
        .update_pipeline_status(
            user_id,
            &response.broker_id,
            crate::inbox::scan::stage_for(classifier_type),
        )
        .await?;

    Ok(Redirect::to("/?folder=check").into_response())
}
