//! JSON endpoints, driven by HTMX and the progress poller.

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use super::pages;
use crate::history::{DEFAULT_USER_ID, ResponseFilter, Status, TaskFilter};
use crate::send::{Outcome, Progress, SendJob, SendOptions, sender_for};
use crate::web::error::WebError;
use crate::web::job::{FailureKind, JobStatus, PendingJob};
use crate::web::state::AppState;
use crate::web::views::{BrokerFilters, BrokerWithStatus, HistoryRow, Stats};

/// Sends per run, unless the request asks for fewer.
///
/// Gmail cuts off around 500 messages a day and counts failures. Stopping at
/// 250 leaves headroom for whatever else the account sends.
pub const DEFAULT_DAILY_LIMIT: usize = 250;

pub async fn stats(State(state): State<AppState>) -> Result<Json<Stats>, WebError> {
    Ok(Json(Stats::new(
        state.brokers.brokers.len(),
        state.store.stats(state.user_id).await?,
    )))
}

pub async fn brokers(
    State(state): State<AppState>,
    Query(filters): Query<BrokerFilters>,
) -> Result<Json<Vec<BrokerWithStatus>>, WebError> {
    let filters = filters.normalized();
    let statuses = state.store.all_broker_statuses(state.user_id).await?;

    Ok(Json(
        state
            .brokers
            .brokers
            .iter()
            .map(|broker| BrokerWithStatus::new(broker.clone(), statuses.get(&broker.id)))
            .filter(|row| row.matches(&filters))
            .collect(),
    ))
}

#[derive(Debug, serde::Deserialize)]
pub struct HistoryQuery {
    #[serde(default = "default_history_limit")]
    pub limit: i64,
}

fn default_history_limit() -> i64 {
    100
}

