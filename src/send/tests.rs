use super::*;

use crate::broker::Broker;
use crate::email::tests::RecordingSender;
use crate::history::{DEFAULT_USER_ID, ResponseFilter, Status};

fn broker(id: &str) -> Broker {
    Broker {
        id: id.to_string(),
        name: format!("Broker {id}"),
        email: format!("privacy@{id}.example"),
        website: String::new(),
        opt_out_url: String::new(),
        region: "us".to_string(),
        category: "marketing".to_string(),
        notes: String::new(),
        requires_id: false,
        tags: Vec::new(),
    }
}

fn profile() -> Profile {
    Profile {
        first_name: "Jane".into(),
        last_name: "Doe".into(),
        email: "jane@example.com".into(),
        ..Default::default()
    }
}

fn options() -> SendOptions {
    SendOptions {
        from: "jane@example.com".into(),
        // Tests must not wait on real delays.
        rate_limit: Duration::ZERO,
        ..Default::default()
    }
}

/// Collect every Progress event a run emits.
async fn run_with(
    brokers: Vec<Broker>,
    sender: Arc<dyn Sender>,
    store: Option<Store>,
    options: SendOptions,
    cancel: &CancellationToken,
) -> (Summary, Vec<Progress>) {
    let job = SendJob {
        brokers,
        profile: profile(),
        engine: Arc::new(Engine::new().unwrap()),
        sender,
        store,
        options,
    };

    let mut events = Vec::new();
    let summary = job.run(cancel, |event| events.push(event)).await;
    (summary, events)
}

