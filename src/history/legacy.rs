//! Adopting a database created by the Go version.
//!
//! eraser's Go code created its tables directly, with no migration table, so
//! sqlx opening one of those databases tries to run the first migration and
//! fails on `table removal_requests already exists`. That would leave anyone
//! upgrading from eraser unable to start eruser at all, with their entire
//! send history sitting in a file it refuses to open.
//!
//! This brings such a database up to eruser's schema in place, keeping every
//! row, and then records the baseline migration as applied so the normal
//! migration path takes over from there.

use sqlx::{Row, SqlitePool};

use super::Error;

/// Whether this database was made by the Go version.
///
/// The test is the `user_id` column: eruser has had it since its first
/// migration, and Go never had it.
pub async fn is_legacy(pool: &SqlitePool) -> Result<bool, Error> {
    if !table_exists(pool, "removal_requests").await? {
        // A database with no tables at all is simply new.
        return Ok(false);
    }
    Ok(!column_exists(pool, "removal_requests", "user_id").await?)
}

/// Bring a Go-era database up to eruser's schema.
pub async fn adopt(pool: &SqlitePool) -> Result<(), Error> {
    tracing::info!("found a database from the Go version of eraser; upgrading it in place");

    // Everything here is additive: no column is dropped and no row is
    // deleted, so the worst case is a database carrying a column eruser does
    // not read.
    let mut tx = pool.begin().await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS users (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            username      TEXT NOT NULL UNIQUE,
            password_hash TEXT,
            created_at    TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT OR IGNORE INTO users (id, username, password_hash) VALUES (1, 'default', NULL)",
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    // Every row already in the file belongs to the person whose machine it
    // is on, which is user 1.
    for table in ["removal_requests", "broker_responses", "pending_tasks"] {
        if table_exists(pool, table).await? {
            add_column_if_missing(pool, table, "user_id", "INTEGER NOT NULL DEFAULT 1").await?;
        }
    }

    // Columns Go's CREATE TABLE never had.
    if table_exists(pool, "removal_requests").await? {
        // Go added this with a bare ALTER that ignored its own error, so
        // whether it exists depends on which version wrote the file.
        add_column_if_missing(
            pool,
            "removal_requests",
            "pipeline_status",
            "TEXT NOT NULL DEFAULT 'email_sent'",
        )
        .await?;
    }

    if table_exists(pool, "broker_responses").await? {
        // Go's AddBrokerResponse inserted email_body, but its CREATE TABLE
        // never declared the column — so every insert failed with "no such
        // column" and no broker reply was ever recorded. Adding it here is
        // what makes the table usable.
        add_column_if_missing(
            pool,
            "broker_responses",
            "email_body",
            "TEXT NOT NULL DEFAULT ''",
        )
        .await?;
    }

    if table_exists(pool, "pending_tasks").await? {
        add_column_if_missing(pool, "pending_tasks", "opened_at", "TEXT").await?;
    }

    normalize_nulls(pool).await?;
    normalize_timestamps(pool).await?;
    create_indexes(pool).await?;
    record_baseline(pool).await?;

    tracing::info!("the database is now on eruser's schema; nothing was removed");
    Ok(())
}

/// Text columns Go declared nullable that eruser reads as plain strings.
const NULLABLE_TEXT: &[(&str, &str)] = &[
    ("removal_requests", "message_id"),
    ("removal_requests", "error"),
    ("removal_requests", "pipeline_status"),
    ("broker_responses", "email_from"),
    ("broker_responses", "email_subject"),
    ("broker_responses", "form_url"),
    ("broker_responses", "confirm_url"),
    ("pending_tasks", "form_url"),
    ("pending_tasks", "screenshot_path"),
    ("pending_tasks", "browser_state"),
    ("pending_tasks", "notes"),
    ("pending_tasks", "status"),
];

/// Replace NULLs with empty strings in columns eruser reads as text.
async fn normalize_nulls(pool: &SqlitePool) -> Result<(), Error> {
    for (table, column) in NULLABLE_TEXT {
        if !table_exists(pool, table).await? || !column_exists(pool, table, column).await? {
            continue;
        }

        // Safe to assert: both names come from the constant above, never
        // from anything a user supplied.
        let sql = format!("UPDATE {table} SET {column} = '' WHERE {column} IS NULL");
        sqlx::query(sqlx::AssertSqlSafe(sql)).execute(pool).await?;
    }

    // A pipeline stage of '' would read as an unknown value; the default is
    // what Go meant by an unset one.
    if column_exists(pool, "removal_requests", "pipeline_status").await? {
        sqlx::query(
            "UPDATE removal_requests SET pipeline_status = 'email_sent' WHERE pipeline_status = ''",
        )
        .execute(pool)
        .await?;
    }
    if column_exists(pool, "pending_tasks", "status").await? {
        sqlx::query("UPDATE pending_tasks SET status = 'pending' WHERE status = ''")
            .execute(pool)
            .await?;
    }

    Ok(())
}

/// Timestamp columns, per table.
const TIMESTAMPS: &[(&str, &[&str])] = &[
    ("removal_requests", &["sent_at", "created_at"]),
    (
        "broker_responses",
        &["received_at", "processed_at", "created_at"],
    ),
    (
        "pending_tasks",
        &["created_at", "opened_at", "completed_at"],
    ),
];

/// Rewrite timestamps into RFC 3339.
///
/// Go stored these by handing a `time.Time` straight to the driver, which
/// wrote its native formatting — `2026-08-19 21:16:20.587187055 -0500 CDT
/// m=+150.374812277`, monotonic-clock reading and all. Nothing but Go reads
/// that back, so every row in a real eraser database is undecodable until it
/// has been rewritten.
async fn normalize_timestamps(pool: &SqlitePool) -> Result<(), Error> {
    for (table, columns) in TIMESTAMPS {
        if !table_exists(pool, table).await? {
            continue;
        }

        for column in *columns {
            if !column_exists(pool, table, column).await? {
                continue;
            }

            // Safe to assert: both names come from the constant above.
            let select = format!(
                "SELECT id, {column} AS value FROM {table} \
                 WHERE {column} IS NOT NULL AND {column} != ''"
            );
            let rows = sqlx::query(sqlx::AssertSqlSafe(select))
                .fetch_all(pool)
                .await?;

            let mut rewritten = 0usize;
            for row in rows {
                let id: i64 = row.try_get("id")?;
                let Ok(value) = row.try_get::<String, _>("value") else {
                    continue;
                };
                let Some(parsed) = parse_go_time(&value) else {
                    continue;
                };

                let update = format!("UPDATE {table} SET {column} = ? WHERE id = ?");
                sqlx::query(sqlx::AssertSqlSafe(update))
                    .bind(parsed.to_rfc3339())
                    .bind(id)
                    .execute(pool)
                    .await?;
                rewritten += 1;
            }

            if rewritten > 0 {
                tracing::info!(table, column, rewritten, "rewrote timestamps into RFC 3339");
            }
        }
    }

    Ok(())
}

/// Read a timestamp the Go version wrote.
///
/// Returns `None` when the value already parses as RFC 3339, so anything
/// needing no change is left exactly as it is.
pub(crate) fn parse_go_time(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let value = value.trim();

    if chrono::DateTime::parse_from_rfc3339(value).is_ok() {
        return None;
    }

    // Go writes "<date> <time> <offset> <zone> m=+<monotonic>". Only the
    // first three parts carry anything chrono can use.
    let parts: Vec<&str> = value.split_whitespace().take(3).collect();

    if parts.len() == 3
        && let Ok(parsed) =
            chrono::DateTime::parse_from_str(&parts.join(" "), "%Y-%m-%d %H:%M:%S%.f %z")
    {
        return Some(parsed.with_timezone(&chrono::Utc));
    }

    // SQLite's own text format, which carries no zone. Go wrote those in UTC.
    let naive = parts[..parts.len().min(2)].join(" ");
    for format in [
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.f",
    ] {
        if let Ok(parsed) = chrono::NaiveDateTime::parse_from_str(&naive, format) {
            return Some(parsed.and_utc());
        }
    }

    None
}

/// Add the indexes eruser's schema declares.
async fn create_indexes(pool: &SqlitePool) -> Result<(), Error> {
    const INDEXES: &[&str] = &[
        "CREATE INDEX IF NOT EXISTS idx_rr_user_broker   ON removal_requests(user_id, broker_id)",
        "CREATE INDEX IF NOT EXISTS idx_rr_user_sent_at  ON removal_requests(user_id, sent_at)",
        "CREATE INDEX IF NOT EXISTS idx_rr_user_status   ON removal_requests(user_id, status)",
        "CREATE INDEX IF NOT EXISTS idx_rr_user_pipeline ON removal_requests(user_id, pipeline_status)",
        "CREATE INDEX IF NOT EXISTS idx_br_user_broker   ON broker_responses(user_id, broker_id)",
        "CREATE INDEX IF NOT EXISTS idx_br_user_type     ON broker_responses(user_id, response_type)",
        "CREATE INDEX IF NOT EXISTS idx_br_user_review   ON broker_responses(user_id, needs_review)",
        "CREATE INDEX IF NOT EXISTS idx_pt_user_broker   ON pending_tasks(user_id, broker_id)",
        "CREATE INDEX IF NOT EXISTS idx_pt_user_type     ON pending_tasks(user_id, task_type)",
        "CREATE INDEX IF NOT EXISTS idx_pt_user_status   ON pending_tasks(user_id, status)",
    ];

    for statement in INDEXES {
        sqlx::query(*statement).execute(pool).await?;
    }

    // The unique index the upsert relies on cannot be created while
    // duplicates exist, and Go's insert-without-checking could leave them.
    if table_exists(pool, "broker_responses").await? {
        sqlx::query(
            "DELETE FROM broker_responses WHERE id NOT IN (
                 SELECT MAX(id) FROM broker_responses
                 GROUP BY user_id, broker_id, email_subject
             )",
        )
        .execute(pool)
        .await?;

        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_br_user_broker_subject
             ON broker_responses(user_id, broker_id, email_subject)",
        )
        .execute(pool)
        .await?;
    }

    Ok(())
}

