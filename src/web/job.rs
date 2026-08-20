//! Background send jobs and their persisted state.
//!
//! Ported from `internal/web/job.go`. A send to 750 brokers takes far longer
//! than a request, so the browser starts a job and then polls it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

/// Where a job is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Running,
    Completed,
    Cancelled,
    /// Stopped at the daily send limit, with brokers still to go.
    Paused,
    /// Stopped by something the user has to fix, such as a bad password.
    Failed,
}

impl JobStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }
}

/// Why a job stopped, when it stopped badly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// The mail server rejected the credentials.
    Authentication,
    /// The provider is refusing more mail for now.
    RateLimit,
    /// Something else; the message carries the detail.
    Other,
}

/// A point-in-time view of a job, as sent to the browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSnapshot {
    pub id: String,
    pub status: JobStatus,
    /// Percentage complete, 0 to 100.
    pub progress: u8,
    pub sent: usize,
    pub failed: usize,
    pub skipped: usize,
    pub total: usize,
    pub current_broker: String,
    pub started_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<FailureKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daily_limit: Option<usize>,
}

/// The mutable half of a job.
#[derive(Debug)]
struct JobState {
    status: JobStatus,
    sent: usize,
    failed: usize,
    skipped: usize,
    total: usize,
    current_broker: String,
    started_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
    error: Option<String>,
    failure_kind: Option<FailureKind>,
    daily_limit: Option<usize>,
    /// Consecutive authentication failures. One can be a fluke; a run of them
    /// means the password is wrong and continuing would send 700 more
    /// doomed requests.
    consecutive_auth_failures: u32,
}

/// How many authentication failures in a row stop a job.
pub const AUTH_FAILURE_LIMIT: u32 = 3;

/// A handle to a running or finished job.
#[derive(Debug, Clone)]
pub struct Job {
    id: String,
    state: Arc<Mutex<JobState>>,
    cancel: CancellationToken,
}

impl Job {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The token the send pipeline watches.
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Record progress after one broker.
    pub fn update(&self, sent: usize, failed: usize, skipped: usize, current_broker: &str) {
        let mut state = self.lock();
        state.sent = sent;
        state.failed = failed;
        state.skipped = skipped;
        state.current_broker = current_broker.to_string();
    }

    pub fn complete(&self) {
        let mut state = self.lock();
        // Cancelling and finishing can race; whichever landed first wins.
        if state.status.is_terminal() {
            return;
        }
        state.status = JobStatus::Completed;
        state.completed_at = Some(Utc::now());
        state.current_broker.clear();
    }

    /// Stop the job because of something the user must fix.
    ///
    /// Go reported this as `completed` with an error string attached, so the
    /// UI showed a failed run as a successful one.
    pub fn fail(&self, kind: FailureKind, message: impl Into<String>) {
        let mut state = self.lock();
        if state.status.is_terminal() {
            return;
        }
        state.status = JobStatus::Failed;
        state.completed_at = Some(Utc::now());
        state.error = Some(message.into());
        state.failure_kind = Some(kind);
        state.current_broker.clear();
        drop(state);
        self.cancel.cancel();
    }

    /// Stop the job at the daily limit, leaving it resumable.
    pub fn pause_at_limit(&self, message: impl Into<String>) {
        let mut state = self.lock();
        if state.status.is_terminal() {
            return;
        }
        state.status = JobStatus::Paused;
        state.completed_at = Some(Utc::now());
        state.error = Some(message.into());
        state.current_broker.clear();
    }