fn outcomes(events: &[Progress]) -> Vec<Outcome> {
    events
        .iter()
        .filter_map(|event| match event {
            Progress::Broker { outcome, .. } => Some(outcome.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn every_broker_gets_one_message() {
    let sender = Arc::new(RecordingSender::default());
    let (summary, _) = run_with(
        vec![broker("a"), broker("b"), broker("c")],
        sender.clone(),
        None,
        options(),
        &CancellationToken::new(),
    )
    .await;

    assert_eq!(summary.sent, 3);
    assert_eq!(summary.failed, 0);
    let sent = sender.sent();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[0].to, "privacy@a.example");
    assert_eq!(sent[0].from, "jane@example.com");
    assert!(sent[0].body.contains("Jane Doe"));
    assert!(sent[0].body.contains("Broker a"));
}

#[tokio::test]
async fn progress_brackets_the_run_and_numbers_each_broker() {
    let (_, events) = run_with(
        vec![broker("a"), broker("b")],
        Arc::new(RecordingSender::default()),
        None,
        options(),
        &CancellationToken::new(),
    )
    .await;

    assert_eq!(events.first(), Some(&Progress::Started { total: 2 }));
    assert!(matches!(events.last(), Some(Progress::Finished(_))));

    let indices: Vec<usize> = events
        .iter()
        .filter_map(|event| match event {
            Progress::Broker { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(indices, [1, 2], "indices should be 1-based and in order");
}

/// A bad address in a 750-entry community database must not stop the run.
#[tokio::test]
async fn one_failure_does_not_abort_the_others() {
    let sender = Arc::new(RecordingSender::failing_for(&["privacy@b.example"]));
    let (summary, events) = run_with(
        vec![broker("a"), broker("b"), broker("c")],
        sender.clone(),
        None,
        options(),
        &CancellationToken::new(),
    )
    .await;

    assert_eq!(summary.sent, 2);
    assert_eq!(summary.failed, 1);
    assert_eq!(sender.sent().len(), 2);
    assert!(matches!(outcomes(&events)[1], Outcome::Failed { .. }));
}

#[tokio::test]
async fn a_malformed_broker_address_fails_without_sending() {
    let mut bad = broker("bad");
    bad.email = "not-an-address".into();

    let sender = Arc::new(RecordingSender::default());
    let (summary, events) = run_with(
        vec![bad, broker("good")],
        sender.clone(),
        None,
        options(),
        &CancellationToken::new(),
    )
    .await;

    assert_eq!(summary.failed, 1);
    assert_eq!(summary.sent, 1);
    assert_eq!(sender.sent().len(), 1);
    match &outcomes(&events)[0] {
        Outcome::Failed { error } => assert!(error.contains("not-an-address"), "{error}"),
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[tokio::test]
async fn an_unknown_template_fails_every_broker_without_sending() {
    let sender = Arc::new(RecordingSender::default());
    let (summary, _) = run_with(
        vec![broker("a"), broker("b")],
        sender.clone(),
        None,
        SendOptions {
            template: "nonexistent".into(),
            ..options()
        },
        &CancellationToken::new(),
    )
    .await;

    assert_eq!(summary.failed, 2);
    assert!(sender.sent().is_empty());
}

#[tokio::test]
async fn results_are_recorded_in_history() {
    let store = Store::open_in_memory().await.unwrap();
    let sender = Arc::new(RecordingSender::failing_for(&["privacy@b.example"]));

    run_with(
        vec![broker("a"), broker("b")],
        sender,
        Some(store.clone()),
        options(),
        &CancellationToken::new(),
    )
    .await;

    let stats = store.stats(DEFAULT_USER_ID).await.unwrap();
    assert_eq!(stats.sent, 1);
    assert_eq!(stats.failed, 1);

    let sent = store
        .last_request_for_broker(DEFAULT_USER_ID, "a")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sent.status, Status::Sent);
    assert!(
        sent.message_id.starts_with('<'),
        "a real Message-ID should be stored"
    );
    assert_eq!(sent.template, "generic");

    let failed = store
        .last_request_for_broker(DEFAULT_USER_ID, "b")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed.status, Status::Failed);
    assert!(!failed.error.is_empty());
}

#[tokio::test]
async fn records_are_written_for_the_configured_user() {
    let store = Store::open_in_memory().await.unwrap();
    sqlx::query("INSERT INTO users (id, username) VALUES (2, 'second')")
        .execute(store.pool())
        .await
        .unwrap();

    run_with(
        vec![broker("a")],
        Arc::new(RecordingSender::default()),
        Some(store.clone()),
        SendOptions {
            user_id: 2,
            ..options()
        },
        &CancellationToken::new(),
    )
    .await;

    assert_eq!(store.stats(DEFAULT_USER_ID).await.unwrap().total, 0);
    assert_eq!(store.stats(2).await.unwrap().sent, 1);
}

/// A preview should leave no trace in history.
#[tokio::test]
async fn a_dry_run_sends_nothing_and_records_nothing() {
    let store = Store::open_in_memory().await.unwrap();
    let (summary, _) = run_with(
        vec![broker("a"), broker("b")],
        Arc::new(crate::email::DryRunSender),
        None,
        options(),
        &CancellationToken::new(),
    )
    .await;

    assert_eq!(summary.sent, 2);
    assert_eq!(store.stats(DEFAULT_USER_ID).await.unwrap().total, 0);
    assert!(
        store
            .broker_responses(DEFAULT_USER_ID, ResponseFilter::default())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn the_daily_limit_stops_sending_but_still_reports_every_broker() {
    let sender = Arc::new(RecordingSender::default());
    let (summary, events) = run_with(
        vec![broker("a"), broker("b"), broker("c"), broker("d")],
        sender.clone(),
        None,
        SendOptions {
            daily_limit: Some(2),
            ..options()
        },
        &CancellationToken::new(),
    )
    .await;

    assert_eq!(summary.sent, 2);
    assert_eq!(summary.skipped, 2);
    assert_eq!(sender.sent().len(), 2);
    assert_eq!(
        outcomes(&events)[2],
        Outcome::SkippedOverLimit,
        "brokers past the cap should still be reported, as skipped"
    );
}

/// A failed attempt still consumed a send, so it counts against the cap.
#[tokio::test]
async fn failures_count_against_the_daily_limit() {
    let sender = Arc::new(RecordingSender::failing_for(&["privacy@a.example"]));
    let (summary, _) = run_with(
        vec![broker("a"), broker("b"), broker("c")],
        sender.clone(),
        None,
        SendOptions {
            daily_limit: Some(2),
            ..options()
        },
        &CancellationToken::new(),
    )
    .await;

    assert_eq!(summary.failed, 1);
    assert_eq!(summary.sent, 1);
    assert_eq!(summary.skipped, 1);
}

#[tokio::test]
async fn cancelling_before_the_run_sends_nothing() {
    let cancel = CancellationToken::new();
    cancel.cancel();

    let sender = Arc::new(RecordingSender::default());
    let (summary, _) = run_with(
        vec![broker("a"), broker("b")],
        sender.clone(),
        None,
        options(),
        &cancel,
    )
    .await;

    assert!(summary.cancelled);
    assert_eq!(summary.sent, 0);
    assert_eq!(summary.skipped, 2);
    assert!(sender.sent().is_empty());
}

#[tokio::test]
async fn cancelling_mid_run_stops_after_the_current_broker() {
    let cancel = CancellationToken::new();
    let sender = Arc::new(RecordingSender::default());

    let job = SendJob {
        brokers: vec![broker("a"), broker("b"), broker("c")],
        profile: profile(),
        engine: Arc::new(Engine::new().unwrap()),
        sender: sender.clone(),
        store: None,
        options: options(),
    };

    let summary = job
        .run(&cancel, |event| {
            if let Progress::Broker { index: 1, .. } = event {
                cancel.cancel();
            }
        })
        .await;

    assert!(summary.cancelled);
    assert_eq!(summary.sent, 1);
    assert_eq!(summary.skipped, 2);
    assert_eq!(sender.sent().len(), 1);
}

/// Cancelling must not have to wait out the inter-send delay.
#[tokio::test(start_paused = true)]
async fn cancelling_interrupts_the_rate_limit_delay() {
    let cancel = CancellationToken::new();
    let job = SendJob {
        brokers: vec![broker("a"), broker("b")],
        profile: profile(),
        engine: Arc::new(Engine::new().unwrap()),
        sender: Arc::new(RecordingSender::default()),
        store: None,
        options: SendOptions {
            rate_limit: Duration::from_secs(3600),
            ..options()
        },
    };

    let started = tokio::time::Instant::now();
    let summary = job
        .run(&cancel, |event| {
            if let Progress::Broker { index: 1, .. } = event {
                cancel.cancel();
            }
        })
        .await;

    assert!(summary.cancelled);
    assert!(
        started.elapsed() < Duration::from_secs(3600),
        "the run waited out the full delay instead of cancelling"
    );
}

#[tokio::test(start_paused = true)]
async fn the_rate_limit_delay_is_applied_between_sends_but_not_after_the_last() {
    let job = SendJob {
        brokers: vec![broker("a"), broker("b"), broker("c")],
        profile: profile(),
        engine: Arc::new(Engine::new().unwrap()),
        sender: Arc::new(RecordingSender::default()),
        store: None,
        options: SendOptions {
            rate_limit: Duration::from_secs(2),
            ..options()
        },
    };

    let started = tokio::time::Instant::now();
    job.run(&CancellationToken::new(), |_| {}).await;

    // Two gaps between three brokers, and none trailing the last.
    assert_eq!(started.elapsed(), Duration::from_secs(4));
}

#[tokio::test]
async fn an_empty_broker_list_is_a_no_op() {
    let (summary, events) = run_with(
        Vec::new(),
        Arc::new(RecordingSender::default()),
        None,
        options(),
        &CancellationToken::new(),
    )
    .await;

    assert_eq!(summary, Summary::default());
    assert_eq!(events.len(), 2, "just Started and Finished");
}

#[tokio::test]
async fn sender_for_picks_the_dry_run_transport() {
    let config = crate::config::EmailConfig {
        provider: "smtp".into(),
        from: "jane@example.com".into(),
        smtp: crate::config::SmtpConfig {
            host: "smtp.example.com".into(),
            port: 465,
            use_tls: true,
            ..Default::default()
        },
    };

    assert_eq!(sender_for(&config, true).unwrap().name(), "dry-run");
    assert_eq!(sender_for(&config, false).unwrap().name(), "smtp");
}

#[test]
fn summary_counts_attempts_excluding_skips() {
    let summary = Summary {
        sent: 3,
        failed: 2,
        skipped: 10,
        cancelled: false,
    };
    assert_eq!(summary.attempted(), 5);
}

#[test]
fn error_chain_flattens_nested_sources() {
    let error = crate::email::Error::Invalid(crate::email::ValidationError::Recipient(Box::new(
        crate::email::ValidationError::Malformed("not-an-address".into()),
    )));

    let flattened = error_chain(&error);
    assert!(flattened.contains("invalid recipient address"));
    assert!(
        flattened.contains("not-an-address"),
        "the detail that identifies the problem must survive: {flattened}"
    );
}

#[test]
fn error_chain_does_not_repeat_a_link_that_restates_its_parent() {
    let error = crate::email::Error::Invalid(crate::email::ValidationError::SubjectLineBreak);
    assert_eq!(error_chain(&error), "subject contains a line break");
}
