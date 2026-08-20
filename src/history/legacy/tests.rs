//! Tests for adopting a Go-era database.
//!
//! The fixture below is the exact schema `internal/history/history.go`
//! creates, copied from the CREATE TABLE statements in its `migrate()` —
//! including the missing `email_body` column, which is the bug that kept the
//! table empty.

use super::*;

use crate::history::{DEFAULT_USER_ID, ResponseFilter, Store};

/// The schema Go's migrate() produces, verbatim.
const GO_SCHEMA: &str = r"
CREATE TABLE removal_requests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    broker_id TEXT NOT NULL,
    broker_name TEXT NOT NULL,
    email TEXT NOT NULL,
    template TEXT NOT NULL,
    status TEXT NOT NULL,
    message_id TEXT,
    error TEXT,
    sent_at DATETIME,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    pipeline_status TEXT DEFAULT 'email_sent'
);
CREATE INDEX idx_broker_id ON removal_requests(broker_id);
CREATE INDEX idx_sent_at ON removal_requests(sent_at);
CREATE INDEX idx_status ON removal_requests(status);
CREATE INDEX idx_pipeline_status ON removal_requests(pipeline_status);

CREATE TABLE broker_responses (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    broker_id TEXT NOT NULL,
    broker_name TEXT NOT NULL,
    response_type TEXT NOT NULL,
    email_from TEXT,
    email_subject TEXT,
    form_url TEXT,
    confirm_url TEXT,
    confidence REAL,
    needs_review INTEGER DEFAULT 0,
    received_at DATETIME,
    processed_at DATETIME,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX idx_br_broker_id ON broker_responses(broker_id);

CREATE TABLE pending_tasks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    broker_id TEXT NOT NULL,
    broker_name TEXT NOT NULL,
    task_type TEXT NOT NULL,
    form_url TEXT,
    screenshot_path TEXT,
    browser_state TEXT,
    notes TEXT,
    status TEXT DEFAULT 'pending',
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    opened_at DATETIME,
    completed_at DATETIME
);
CREATE INDEX idx_pt_broker_id ON pending_tasks(broker_id);
";

/// A pool holding a database exactly as the Go version would have left it.
async fn go_database() -> SqlitePool {
    let pool = SqlitePool::connect(":memory:").await.expect("a pool");

    for statement in GO_SCHEMA.split(';').filter(|s| !s.trim().is_empty()) {
        sqlx::query(sqlx::AssertSqlSafe(statement.to_string()))
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("fixture statement failed: {e}\n{statement}"));
    }

    pool
}

/// Add a request the way Go's Add() would, NULLs and all.
async fn insert_go_request(pool: &SqlitePool, broker_id: &str, status: &str) {
    sqlx::query(
        "INSERT INTO removal_requests
            (broker_id, broker_name, email, template, status, message_id, error, sent_at)
         VALUES (?, ?, ?, 'generic', ?, NULL, NULL, '2026-08-01 12:00:00')",
    )
    .bind(broker_id)
    .bind(format!("Broker {broker_id}"))
    .bind(format!("privacy@{broker_id}.example"))
    .bind(status)
    .execute(pool)
    .await
    .expect("the request should insert");
}

// -------------------------------------------------------------------
// Detection
// -------------------------------------------------------------------

#[tokio::test]
async fn a_go_database_is_recognised() {
    let pool = go_database().await;
    assert!(is_legacy(&pool).await.unwrap());
}

#[tokio::test]
async fn an_empty_database_is_not_treated_as_legacy() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    assert!(!is_legacy(&pool).await.unwrap());
}

#[tokio::test]
async fn an_eruser_database_is_not_treated_as_legacy() {
    let store = Store::open_in_memory().await.unwrap();
    assert!(!is_legacy(store.pool()).await.unwrap());
}

// -------------------------------------------------------------------
// Adoption
// -------------------------------------------------------------------

/// The whole point: someone upgrading from eraser must not lose their
/// history, and must not be locked out of the file it lives in.
#[tokio::test]
async fn adoption_keeps_every_row() {
    let pool = go_database().await;
    for broker in ["acme", "globex", "initech"] {
        insert_go_request(&pool, broker, "sent").await;
    }
    insert_go_request(&pool, "failed-one", "failed").await;

    adopt(&pool).await.unwrap();

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM removal_requests")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 4, "no row should have been lost");
}