    pub fn cancel(&self) {
        let mut state = self.lock();
        if state.status.is_terminal() {
            return;
        }
        state.status = JobStatus::Cancelled;
        state.completed_at = Some(Utc::now());
        state.current_broker.clear();
        drop(state);
        self.cancel.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Count an authentication failure, reporting whether the job should stop.
    pub fn record_auth_failure(&self) -> bool {
        let mut state = self.lock();
        state.consecutive_auth_failures += 1;
        state.consecutive_auth_failures >= AUTH_FAILURE_LIMIT
    }

    /// A success clears the streak; the earlier failures were flukes.
    pub fn reset_auth_failures(&self) {
        self.lock().consecutive_auth_failures = 0;
    }

    pub fn status(&self) -> JobStatus {
        self.lock().status
    }

    pub fn snapshot(&self) -> JobSnapshot {
        let state = self.lock();
        let handled = state.sent + state.failed + state.skipped;
        // A run with no brokers is finished the moment it starts; reporting
        // 0% would leave the progress bar stuck. Saturating at 100 covers
        // counts that somehow overshoot the total.
        let progress = handled
            .checked_mul(100)
            .and_then(|scaled| scaled.checked_div(state.total))
            .map_or(100, |percent| percent.min(100) as u8);

        JobSnapshot {
            id: self.id.clone(),
            status: state.status,
            progress,
            sent: state.sent,
            failed: state.failed,
            skipped: state.skipped,
            total: state.total,
            current_broker: state.current_broker.clone(),
            started_at: state.started_at,
            completed_at: state.completed_at,
            error: state.error.clone(),
            failure_kind: state.failure_kind,
            daily_limit: state.daily_limit,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, JobState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Every job this process has run.
#[derive(Debug, Clone, Default)]
pub struct JobManager {
    jobs: Arc<Mutex<HashMap<String, Job>>>,
}

impl JobManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start tracking a new job.
    pub fn create(&self, total: usize, daily_limit: Option<usize>) -> Job {
        let job = Job {
            id: uuid::Uuid::new_v4().to_string(),
            state: Arc::new(Mutex::new(JobState {
                status: JobStatus::Running,
                sent: 0,
                failed: 0,
                skipped: 0,
                total,
                current_broker: String::new(),
                started_at: Utc::now(),
                completed_at: None,
                error: None,
                failure_kind: None,
                daily_limit,
                consecutive_auth_failures: 0,
            })),
            cancel: CancellationToken::new(),
        };

        self.lock().insert(job.id.clone(), job.clone());
        job
    }

    pub fn get(&self, id: &str) -> Option<Job> {
        self.lock().get(id).cloned()
    }

    /// The job currently running, if any.
    ///
    /// Only one send runs at a time, so the browser can ask "is something
    /// happening?" without knowing an id.
    pub fn active(&self) -> Option<Job> {
        self.lock()
            .values()
            .find(|job| job.status() == JobStatus::Running)
            .cloned()
    }

    /// Forget finished jobs older than `max_age`.
    pub fn cleanup(&self, max_age: chrono::Duration) -> usize {
        let cutoff = Utc::now() - max_age;
        let mut jobs = self.lock();
        let before = jobs.len();

        jobs.retain(|_, job| {
            let state = job.lock();
            !state.status.is_terminal()
                || state.completed_at.is_none_or(|finished| finished >= cutoff)
        });

        before - jobs.len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Job>> {
        self.jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// A job saved across restarts, so a run interrupted by a crash or a daily
/// limit can pick up where it stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingJob {
    pub id: String,
    pub status: JobStatus,
    pub sent: usize,
    pub failed: usize,
    pub total: usize,
    pub started_at: DateTime<Utc>,
    /// Broker ids not yet attempted.
    pub remaining_brokers: Vec<String>,
    /// The filters the run was started with, so resuming means the same thing.
    #[serde(default)]
    pub search: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub status_filter: String,
}

/// Reads and writes the pending job file.
#[derive(Debug, Clone)]
pub struct JobPersistence {
    data_dir: PathBuf,
}

impl JobPersistence {
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        Self {
            data_dir: data_dir.as_ref().to_path_buf(),
        }
    }

    pub fn file_path(&self) -> PathBuf {
        self.data_dir.join("pending_job.json")
    }

    pub fn save(&self, job: &PendingJob) -> Result<(), Error> {
        std::fs::create_dir_all(&self.data_dir).map_err(|source| Error::Write {
            path: self.data_dir.clone(),
            source,
        })?;

        let json = serde_json::to_string_pretty(job)?;
        write_owner_only(&self.file_path(), &json)
    }

    /// The saved job, or `None` when there is nothing pending.
    ///
    /// A corrupt file is `None` too, not an error: a half-written JSON blob
    /// must not stop the server from starting.
    pub fn load(&self) -> Option<PendingJob> {
        let data = std::fs::read_to_string(self.file_path()).ok()?;
        match serde_json::from_str(&data) {
            Ok(job) => Some(job),
            Err(error) => {
                tracing::warn!(%error, "ignoring an unreadable pending job file");
                None
            }
        }
    }

    pub fn clear(&self) -> Result<(), Error> {
        match std::fs::remove_file(self.file_path()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(Error::Write {
                path: self.file_path(),
                source,
            }),
        }
    }
}

/// The pending job names every broker still to be contacted, so it is
/// written owner-only like the config.
#[cfg(unix)]
fn write_owner_only(path: &Path, data: &str) -> Result<(), Error> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| Error::Write {
            path: path.to_path_buf(),
            source,
        })?;

    file.write_all(data.as_bytes())
        .map_err(|source| Error::Write {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(not(unix))]
fn write_owner_only(path: &Path, data: &str) -> Result<(), Error> {
    std::fs::write(path, data).map_err(|source| Error::Write {
        path: path.to_path_buf(),
        source,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to write {path}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to serialize the pending job")]
    Serialize(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests;