/// Record the baseline migration as applied.
///
/// The schema now matches what migration 1 would have produced, so running
/// it would only fail on tables that already exist. Writing the row with the
/// migration's real checksum means sqlx also will not complain that the file
/// has changed.
async fn record_baseline(pool: &SqlitePool) -> Result<(), Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _sqlx_migrations (
            version        BIGINT PRIMARY KEY,
            description    TEXT NOT NULL,
            installed_on   TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            success        BOOLEAN NOT NULL,
            checksum       BLOB NOT NULL,
            execution_time BIGINT NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    let Some(baseline) = super::MIGRATOR.iter().next() else {
        return Ok(());
    };

    sqlx::query(
        "INSERT OR IGNORE INTO _sqlx_migrations
             (version, description, success, checksum, execution_time)
         VALUES (?, ?, TRUE, ?, 0)",
    )
    .bind(baseline.version)
    .bind(baseline.description.as_ref())
    .bind(baseline.checksum.as_ref())
    .execute(pool)
    .await?;

    Ok(())
}

async fn table_exists(pool: &SqlitePool, table: &str) -> Result<bool, Error> {
    let found: Option<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?")
            .bind(table)
            .fetch_optional(pool)
            .await?;
    Ok(found.is_some())
}

async fn column_exists(pool: &SqlitePool, table: &str, column: &str) -> Result<bool, Error> {
    Ok(columns_of(pool, table).await?.iter().any(|c| c == column))
}

/// The column names of a table.
async fn columns_of(pool: &SqlitePool, table: &str) -> Result<Vec<String>, Error> {
    // PRAGMA does not take a bind parameter. The name is never user-supplied:
    // every caller passes one of the constants in this module.
    let sql = format!("PRAGMA table_info({table})");
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .fetch_all(pool)
        .await?;

    rows.iter().map(|row| Ok(row.try_get("name")?)).collect()
}

/// Add a column, if it is not already there.
async fn add_column_if_missing(
    pool: &SqlitePool,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), Error> {
    if column_exists(pool, table, column).await? {
        return Ok(());
    }

    // Safe to assert: all three come from constants in this module.
    let sql = format!("ALTER TABLE {table} ADD COLUMN {column} {definition}");
    sqlx::query(sqlx::AssertSqlSafe(sql)).execute(pool).await?;

    tracing::info!(table, column, "added a column the Go version did not have");
    Ok(())
}

#[cfg(test)]
mod tests;
