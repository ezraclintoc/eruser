//! Persistent history: requests sent, replies received, work still pending.
//!
//! Ported from `internal/history/history.go`. Backed by SQLite through sqlx,
//! with the schema in `migrations/`.
//!
//! Every row belongs to a user. Until authentication exists that is always
//! [`DEFAULT_USER_ID`], but the column is present from the first migration so
//! that multi-user support does not require rewriting existing databases.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use chrono::{Datelike, TimeZone, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteRow};
use sqlx::{Row, SqlitePool};

mod error;
mod types;

pub use error::Error;
pub use types::*;

/// Bundled migrations, embedded at compile time.
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Handle to the history database.
#[derive(Debug, Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    /// Open (creating if needed) the database at `path` and run migrations.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|source| Error::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            // The web UI reads while a send job writes; WAL keeps readers
            // from blocking on the writer.
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true)
            .busy_timeout(std::time::Duration::from_secs(10));

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await
            .map_err(|source| Error::Open {
                path: path.to_path_buf(),
                source,
            })?;

        Self::from_pool(pool).await
    }

    /// An ephemeral database, for tests.
    pub async fn open_in_memory() -> Result<Self, Error> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::from_str(":memory:")
                    .expect("in-memory connect string is valid")
                    .foreign_keys(true),
            )
            .await
            .map_err(|source| Error::Open {
                path: PathBuf::from(":memory:"),
                source,
            })?;

        Self::from_pool(pool).await
    }

    async fn from_pool(pool: SqlitePool) -> Result<Self, Error> {
        MIGRATOR.run(&pool).await.map_err(Error::Migrate)?;
        Ok(Self { pool })
    }

    /// The underlying pool, for callers that need their own queries.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Default location: `~/.eraser/history.db`.
    pub fn default_path() -> PathBuf {
        match crate::config::home_dir() {
            Some(home) => home.join(".eraser").join("history.db"),
            None => PathBuf::from("eraser_history.db"),
        }
    }

    // ---------------------------------------------------------------
    // Removal requests
    // ---------------------------------------------------------------

    /// Record an attempted removal request, returning its new id.
    pub async fn add_record(&self, record: &NewRecord) -> Result<i64, Error> {
        let id = sqlx::query(
            "INSERT INTO removal_requests
                 (user_id, broker_id, broker_name, email, template, status,
                  message_id, error, sent_at, created_at, pipeline_status)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(record.user_id)
        .bind(&record.broker_id)
        .bind(&record.broker_name)
        .bind(&record.email)
        .bind(&record.template)
        .bind(record.status.as_str())
        .bind(&record.message_id)
        .bind(&record.error)
        .bind(record.sent_at)
        .bind(Utc::now())
        .bind(PipelineStatus::EmailSent.as_str())
        .execute(&self.pool)
        .await?
        .last_insert_rowid();

        Ok(id)
    }

    pub async fn last_request_for_broker(
        &self,
        user_id: i64,
        broker_id: &str,
    ) -> Result<Option<Record>, Error> {
        let row = sqlx::query(
            "SELECT * FROM removal_requests
             WHERE user_id = ? AND broker_id = ?
             ORDER BY sent_at DESC, id DESC LIMIT 1",
        )
        .bind(user_id)
        .bind(broker_id)
        .fetch_optional(&self.pool)
        .await?;

        row.map(record_from_row).transpose()
    }

    pub async fn recent_requests(&self, user_id: i64, limit: i64) -> Result<Vec<Record>, Error> {
        let rows = sqlx::query(
            "SELECT * FROM removal_requests
             WHERE user_id = ?
             ORDER BY sent_at DESC, id DESC LIMIT ?",
        )
        .bind(user_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(record_from_row).collect()
    }

    pub async fn stats(&self, user_id: i64) -> Result<Stats, Error> {
        let row = sqlx::query(
            "SELECT COUNT(*)                                        AS total,
                    SUM(CASE WHEN status = 'sent'   THEN 1 ELSE 0 END) AS sent,
                    SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END) AS failed
             FROM removal_requests WHERE user_id = ?",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(Stats {
            total: row.try_get::<i64, _>("total")?,
            // SUM over zero rows is NULL, not 0.
            sent: row.try_get::<Option<i64>, _>("sent")?.unwrap_or(0),
            failed: row.try_get::<Option<i64>, _>("failed")?.unwrap_or(0),
        })
    }

    /// Sent and failed counts since the first of the current month, local time.
    pub async fn monthly_stats(&self, user_id: i64) -> Result<Stats, Error> {
        let now = chrono::Local::now();
        let start = chrono::Local
            .with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
            .single()
            .map(|dt| dt.with_timezone(&Utc))
            // A DST transition at local midnight on the 1st can make that
            // instant ambiguous or nonexistent; fall back to UTC.
            .unwrap_or_else(|| {
                Utc.with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
                    .single()
                    .expect("the first of a month at midnight UTC always exists")
            });

        let row = sqlx::query(
            "SELECT COUNT(*)                                           AS total,
                    SUM(CASE WHEN status = 'sent'   THEN 1 ELSE 0 END) AS sent,
                    SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END) AS failed
             FROM removal_requests WHERE user_id = ? AND sent_at >= ?",
        )
        .bind(user_id)
        .bind(start)
        .fetch_one(&self.pool)
        .await?;

        Ok(Stats {
            total: row.try_get::<i64, _>("total")?,
            sent: row.try_get::<Option<i64>, _>("sent")?.unwrap_or(0),
            failed: row.try_get::<Option<i64>, _>("failed")?.unwrap_or(0),
        })
    }

    /// Latest status per broker, keyed by broker id.
    pub async fn all_broker_statuses(
        &self,
        user_id: i64,
    ) -> Result<std::collections::HashMap<String, BrokerStatus>, Error> {
        // The correlated subquery in the Go version ran once per broker.
        // A window function does the same work in a single pass.
        let rows = sqlx::query(
            "SELECT broker_id,
                    MAX(sent_at) AS last_sent,
                    COUNT(*)     AS total_sent,
                    (SELECT status FROM removal_requests inner_rr
                      WHERE inner_rr.user_id = outer_rr.user_id
                        AND inner_rr.broker_id = outer_rr.broker_id
                      ORDER BY sent_at DESC, id DESC LIMIT 1) AS status
             FROM removal_requests outer_rr
             WHERE user_id = ?
             GROUP BY broker_id",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                let status = BrokerStatus {
                    broker_id: row.try_get("broker_id")?,
                    last_sent: row.try_get("last_sent")?,
                    status: Status::from_db(&row.try_get::<String, _>("status")?),
                    total_sent: row.try_get("total_sent")?,
                };
                Ok((status.broker_id.clone(), status))
            })
            .collect()
    }

    /// Delete every request with the given status. Returns how many went.
    pub async fn delete_by_status(&self, user_id: i64, status: Status) -> Result<u64, Error> {
        let deleted = sqlx::query("DELETE FROM removal_requests WHERE user_id = ? AND status = ?")
            .bind(user_id)
            .bind(status.as_str())
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(deleted)
    }

    // ---------------------------------------------------------------
    // Broker responses
    // ---------------------------------------------------------------

    /// Insert a classified reply, or update the existing row for the same
    /// (user, broker, subject).
    ///
    /// Upstream inserted unconditionally after a separate lookup, which let
    /// two monitor runs race and store the same reply twice. The unique index
    /// plus an upsert makes that impossible.
    pub async fn upsert_broker_response(&self, response: &NewBrokerResponse) -> Result<i64, Error> {
        let now = Utc::now();
        let id = sqlx::query(
            "INSERT INTO broker_responses
                 (user_id, broker_id, broker_name, response_type, email_from,
                  email_subject, email_body, form_url, confirm_url, confidence,
                  needs_review, received_at, processed_at, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(user_id, broker_id, email_subject) DO UPDATE SET
                 response_type = excluded.response_type,
                 email_from    = excluded.email_from,
                 email_body    = excluded.email_body,
                 form_url      = excluded.form_url,
                 confirm_url   = excluded.confirm_url,
                 confidence    = excluded.confidence,
                 needs_review  = excluded.needs_review,
                 received_at   = excluded.received_at,
                 processed_at  = excluded.processed_at
             RETURNING id",
        )
        .bind(response.user_id)
        .bind(&response.broker_id)
        .bind(&response.broker_name)
        .bind(response.response_type.as_str())
        .bind(&response.email_from)
        .bind(&response.email_subject)
        .bind(&response.email_body)
        .bind(&response.form_url)
        .bind(&response.confirm_url)
        .bind(response.confidence)
        .bind(response.needs_review)
        .bind(response.received_at)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await?
        .try_get("id")?;

        Ok(id)
    }

    pub async fn find_response_by_subject(
        &self,
        user_id: i64,
        broker_id: &str,
        subject: &str,
    ) -> Result<Option<BrokerResponse>, Error> {
        let row = sqlx::query(
            "SELECT * FROM broker_responses
             WHERE user_id = ? AND broker_id = ? AND email_subject = ? LIMIT 1",
        )
        .bind(user_id)
        .bind(broker_id)
        .bind(subject)
        .fetch_optional(&self.pool)
        .await?;

        row.map(response_from_row).transpose()
    }

    /// Replace the classification of an existing reply, e.g. after a
    /// classifier change or a manual correction.
    pub async fn update_response_classification(
        &self,
        id: i64,
        response_type: ResponseType,
        form_url: &str,
        confirm_url: &str,
        confidence: f64,
        needs_review: bool,
    ) -> Result<(), Error> {
        sqlx::query(
            "UPDATE broker_responses
             SET response_type = ?, form_url = ?, confirm_url = ?,
                 confidence = ?, needs_review = ?, processed_at = ?
             WHERE id = ?",
        )
        .bind(response_type.as_str())
        .bind(form_url)
        .bind(confirm_url)
        .bind(confidence)
        .bind(needs_review)
        .bind(Utc::now())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_response_body(&self, id: i64, body: &str) -> Result<(), Error> {
        sqlx::query("UPDATE broker_responses SET email_body = ? WHERE id = ?")
            .bind(body)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Remove every stored reply for a user, for a full re-scan.
    pub async fn clear_broker_responses(&self, user_id: i64) -> Result<u64, Error> {
        let deleted = sqlx::query("DELETE FROM broker_responses WHERE user_id = ?")
            .bind(user_id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(deleted)
    }

    /// List replies, newest first.
    ///
    /// Go had two near-identical functions here — one that fetched the body
    /// and one that did not — differing only in a column list and four
    /// branches of hand-written SQL. This is one function with a filter.
    pub async fn broker_responses(
        &self,
        user_id: i64,
        filter: ResponseFilter,
    ) -> Result<Vec<BrokerResponse>, Error> {
        let mut sql = String::from("SELECT * FROM broker_responses WHERE user_id = ?");
        if filter.response_type.is_some() {
            sql.push_str(" AND response_type = ?");
        }
        if filter.needs_review {
            sql.push_str(" AND needs_review = 1");
        }
        sql.push_str(" ORDER BY created_at DESC, id DESC");
        if filter.limit.is_some() {
            sql.push_str(" LIMIT ?");
        }

        // Safe to assert: every fragment above is a string literal, and all
        // values are bound as parameters below.
        let mut query = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(user_id);
        if let Some(response_type) = filter.response_type {
            query = query.bind(response_type.as_str());
        }
        if let Some(limit) = filter.limit {
            query = query.bind(limit);
        }

        let rows = query.fetch_all(&self.pool).await?;
        rows.into_iter().map(response_from_row).collect()
    }

    /// Counts per response type.
    pub async fn response_stats(
        &self,
        user_id: i64,
    ) -> Result<std::collections::HashMap<ResponseType, i64>, Error> {
        let rows = sqlx::query(
            "SELECT response_type, COUNT(*) AS count
             FROM broker_responses WHERE user_id = ? GROUP BY response_type",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok((
                    ResponseType::from_db(&row.try_get::<String, _>("response_type")?),
                    row.try_get::<i64, _>("count")?,
                ))
            })
            .collect()
    }

    // ---------------------------------------------------------------
    // Pending tasks
    // ---------------------------------------------------------------

    pub async fn add_task(&self, task: &NewPendingTask) -> Result<i64, Error> {
        let id = sqlx::query(
            "INSERT INTO pending_tasks
                 (user_id, broker_id, broker_name, task_type, form_url,
                  screenshot_path, browser_state, notes, status, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(task.user_id)
        .bind(&task.broker_id)
        .bind(&task.broker_name)
        .bind(task.task_type.as_str())
        .bind(&task.form_url)
        .bind(&task.screenshot_path)
        .bind(&task.browser_state)
        .bind(&task.notes)
        .bind(TaskStatus::Pending.as_str())
        .bind(Utc::now())
        .execute(&self.pool)
        .await?
        .last_insert_rowid();

        Ok(id)
    }

    pub async fn tasks(&self, user_id: i64, filter: TaskFilter) -> Result<Vec<PendingTask>, Error> {
        let mut sql = String::from("SELECT * FROM pending_tasks WHERE user_id = ?");
        if filter.task_type.is_some() {
            sql.push_str(" AND task_type = ?");
        }
        if filter.status.is_some() {
            sql.push_str(" AND status = ?");
        }
        sql.push_str(" ORDER BY created_at DESC, id DESC");

        // Safe to assert: every fragment above is a string literal, and all
        // values are bound as parameters below.
        let mut query = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(user_id);
        if let Some(task_type) = filter.task_type {
            query = query.bind(task_type.as_str());
        }
        if let Some(status) = filter.status {
            query = query.bind(status.as_str());
        }

        let rows = query.fetch_all(&self.pool).await?;
        rows.into_iter().map(task_from_row).collect()
    }

    /// Fetch one task, scoped to its owner so a guessed id cannot read
    /// another user's task once authentication exists.
    pub async fn task_by_id(&self, user_id: i64, id: i64) -> Result<Option<PendingTask>, Error> {
        let row = sqlx::query("SELECT * FROM pending_tasks WHERE user_id = ? AND id = ?")
            .bind(user_id)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;

        row.map(task_from_row).transpose()
    }

    /// Mark a task finished or skipped. Returns false if no such task exists.
    pub async fn complete_task(
        &self,
        user_id: i64,
        id: i64,
        status: TaskStatus,
    ) -> Result<bool, Error> {
        let affected = sqlx::query(
            "UPDATE pending_tasks SET status = ?, completed_at = ?
             WHERE user_id = ? AND id = ?",
        )
        .bind(status.as_str())
        .bind(Utc::now())
        .bind(user_id)
        .bind(id)
        .execute(&self.pool)
        .await?
        .rows_affected();

        Ok(affected > 0)
    }

    /// Stamp the first time the user opened a task's helper page.
    pub async fn mark_task_opened(&self, user_id: i64, id: i64) -> Result<(), Error> {
        sqlx::query(
            "UPDATE pending_tasks SET opened_at = ?
             WHERE user_id = ? AND id = ? AND opened_at IS NULL",
        )
        .bind(Utc::now())
        .bind(user_id)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn task_stats(&self, user_id: i64) -> Result<TaskStats, Error> {
        let row = sqlx::query(
            "SELECT SUM(CASE WHEN status = 'pending'   THEN 1 ELSE 0 END) AS pending,
                    SUM(CASE WHEN status = 'completed' THEN 1 ELSE 0 END) AS completed,
                    SUM(CASE WHEN status = 'skipped'   THEN 1 ELSE 0 END) AS skipped
             FROM pending_tasks WHERE user_id = ?",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(TaskStats {
            pending: row.try_get::<Option<i64>, _>("pending")?.unwrap_or(0),
            completed: row.try_get::<Option<i64>, _>("completed")?.unwrap_or(0),
            skipped: row.try_get::<Option<i64>, _>("skipped")?.unwrap_or(0),
        })
    }

    // ---------------------------------------------------------------
    // Pipeline
    // ---------------------------------------------------------------

    /// Advance the most recent request for a broker to a new pipeline stage.
    pub async fn update_pipeline_status(
        &self,
        user_id: i64,
        broker_id: &str,
        status: PipelineStatus,
    ) -> Result<bool, Error> {
        let affected = sqlx::query(
            "UPDATE removal_requests SET pipeline_status = ?
             WHERE id = (SELECT id FROM removal_requests
                         WHERE user_id = ? AND broker_id = ?
                         ORDER BY sent_at DESC, id DESC LIMIT 1)",
        )
        .bind(status.as_str())
        .bind(user_id)
        .bind(broker_id)
        .execute(&self.pool)
        .await?
        .rows_affected();

        Ok(affected > 0)
    }

    /// Counts per pipeline stage, considering only each broker's latest request.
    pub async fn pipeline_stats(
        &self,
        user_id: i64,
    ) -> Result<std::collections::HashMap<PipelineStatus, i64>, Error> {
        let rows = sqlx::query(
            "SELECT pipeline_status, COUNT(*) AS count FROM removal_requests
             WHERE user_id = ?
               AND id IN (SELECT MAX(id) FROM removal_requests
                          WHERE user_id = ? GROUP BY broker_id)
             GROUP BY pipeline_status",
        )
        .bind(user_id)
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok((
                    PipelineStatus::from_db(&row.try_get::<String, _>("pipeline_status")?),
                    row.try_get::<i64, _>("count")?,
                ))
            })
            .collect()
    }

    /// Every form detected in a broker reply, with its current state.
    pub async fn forms_with_status(&self, user_id: i64) -> Result<Vec<FormWithStatus>, Error> {
        let rows = sqlx::query(
            "SELECT br.broker_id,
                    br.broker_name,
                    br.form_url,
                    br.email_subject,
                    br.created_at                AS detected_at,
                    COALESCE(pt.id, 0)           AS task_id,
                    COALESCE(pt.status, '')      AS task_status,
                    COALESCE(rr.pipeline_status, '') AS pipeline_status
             FROM broker_responses br
             LEFT JOIN pending_tasks pt
                    ON pt.user_id = br.user_id
                   AND pt.broker_id = br.broker_id
                   AND pt.task_type IN ('captcha', 'manual_form')
             LEFT JOIN (
                    SELECT broker_id, pipeline_status FROM removal_requests
                    WHERE user_id = ?
                      AND id IN (SELECT MAX(id) FROM removal_requests
                                 WHERE user_id = ? GROUP BY broker_id)
                 ) rr ON rr.broker_id = br.broker_id
             WHERE br.user_id = ? AND br.form_url != ''
             ORDER BY br.created_at DESC, br.id DESC",
        )
        .bind(user_id)
        .bind(user_id)
        .bind(user_id)
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        let mut seen = std::collections::HashSet::new();
        let mut forms = Vec::new();
        for row in rows {
            let broker_id: String = row.try_get("broker_id")?;
            // The join can produce several rows per broker; keep the newest.
            if !seen.insert(broker_id.clone()) {
                continue;
            }

            let task_id: i64 = row.try_get("task_id")?;
            let task_status: String = row.try_get("task_status")?;
            let pipeline_status =
                PipelineStatus::from_db(&row.try_get::<String, _>("pipeline_status")?);

            forms.push(FormWithStatus {
                broker_id,
                broker_name: row.try_get("broker_name")?,
                form_url: row.try_get("form_url")?,
                email_subject: row.try_get("email_subject")?,
                detected_at: row.try_get("detected_at")?,
                status: form_status(task_id, &task_status, pipeline_status),
                task_id,
                pipeline_status,
            });
        }

        Ok(forms)
    }

    pub async fn form_stats(&self, user_id: i64) -> Result<FormStats, Error> {
        let mut stats = FormStats::default();
        for form in self.forms_with_status(user_id).await? {
            match form.status {
                FormStatus::Pending => stats.pending += 1,
                FormStatus::Filled => stats.filled += 1,
                FormStatus::Captcha => stats.captcha += 1,
                FormStatus::Failed => stats.failed += 1,
                FormStatus::Skipped => stats.skipped += 1,
            }
        }
        Ok(stats)
    }
}