#[tokio::test]
async fn adopted_rows_belong_to_the_default_user() {
    let pool = go_database().await;
    insert_go_request(&pool, "acme", "sent").await;

    adopt(&pool).await.unwrap();

    let owner: i64 = sqlx::query_scalar("SELECT user_id FROM removal_requests LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(owner, DEFAULT_USER_ID);
}

/// Go's AddBrokerResponse inserted a column its CREATE TABLE never declared,
/// so every insert failed with "no such column" and not one broker reply was
/// ever recorded. Adding it is what makes the table usable.
#[tokio::test]
async fn the_column_go_forgot_is_added() {
    let pool = go_database().await;
    assert!(
        !column_exists(&pool, "broker_responses", "email_body")
            .await
            .unwrap(),
        "the fixture should reproduce the missing column"
    );

    adopt(&pool).await.unwrap();

    assert!(
        column_exists(&pool, "broker_responses", "email_body")
            .await
            .unwrap()
    );
}

/// Go left these NULL; eruser reads them as plain strings.
#[tokio::test]
async fn nulls_are_replaced_with_empty_strings() {
    let pool = go_database().await;
    insert_go_request(&pool, "acme", "sent").await;

    adopt(&pool).await.unwrap();

    let (message_id, error): (String, String) =
        sqlx::query_as("SELECT message_id, error FROM removal_requests LIMIT 1")
            .fetch_one(&pool)
            .await
            .expect("the columns should read as text, not NULL");
    assert_eq!(message_id, "");
    assert_eq!(error, "");
}

/// Go's insert-without-checking could leave two rows for the same reply,
/// which would stop the unique index the upsert relies on from being built.
#[tokio::test]
async fn duplicate_replies_are_collapsed_so_the_unique_index_can_be_built() {
    let pool = go_database().await;

    for _ in 0..3 {
        sqlx::query(
            "INSERT INTO broker_responses
                (broker_id, broker_name, response_type, email_subject)
             VALUES ('acme', 'Acme', 'pending', 'Re: your request')",
        )
        .execute(&pool)
        .await
        .unwrap();
    }

    adopt(&pool).await.unwrap();

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM broker_responses")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "the newest of the duplicates should remain");

    // And the index now exists, so upserts work from here on.
    sqlx::query(
        "INSERT INTO broker_responses (user_id, broker_id, broker_name, response_type, email_subject)
         VALUES (1, 'acme', 'Acme', 'success', 'Re: your request')
         ON CONFLICT(user_id, broker_id, email_subject) DO UPDATE SET response_type = 'success'",
    )
    .execute(&pool)
    .await
    .expect("the upsert should work after adoption");
}

#[tokio::test]
async fn adoption_can_be_run_twice_without_complaint() {
    let pool = go_database().await;
    insert_go_request(&pool, "acme", "sent").await;

    adopt(&pool).await.unwrap();
    adopt(&pool).await.expect("a second run should be harmless");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM removal_requests")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

/// After adoption the normal migration path has to run cleanly, or the
/// database is still unopenable — just for a different reason.
#[tokio::test]
async fn the_baseline_migration_is_recorded_so_migrations_run_clean() {
    let pool = go_database().await;
    insert_go_request(&pool, "acme", "sent").await;

    adopt(&pool).await.unwrap();

    crate::history::MIGRATOR
        .run(&pool)
        .await
        .expect("migrations should be a no-op after adoption");
}

// -------------------------------------------------------------------
// End to end
// -------------------------------------------------------------------

/// The real thing: point Store::open at a file the Go version wrote and use
/// it as though it had always been eruser's.
#[tokio::test]
async fn a_go_database_opens_and_reads_back_its_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");

    // Build a Go-era file on disk.
    {
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(options).await.unwrap();

        for statement in GO_SCHEMA.split(';').filter(|s| !s.trim().is_empty()) {
            sqlx::query(sqlx::AssertSqlSafe(statement.to_string()))
                .execute(&pool)
                .await
                .unwrap();
        }
        for broker in ["acme", "globex"] {
            insert_go_request(&pool, broker, "sent").await;
        }
        insert_go_request(&pool, "initech", "failed").await;
        pool.close().await;
    }

    // Open it as eruser.
    let store = Store::open(&path).await.expect("a Go database should open");

    let stats = store.stats(DEFAULT_USER_ID).await.unwrap();
    assert_eq!(stats.total, 3);
    assert_eq!(stats.sent, 2);
    assert_eq!(stats.failed, 1);

    let recent = store.recent_requests(DEFAULT_USER_ID, 10).await.unwrap();
    assert_eq!(recent.len(), 3);
    assert!(recent.iter().any(|r| r.broker_id == "acme"));

    // Writing works too, and lands alongside the imported rows.
    store
        .add_record(&crate::history::NewRecord::sent(
            "newone",
            "New One",
            "privacy@newone.example",
            "gdpr",
            "<id@example.com>",
        ))
        .await
        .unwrap();
    assert_eq!(store.stats(DEFAULT_USER_ID).await.unwrap().total, 4);

    // And the reply table Go could never write to now accepts a row.
    store
        .upsert_broker_response(&crate::history::NewBrokerResponse {
            broker_id: "acme".into(),
            broker_name: "Acme".into(),
            email_subject: "Re: your request".into(),
            email_body: "We have removed your data.".into(),
            ..Default::default()
        })
        .await
        .expect("broker replies should be storable after adoption");

    assert_eq!(
        store
            .broker_responses(DEFAULT_USER_ID, ResponseFilter::default())
            .await
            .unwrap()
            .len(),
        1
    );

    store.close().await;

    // Reopening is a no-op, not a second adoption.
    let reopened = Store::open(&path).await.expect("reopening should work");
    assert_eq!(reopened.stats(DEFAULT_USER_ID).await.unwrap().total, 4);
}

/// A pipeline stage Go left NULL should read as the default rather than as
/// an unknown value.
#[tokio::test]
async fn an_unset_pipeline_stage_reads_as_email_sent() {
    let pool = go_database().await;
    sqlx::query(
        "INSERT INTO removal_requests
            (broker_id, broker_name, email, template, status, pipeline_status)
         VALUES ('acme', 'Acme', 'a@b.example', 'generic', 'sent', NULL)",
    )
    .execute(&pool)
    .await
    .unwrap();

    adopt(&pool).await.unwrap();

    let stage: String = sqlx::query_scalar("SELECT pipeline_status FROM removal_requests LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stage, "email_sent");
}

#[tokio::test]
async fn a_status_go_left_unset_reads_as_pending() {
    let pool = go_database().await;
    sqlx::query(
        "INSERT INTO pending_tasks (broker_id, broker_name, task_type, status)
         VALUES ('acme', 'Acme', 'captcha', NULL)",
    )
    .execute(&pool)
    .await
    .unwrap();

    adopt(&pool).await.unwrap();

    let status: String = sqlx::query_scalar("SELECT status FROM pending_tasks LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "pending");
}

// -------------------------------------------------------------------
// Helpers
// -------------------------------------------------------------------

#[tokio::test]
async fn adding_a_column_that_is_already_there_is_harmless() {
    let pool = go_database().await;

    add_column_if_missing(
        &pool,
        "removal_requests",
        "user_id",
        "INTEGER NOT NULL DEFAULT 1",
    )
    .await
    .unwrap();
    add_column_if_missing(
        &pool,
        "removal_requests",
        "user_id",
        "INTEGER NOT NULL DEFAULT 1",
    )
    .await
    .expect("the second call should do nothing");

    let user_id_columns = columns_of(&pool, "removal_requests")
        .await
        .unwrap()
        .iter()
        .filter(|c| *c == "user_id")
        .count();
    assert_eq!(user_id_columns, 1);
}

#[tokio::test]
async fn missing_tables_and_columns_are_reported_as_missing() {
    let pool = go_database().await;

    assert!(table_exists(&pool, "removal_requests").await.unwrap());
    assert!(!table_exists(&pool, "nothing_here").await.unwrap());
    assert!(
        column_exists(&pool, "removal_requests", "broker_id")
            .await
            .unwrap()
    );
    assert!(
        !column_exists(&pool, "removal_requests", "nothing_here")
            .await
            .unwrap()
    );
}

// -------------------------------------------------------------------
// Timestamps
// -------------------------------------------------------------------

/// Go handed a `time.Time` straight to the driver, which wrote its native
/// formatting — monotonic-clock reading and all. This is the literal shape
/// found in a real eraser database.
#[test]
fn a_go_native_timestamp_is_understood() {
    let parsed = parse_go_time("2026-08-19 21:16:20.587187055 -0500 CDT m=+150.374812277")
        .expect("Go's own format must parse");

    assert_eq!(parsed.format("%Y-%m-%d").to_string(), "2026-08-20");
    // 21:16 at -0500 is 02:16 the next day in UTC.
    assert_eq!(parsed.format("%H:%M").to_string(), "02:16");
}

#[test]
fn go_timestamps_without_a_monotonic_reading_are_understood() {
    assert!(parse_go_time("2026-08-19 21:16:20.587187055 -0500 CDT").is_some());
    assert!(parse_go_time("2026-08-19 21:16:20 +0000 UTC").is_some());
}

/// SQLite's own text format has no zone; Go wrote those in UTC.
#[test]
fn a_plain_sqlite_timestamp_is_read_as_utc() {
    let parsed = parse_go_time("2026-08-19 21:16:20").expect("SQLite's format must parse");
    assert_eq!(parsed.to_rfc3339(), "2026-08-19T21:16:20+00:00");
}

/// A value that already parses needs no rewriting, and rewriting it would
/// only risk changing it.
#[test]
fn a_value_that_is_already_correct_is_left_alone() {
    assert!(parse_go_time("2026-08-19T21:16:20+00:00").is_none());
    assert!(parse_go_time("2026-08-19T21:16:20Z").is_none());
}

#[test]
fn nonsense_is_left_alone_rather_than_guessed_at() {
    assert!(parse_go_time("").is_none());
    assert!(parse_go_time("not a time").is_none());
    assert!(parse_go_time("yesterday").is_none());
}

/// Without this the rows are present but every read fails to decode, which
/// is no better than not opening the file at all.
#[tokio::test]
async fn adoption_makes_go_timestamps_readable() {
    let pool = go_database().await;
    sqlx::query(
        "INSERT INTO removal_requests
            (broker_id, broker_name, email, template, status, sent_at, created_at)
         VALUES ('acme', 'Acme', 'a@b.example', 'generic', 'sent',
                 '2026-08-19 21:16:20.587187055 -0500 CDT m=+150.374812277',
                 '2026-08-19 21:16:20.587187055 -0500 CDT m=+150.374812277')",
    )
    .execute(&pool)
    .await
    .unwrap();

    adopt(&pool).await.unwrap();

    let sent_at: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT sent_at FROM removal_requests LIMIT 1")
            .fetch_one(&pool)
            .await
            .expect("the timestamp should decode after adoption");

    assert_eq!(sent_at.format("%Y-%m-%d").to_string(), "2026-08-20");
}

/// Running adoption again must not shift a timestamp that was already
/// rewritten.
#[tokio::test]
async fn rewriting_a_timestamp_twice_does_not_move_it() {
    let pool = go_database().await;
    sqlx::query(
        "INSERT INTO removal_requests
            (broker_id, broker_name, email, template, status, sent_at)
         VALUES ('acme', 'Acme', 'a@b.example', 'generic', 'sent',
                 '2026-08-19 21:16:20.587187055 -0500 CDT m=+150.374812277')",
    )
    .execute(&pool)
    .await
    .unwrap();

    adopt(&pool).await.unwrap();
    let once: String = sqlx::query_scalar("SELECT sent_at FROM removal_requests LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();

    adopt(&pool).await.unwrap();
    let twice: String = sqlx::query_scalar("SELECT sent_at FROM removal_requests LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(once, twice);
}