pub async fn history(
    State(state): State<AppState>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<HistoryRow>>, WebError> {
    // Clamped so a hand-edited URL cannot ask for the whole table.
    let limit = query.limit.clamp(1, 1000);

    Ok(Json(
        state
            .store
            .recent_requests(state.user_id, limit)
            .await?
            .into_iter()
            .map(HistoryRow::from)
            .collect(),
    ))
}

/// Clear failed rows so a retry starts from a clean list.
pub async fn delete_failed(State(state): State<AppState>) -> Result<Response, WebError> {
    let deleted = state
        .store
        .delete_by_status(state.user_id, Status::Failed)
        .await?;

    Ok(Json(json!({ "deleted": deleted })).into_response())
}

/// Send to one broker, right now.
pub async fn send_one(
    State(state): State<AppState>,
    Path(broker_id): Path<String>,
) -> Result<Response, WebError> {
    let config = state.config().ok_or(WebError::NotConfigured)?;
    config.validate().map_err(|_| WebError::NotConfigured)?;

    let broker = state
        .brokers
        .find_by_id(&broker_id)
        .ok_or(WebError::NotFound)?
        .clone();

    let job = SendJob {
        brokers: vec![broker],
        profile: config.profile.clone(),
        engine: state.engine.clone(),
        pool: crate::send::SenderPool::single(
            sender_for(&config.email, false)?,
            config.email.from.clone(),
        ),
        store: Some(state.store.clone()),
        options: SendOptions {
            template: config.options.template.clone(),
            from: config.email.from.clone(),
            // One broker, so nothing to pace against.
            rate_limit: Duration::ZERO,
            daily_limit: None,
            user_id: state.user_id,
        },
    };

    let mut outcome = None;
    job.run(&tokio_util::sync::CancellationToken::new(), |event| {
        if let Progress::Broker {
            outcome: result, ..
        } = event
        {
            outcome = Some(result);
        }
    })
    .await;

    Ok(match outcome {
        Some(Outcome::Sent { .. }) => Json(json!({ "status": "sent" })).into_response(),
        Some(Outcome::Failed { error }) => {
            Json(json!({ "status": "failed", "error": error })).into_response()
        }
        _ => Json(json!({ "status": "skipped" })).into_response(),
    })
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct SendAllQuery {
    #[serde(flatten)]
    pub filters: BrokerFilters,
    /// Cap on this run. Absent means the default daily limit.
    pub limit: Option<usize>,
}

/// Start a background send to everything matching the current filters.
pub async fn send_all(
    State(state): State<AppState>,
    Query(query): Query<SendAllQuery>,
) -> Result<Response, WebError> {
    let filters = query.filters.normalized();
    let statuses = state.store.all_broker_statuses(state.user_id).await?;

    let brokers: Vec<_> = state
        .brokers
        .brokers
        .iter()
        .filter(|broker| {
            BrokerWithStatus::new((*broker).clone(), statuses.get(&broker.id)).matches(&filters)
        })
        .cloned()
        .collect();

    if brokers.is_empty() {
        return Err(WebError::BadRequest(
            "No brokers match those filters.".into(),
        ));
    }

    let job = start_send(&state, brokers, &filters, query.limit).await?;
    Ok(Json(job.snapshot()).into_response())
}

/// What is left of a run that stopped before it finished.
pub async fn pending_job(State(state): State<AppState>) -> Response {
    match state.job_persistence.load() {
        Some(pending) if !pending.remaining_brokers.is_empty() => Json(json!({
            "pending": true,
            "remaining": pending.remaining_brokers.len(),
            "sent": pending.sent,
            "failed": pending.failed,
            "total": pending.total,
            "started_at": pending.started_at,
        }))
        .into_response(),
        _ => Json(json!({ "pending": false })).into_response(),
    }
}

/// Continue a run that stopped before it finished.
///
/// Deliberately something you ask for. Upstream resumed automatically two
/// seconds after the server started, which meant opening the interface could
/// put hundreds of emails on the wire without anyone saying so.
pub async fn resume_job(State(state): State<AppState>) -> Result<Response, WebError> {
    let Some(pending) = state.job_persistence.load() else {
        return Err(WebError::BadRequest("There is nothing to resume.".into()));
    };
    if pending.remaining_brokers.is_empty() {
        // Nothing left in it; clear the file rather than leave it lying around.
        let _ = state.job_persistence.clear();
        return Err(WebError::BadRequest("There is nothing to resume.".into()));
    }

    // Resolve the ids against the database as it is now. A broker removed
    // from brokers.yaml since the run started is simply skipped.
    let brokers: Vec<_> = pending
        .remaining_brokers
        .iter()
        .filter_map(|id| state.brokers.find_by_id(id).cloned())
        .collect();

    if brokers.is_empty() {
        let _ = state.job_persistence.clear();
        return Err(WebError::BadRequest(
            "None of the brokers left in that run are still in the database.".into(),
        ));
    }

    let filters = crate::web::views::BrokerFilters {
        search: pending.search.clone(),
        category: pending.category.clone(),
        region: pending.region.clone(),
        status: pending.status_filter.clone(),
    };

    let job = start_send(&state, brokers, &filters, pending.daily_limit).await?;

    // Carry the earlier totals across, so the progress bar continues rather
    // than restarting from zero.
    job.adopt_totals(pending.sent, pending.failed);

    Ok(Json(job.snapshot()).into_response())
}

/// Create a job, record what it has to do, and set it running.
///
/// Shared by starting a run and resuming one, so a resumed run behaves
/// exactly like a fresh one from here on.
async fn start_send(
    state: &AppState,
    brokers: Vec<crate::broker::Broker>,
    filters: &crate::web::views::BrokerFilters,
    limit: Option<usize>,
) -> Result<crate::web::job::Job, WebError> {
    // Two concurrent runs would both count against the same daily limit and
    // interleave their progress, so only one at a time.
    if state.jobs.active().is_some() {
        return Err(WebError::JobAlreadyRunning);
    }

    let config = state.config().ok_or(WebError::NotConfigured)?;
    config.validate().map_err(|_| WebError::NotConfigured)?;

    let daily_limit = limit.unwrap_or(DEFAULT_DAILY_LIMIT);
    let job = state.jobs.create(brokers.len(), Some(daily_limit));
    let snapshot = job.snapshot();

    // Record what is still to do before any of it happens, so a crash
    // mid-run leaves something to resume from.
    state.job_persistence.save(&PendingJob {
        id: job.id().to_string(),
        status: JobStatus::Running,
        sent: 0,
        failed: 0,
        total: brokers.len(),
        started_at: snapshot.started_at,
        remaining_brokers: brokers.iter().map(|b| b.id.clone()).collect(),
        search: filters.search.clone(),
        category: filters.category.clone(),
        region: filters.region.clone(),
        status_filter: filters.status.clone(),
        daily_limit: Some(daily_limit),
    })?;

    // Every account this person may send through, so a run rolls over
    // rather than stopping at one mailbox's daily cap.
    let capacity = state.store.account_capacity(state.user_id).await?;
    let pool = if capacity.is_empty() {
        // Nothing configured as an account yet; fall back to whatever the
        // config file described.
        crate::send::SenderPool::single(
            sender_for(&config.email, false)?,
            config.email.from.clone(),
        )
    } else {
        crate::send::SenderPool::from_capacity(&capacity)
    };

    if pool.is_empty() {
        return Err(WebError::BadRequest(
            "Every sending account has used its allowance for today. \
             Add another account, or try again tomorrow."
                .into(),
        ));
    }

    let options = SendOptions {
        template: config.options.template.clone(),
        from: config.email.from.clone(),
        rate_limit: Duration::from_millis(config.options.rate_limit_ms),
        daily_limit: Some(daily_limit),
        user_id: state.user_id,
    };

    let background = state.clone();
    let running = job.clone();
    let broker_ids: Vec<String> = brokers.iter().map(|b| b.id.clone()).collect();

    tokio::spawn(async move {
        run_send_job(background, running, brokers, broker_ids, pool, options).await;
    });

    Ok(job)
}

/// Drive one background send, keeping the job and the resume file current.
async fn run_send_job(
    state: AppState,
    job: crate::web::job::Job,
    brokers: Vec<crate::broker::Broker>,
    broker_ids: Vec<String>,
    pool: crate::send::SenderPool,
    options: SendOptions,
) {
    let cancel = job.cancellation_token();
    let persistence = state.job_persistence.clone();
    let started_at = job.snapshot().started_at;
    let total = brokers.len();
    let job_id = job.id().to_string();
    let options_daily_limit = options.daily_limit;

    let pipeline = SendJob {
        brokers,
        profile: state.config().unwrap_or_default().profile,
        engine: state.engine.clone(),
        pool,
        store: Some(state.store.clone()),
        options,
    };

    let mut sent = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut stopped_for_auth = false;

    let summary = pipeline
        .run(&cancel, |event| {
            let Progress::Broker {
                index,
                broker_name,
                outcome,
                ..
            } = &event
            else {
                return;
            };

            match outcome {
                Outcome::Sent { .. } => {
                    sent += 1;
                    // A success means earlier auth failures were flukes.
                    job.reset_auth_failures();
                }
                Outcome::Failed { error } => {
                    failed += 1;
                    if looks_like_auth_failure(error) && job.record_auth_failure() {
                        stopped_for_auth = true;
                        job.fail(
                            FailureKind::Authentication,
                            "The mail server rejected the sign-in three times. \
                             Check the address and app password in Settings.",
                        );
                    }
                }
                Outcome::SkippedOverLimit => skipped += 1,
            }

            job.update(sent, failed, skipped, broker_name);

            // Persist what is left, so a crash resumes rather than restarts.
            let remaining = broker_ids.get(*index..).unwrap_or_default().to_vec();
            if let Err(error) = persistence.save(&PendingJob {
                id: job_id.clone(),
                status: JobStatus::Running,
                sent,
                failed,
                total,
                started_at,
                remaining_brokers: remaining,
                search: String::new(),
                category: String::new(),
                region: String::new(),
                status_filter: String::new(),
                daily_limit: options_daily_limit,
            }) {
                tracing::warn!(%error, "could not save send progress");
            }
        })
        .await;

    if stopped_for_auth {
        // Leave the resume file: the run should continue once the password
        // is fixed, not start over.
        return;
    }

    if summary.skipped > 0 && !summary.cancelled {
        job.pause_at_limit(format!(
            "Stopped at {} messages for today. Start it again tomorrow to continue.",
            summary.sent + summary.failed
        ));
        return;
    }

    job.complete();
    if let Err(error) = persistence.clear() {
        tracing::warn!(%error, "could not clear the finished job file");
    }
}

/// Whether a recorded error looks like the credentials being wrong.
///
/// The error text comes from `email::Error`, which is a fixed set of
/// messages, so this matches on our own wording rather than a provider's.
fn looks_like_auth_failure(error: &str) -> bool {
    let error = error.to_lowercase();
    error.contains("authentication") || error.contains("password")
}

pub async fn active_job(State(state): State<AppState>) -> Response {
    match state.jobs.active() {
        Some(job) => Json(json!({ "active": true, "job": job.snapshot() })).into_response(),
        None => Json(json!({ "active": false })).into_response(),
    }
}

pub async fn job_status(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<Response, WebError> {
    let job = state.jobs.get(&job_id).ok_or(WebError::NotFound)?;
    Ok(Json(job.snapshot()).into_response())
}

pub async fn cancel_job(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<Response, WebError> {
    let job = state.jobs.get(&job_id).ok_or(WebError::NotFound)?;
    job.cancel();

    // A cancelled run is not resumed; the remaining brokers stay untouched
    // and can be sent to whenever the user chooses.
    if let Err(error) = state.job_persistence.clear() {
        tracing::warn!(%error, "could not clear the cancelled job file");
    }

    Ok(Json(job.snapshot()).into_response())
}

pub async fn pipeline_stats(State(state): State<AppState>) -> Result<Response, WebError> {
    Ok(Json(pages::pipeline_stats(&state).await?).into_response())
}

pub async fn responses(State(state): State<AppState>) -> Result<Response, WebError> {
    Ok(Json(
        state
            .store
            .broker_responses(
                state.user_id,
                ResponseFilter {
                    limit: Some(200),
                    ..Default::default()
                },
            )
            .await?,
    )
    .into_response())
}

pub async fn tasks(State(state): State<AppState>) -> Result<Response, WebError> {
    Ok(Json(
        state
            .store
            .tasks(state.user_id, TaskFilter::default())
            .await?,
    )
    .into_response())
}

/// Read the mailbox and file whatever brokers have sent back.
pub async fn inbox_scan(State(state): State<AppState>) -> Result<Response, WebError> {
    let config = state.config().ok_or(WebError::NotConfigured)?;
    config
        .validate_inbox()
        .map_err(|problem| WebError::BadRequest(problem.to_string()))?;

    let mut monitor = crate::inbox::Monitor::new(config.inbox.clone(), &state.brokers.brokers);
    let options = crate::inbox::ScanOptions {
        user_id: state.user_id,
        ..Default::default()
    };

    // The scan runs inline rather than as a background job: a week of mail is
    // a handful of seconds, and a job would need its own progress plumbing
    // for no gain.
    let summary = crate::inbox::scan(&mut monitor, &state.store, &options, |_| {})
        .await
        .map_err(WebError::Inbox)?;

    Ok(Json(json!({
        "fetched": summary.fetched,
        "matched": summary.matched,
        "stored": summary.stored,
        "bounced": summary.bounced,
        "needs_review": summary.by_type.needs_review,
        "waiting_on_you": summary.by_type.form_required + summary.by_type.confirmation_required,
    }))
    .into_response())
}

/// Re-read every stored reply with the current patterns.
///
/// Useful after the classifier changes: replies whose bodies were kept are
/// re-read in full, and the rest fall back to their subject lines. No mail is
/// fetched, so this works even after the mailbox has been cleared.
pub async fn inbox_reclassify(State(state): State<AppState>) -> Result<Response, WebError> {
    let changed = crate::inbox::scan::reclassify_stored(&state.store, state.user_id)
        .await
        .map_err(WebError::Inbox)?;

    Ok(Json(json!({ "reclassified": changed })).into_response())
}

/// Forget every stored reply, then read the mailbox again from scratch.
pub async fn inbox_rescan(State(state): State<AppState>) -> Result<Response, WebError> {
    let cleared = state.store.clear_broker_responses(state.user_id).await?;
    let mut response = inbox_scan(State(state)).await?;

    // Report what was discarded alongside what came back.
    response.headers_mut().insert(
        "x-cleared",
        cleared
            .to_string()
            .parse()
            .unwrap_or_else(|_| "0".parse().expect("a valid header")),
    );
    Ok(response)
}

/// The user a request acts as. One user until authentication lands, but
/// routed through here so every call site is already asking the question.
pub fn current_user(_state: &AppState) -> i64 {
    DEFAULT_USER_ID
}

#[cfg(test)]
mod tests;