/// Resolve a form's state from its task and its broker's pipeline stage.
///
/// A task, when one exists, is more specific than the pipeline stage and so
/// wins.
fn form_status(task_id: i64, task_status: &str, pipeline_status: PipelineStatus) -> FormStatus {
    match task_status {
        "completed" => return FormStatus::Filled,
        "skipped" => return FormStatus::Skipped,
        "pending" if task_id > 0 => return FormStatus::Captcha,
        _ => {}
    }

    match pipeline_status {
        PipelineStatus::FormFilled | PipelineStatus::Confirmed => FormStatus::Filled,
        PipelineStatus::Failed => FormStatus::Failed,
        PipelineStatus::Rejected => FormStatus::Skipped,
        _ => FormStatus::Pending,
    }
}

fn record_from_row(row: SqliteRow) -> Result<Record, Error> {
    Ok(Record {
        id: row.try_get("id")?,
        user_id: row.try_get("user_id")?,
        broker_id: row.try_get("broker_id")?,
        broker_name: row.try_get("broker_name")?,
        email: row.try_get("email")?,
        template: row.try_get("template")?,
        status: Status::from_db(&row.try_get::<String, _>("status")?),
        message_id: row.try_get("message_id")?,
        error: row.try_get("error")?,
        sent_at: row.try_get("sent_at")?,
        created_at: row.try_get("created_at")?,
        pipeline_status: PipelineStatus::from_db(&row.try_get::<String, _>("pipeline_status")?),
    })
}

