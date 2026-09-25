//! Letter overrides: wording a person has changed.
//!
//! The three request letters are embedded in the binary. When someone edits
//! one from the web interface, the edited subject and body are stored here
//! and win over the shipped copy until the row is deleted.
//!
//! Overrides, not copies: an absent row means "send the shipped letter", so
//! a wording fix in a release still reaches anyone who has not gone out of
//! their way to change it. Reverting is a delete.

use chrono::{DateTime, Utc};

use super::{Error, Store};

/// One stored letter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LetterOverride {
    pub user_id: i64,
    /// `generic`, `ccpa`, or `gdpr` — the embedded template this replaces.
    pub name: String,
    /// Fixed text on one line: a subject that spans lines is header
    /// injection, so newlines are folded away before anything is stored.
    pub subject: String,
    pub body: String,
    pub updated_at: Option<DateTime<Utc>>,
}

fn row_to_override(row: &sqlx::sqlite::SqliteRow) -> Result<LetterOverride, Error> {
    use sqlx::Row;

    let updated_at = row
        .try_get::<Option<String>, _>("updated_at")?
        .and_then(|text| DateTime::parse_from_rfc3339(&text).ok())
        .map(|time| time.with_timezone(&Utc));

    Ok(LetterOverride {
        user_id: row.try_get("user_id")?,
        name: row.try_get("name")?,
        subject: row.try_get("subject")?,
        body: row.try_get("body")?,
        updated_at,
    })
}

impl Store {
    /// Every letter this person has edited, any order — there are only three.
    pub async fn letter_overrides(&self, user_id: i64) -> Result<Vec<LetterOverride>, Error> {
        let rows = sqlx::query("SELECT * FROM letter_overrides WHERE user_id = ?")
            .bind(user_id)
            .fetch_all(&self.pool)
            .await?;

        rows.iter().map(row_to_override).collect()
    }

    /// One stored letter, or `None` when the shipped copy still stands.
    pub async fn letter_override(
        &self,
        user_id: i64,
        name: &str,
    ) -> Result<Option<LetterOverride>, Error> {
        let row = sqlx::query("SELECT * FROM letter_overrides WHERE user_id = ? AND name = ?")
            .bind(user_id)
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;

        row.as_ref().map(row_to_override).transpose()
    }

    /// Store an edit, replacing any previous one for the same letter.
    ///
    /// The subject is flattened to a single line on the way in, so what
    /// reaches the database is safe to put on the wire however it is sent.
    pub async fn save_letter_override(
        &self,
        user_id: i64,
        name: &str,
        subject: &str,
        body: &str,
    ) -> Result<(), Error> {
        // A subject line is one line: fold the rest onto it with spaces. The
        // body is free text and keeps its shape.
        let subject: String = subject.split_whitespace().collect::<Vec<_>>().join(" ");

        sqlx::query(
            "INSERT INTO letter_overrides (user_id, name, subject, body, updated_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(user_id, name) DO UPDATE SET
                 subject = excluded.subject,
                 body = excluded.body,
                 updated_at = excluded.updated_at",
        )
        .bind(user_id)
        .bind(name)
        .bind(&subject)
        .bind(body)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Forget an edit. The shipped letter goes back into use. Returns false
    /// when there was nothing stored, which the caller may treat as done.
    pub async fn delete_letter_override(&self, user_id: i64, name: &str) -> Result<bool, Error> {
        let affected = sqlx::query("DELETE FROM letter_overrides WHERE user_id = ? AND name = ?")
            .bind(user_id)
            .bind(name)
            .execute(&self.pool)
            .await?
            .rows_affected();

        Ok(affected > 0)
    }
}
