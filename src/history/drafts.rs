//! Drafted replies, and what becomes of them.
//!
//! The drafter writes; this layer remembers. A draft is keyed to the broker
//! reply it answers, so re-running the monitor never writes a second one,
//! and it rides the task list as a `draft_reply` task — the same queue a
//! captcha uses, because it is the same kind of work: something a model
//! prepared that a person has to decide on.

use chrono::{DateTime, Utc};

use super::types::{DEFAULT_USER_ID, DraftStatus, ReplyType, TaskType};
use super::{Error, NewPendingTask, Store};

/// A drafted reply, as read back.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ReplyDraft {
    pub id: i64,
    pub user_id: i64,
    pub broker_id: String,
    pub broker_name: String,
    pub reply_type: ReplyType,
    pub in_reply_to: String,
    pub subject: String,
    pub body: String,
    pub model: String,
    pub status: DraftStatus,
    pub validation: String,
    pub created_at: Option<DateTime<Utc>>,
    pub sent_at: Option<DateTime<Utc>>,
}

/// A new draft to keep.
#[derive(Debug, Clone)]
pub struct NewReplyDraft {
    pub user_id: i64,
    pub broker_id: String,
    pub broker_name: String,
    pub reply_type: ReplyType,
    pub in_reply_to: String,
    pub subject: String,
    pub body: String,
    pub model: String,
    pub validation: String,
}

impl Default for NewReplyDraft {
    fn default() -> Self {
        Self {
            user_id: DEFAULT_USER_ID,
            broker_id: String::new(),
            broker_name: String::new(),
            reply_type: ReplyType::MissingInfo,
            in_reply_to: String::new(),
            subject: String::new(),
            body: String::new(),
            model: String::new(),
            validation: "{}".to_string(),
        }
    }
}

fn row_to_draft(row: &sqlx::sqlite::SqliteRow) -> Result<ReplyDraft, Error> {
    use sqlx::Row;

    Ok(ReplyDraft {
        id: row.try_get("id")?,
        user_id: row.try_get("user_id")?,
        broker_id: row.try_get("broker_id")?,
        broker_name: row.try_get("broker_name")?,
        reply_type: ReplyType::from_db(&row.try_get::<String, _>("reply_type")?),
        in_reply_to: row.try_get("in_reply_to")?,
        subject: row.try_get("subject")?,
        body: row.try_get("body")?,
        model: row.try_get("model")?,
        status: DraftStatus::from_db(&row.try_get::<String, _>("status")?),
        validation: row.try_get("validation")?,
        created_at: row
            .try_get::<Option<String>, _>("created_at")?
            .and_then(|text| DateTime::parse_from_rfc3339(&text).ok())
            .map(|time| time.with_timezone(&Utc)),
        sent_at: row
            .try_get::<Option<String>, _>("sent_at")?
            .and_then(|text| DateTime::parse_from_rfc3339(&text).ok())
            .map(|time| time.with_timezone(&Utc)),
    })
}

impl Store {
    /// Keep a draft, replacing one that already answers the same reply.
    ///
    /// A re-run of the monitor re-reads the same mailbox; without the
    /// upsert, every run would leave another copy of the same draft.
    pub async fn upsert_draft(&self, draft: &NewReplyDraft) -> Result<i64, Error> {
        let id = sqlx::query(
            "INSERT INTO reply_drafts
                 (user_id, broker_id, broker_name, reply_type, in_reply_to,
                  subject, body, model, validation)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(user_id, broker_id, in_reply_to, reply_type) DO UPDATE SET
                 broker_name = excluded.broker_name,
                 subject = excluded.subject,
                 body = excluded.body,
                 model = excluded.model,
                 validation = excluded.validation,
                 status = 'draft'",
        )
        .bind(draft.user_id)
        .bind(&draft.broker_id)
        .bind(&draft.broker_name)
        .bind(draft.reply_type.as_str())
        .bind(&draft.in_reply_to)
        .bind(&draft.subject)
        .bind(&draft.body)
        .bind(&draft.model)
        .bind(&draft.validation)
        .execute(&self.pool)
        .await?
        .last_insert_rowid();

        Ok(id)
    }

    /// Drafts waiting for someone to read them, newest first.
    pub async fn pending_drafts(&self, user_id: i64) -> Result<Vec<ReplyDraft>, Error> {
        let rows = sqlx::query(
            "SELECT * FROM reply_drafts
             WHERE user_id = ? AND status = 'draft'
             ORDER BY created_at DESC, id DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(row_to_draft).collect()
    }

    /// One draft, for the preview page.
    pub async fn draft(&self, user_id: i64, draft_id: i64) -> Result<ReplyDraft, Error> {
        let row = sqlx::query("SELECT * FROM reply_drafts WHERE user_id = ? AND id = ?")
            .bind(user_id)
            .bind(draft_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(Error::UnknownTask)?;

        row_to_draft(&row)
    }

    /// Whether a reply already has a draft, so the monitor does not draft
    /// the same email twice.
    pub async fn has_draft(
        &self,
        user_id: i64,
        broker_id: &str,
        in_reply_to: &str,
        reply_type: ReplyType,
    ) -> Result<bool, Error> {
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM reply_drafts
             WHERE user_id = ? AND broker_id = ? AND in_reply_to = ? AND reply_type = ?",
        )
        .bind(user_id)
        .bind(broker_id)
        .bind(in_reply_to)
        .bind(reply_type.as_str())
        .fetch_optional(&self.pool)
        .await?;

        Ok(found.is_some())
    }

    /// Mark a draft as gone out. Called after the send path accepted it.
    pub async fn mark_draft_sent(&self, user_id: i64, draft_id: i64) -> Result<(), Error> {
        sqlx::query(
            "UPDATE reply_drafts SET status = 'sent', sent_at = ? WHERE user_id = ? AND id = ?",
        )
        .bind(Utc::now())
        .bind(user_id)
        .bind(draft_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// A person read the draft and did not want it.
    pub async fn discard_draft(&self, user_id: i64, draft_id: i64) -> Result<(), Error> {
        sqlx::query("UPDATE reply_drafts SET status = 'discarded' WHERE user_id = ? AND id = ?")
            .bind(user_id)
            .bind(draft_id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    /// Put a draft on the task list. Idempotent: the task for a given draft
    /// is only queued once, however many runs notice it.
    pub async fn queue_draft_task(&self, draft: &ReplyDraft) -> Result<(), Error> {
        let already: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM pending_tasks
             WHERE user_id = ? AND broker_id = ? AND task_type = 'draft_reply'
               AND status = 'pending' AND notes LIKE ? || '%'",
        )
        .bind(draft.user_id)
        .bind(&draft.broker_id)
        .bind(format!("Draft #{}", draft.id))
        .fetch_optional(&self.pool)
        .await?;

        if already.is_some() {
            return Ok(());
        }

        self.add_task(&NewPendingTask {
            user_id: draft.user_id,
            broker_id: draft.broker_id.clone(),
            broker_name: draft.broker_name.clone(),
            task_type: TaskType::DraftReply,
            form_url: String::new(),
            screenshot_path: String::new(),
            browser_state: draft.id.to_string(),
            notes: format!(
                "Draft #{} — a reply to \"{}\" is written and waiting to be sent",
                draft.id, draft.in_reply_to
            ),
        })
        .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests;