fn response_from_row(row: SqliteRow) -> Result<BrokerResponse, Error> {
    Ok(BrokerResponse {
        id: row.try_get("id")?,
        user_id: row.try_get("user_id")?,
        broker_id: row.try_get("broker_id")?,
        broker_name: row.try_get("broker_name")?,
        response_type: ResponseType::from_db(&row.try_get::<String, _>("response_type")?),
        email_from: row.try_get("email_from")?,
        email_subject: row.try_get("email_subject")?,
        email_body: row.try_get("email_body")?,
        form_url: row.try_get("form_url")?,
        confirm_url: row.try_get("confirm_url")?,
        confidence: row.try_get("confidence")?,
        needs_review: row.try_get::<i64, _>("needs_review")? != 0,
        received_at: row.try_get("received_at")?,
        processed_at: row.try_get("processed_at")?,
        created_at: row.try_get("created_at")?,
    })
}

fn task_from_row(row: SqliteRow) -> Result<PendingTask, Error> {
    Ok(PendingTask {
        id: row.try_get("id")?,
        user_id: row.try_get("user_id")?,
        broker_id: row.try_get("broker_id")?,
        broker_name: row.try_get("broker_name")?,
        task_type: TaskType::from_db(&row.try_get::<String, _>("task_type")?),
        form_url: row.try_get("form_url")?,
        screenshot_path: row.try_get("screenshot_path")?,
        browser_state: row.try_get("browser_state")?,
        notes: row.try_get("notes")?,
        status: TaskStatus::from_db(&row.try_get::<String, _>("status")?),
        created_at: row.try_get("created_at")?,
        opened_at: row.try_get("opened_at")?,
        completed_at: row.try_get("completed_at")?,
    })
}

#[cfg(test)]
mod tests;
