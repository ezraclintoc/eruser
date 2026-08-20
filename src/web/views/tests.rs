use super::*;

use crate::history::{DEFAULT_USER_ID, PipelineStatus, Status};

fn broker(id: &str, category: &str, region: &str) -> Broker {
    Broker {
        id: id.to_string(),
        name: format!("Broker {id}"),
        email: format!("privacy@{id}.example"),
        website: String::new(),
        opt_out_url: String::new(),
        region: region.to_string(),
        category: category.to_string(),
        notes: String::new(),
        requires_id: false,
        tags: Vec::new(),
    }
}

fn broker_status(status: Status, total_sent: i64) -> BrokerStatus {
    BrokerStatus {
        broker_id: "acme".into(),
        last_sent: Some(chrono::Utc::now()),
        status,
        total_sent,
    }
}

fn filters(status: &str) -> BrokerFilters {
    BrokerFilters {
        status: status.to_string(),
        ..Default::default()
    }
}

// -------------------------------------------------------------------
// Stats
// -------------------------------------------------------------------

#[test]
fn pending_is_what_is_left_of_the_database() {
    let stats = Stats::new(
        764,
        history::Stats {
            total: 100,
            sent: 90,
            failed: 10,
        },
    );

    assert_eq!(stats.total_brokers, 764);
    assert_eq!(stats.sent, 90);
    assert_eq!(stats.failed, 10);
    assert_eq!(stats.pending, 664);
}

/// History can outnumber the database when brokers are removed from
/// brokers.yaml after being contacted. A negative "pending" on the dashboard
/// would be nonsense.
#[test]
fn pending_never_goes_negative() {
    let stats = Stats::new(
        5,
        history::Stats {
            total: 100,
            sent: 90,
            failed: 10,
        },
    );
    assert_eq!(stats.pending, 0);
}

#[test]
fn an_untouched_install_shows_everything_as_pending() {
    let stats = Stats::new(764, history::Stats::default());
    assert_eq!(stats.pending, 764);
    assert_eq!(stats.sent, 0);
}

// -------------------------------------------------------------------
// Broker rows
// -------------------------------------------------------------------

#[test]
fn a_broker_with_no_history_reads_as_never_contacted() {
    let row = BrokerWithStatus::new(broker("acme", "marketing", "us"), None);

    assert_eq!(row.status, "never");
    assert_eq!(row.last_sent, "");
    assert_eq!(row.total_sent, 0);
}

#[test]
fn a_contacted_broker_carries_its_status_and_date() {
    let row = BrokerWithStatus::new(
        broker("acme", "marketing", "us"),
        Some(&broker_status(Status::Sent, 3)),
    );

    assert_eq!(row.status, "sent");
    assert_eq!(row.total_sent, 3);
    assert!(!row.last_sent.is_empty(), "a contacted broker has a date");
}

#[test]
fn a_broker_row_flattens_so_templates_read_it_directly() {
    let row = BrokerWithStatus::new(broker("acme", "marketing", "us"), None);
    let json = serde_json::to_value(&row).unwrap();

    assert_eq!(json["name"], "Broker acme");
    assert_eq!(json["email"], "privacy@acme.example");
    assert_eq!(json["status"], "never");
    assert!(
        json.get("broker").is_none(),
        "the broker should be flattened"
    );
}

#[test]
fn search_matches_name_or_address_case_insensitively() {
    let row = BrokerWithStatus::new(broker("acme", "marketing", "us"), None);
    let search = |text: &str| BrokerFilters {
        search: text.to_string(),
        ..Default::default()
    };

    assert!(row.matches(&search("BROKER ACME")));
    assert!(row.matches(&search("privacy@acme")));
    assert!(!row.matches(&search("nothing")));
}

#[test]
fn category_and_region_filters_are_exact_but_case_insensitive() {
    let row = BrokerWithStatus::new(broker("acme", "marketing", "us"), None);

    assert!(row.matches(&BrokerFilters {
        category: "Marketing".into(),
        ..Default::default()
    }));
    assert!(!row.matches(&BrokerFilters {
        category: "people-search".into(),
        ..Default::default()
    }));
    assert!(row.matches(&BrokerFilters {
        region: "US".into(),
        ..Default::default()
    }));
    assert!(!row.matches(&BrokerFilters {
        region: "eu".into(),
        ..Default::default()
    }));
}

/// On this page "pending" means "not yet contacted", which is stored as
/// "never" — the two names have to line up or the filter shows nothing.
#[test]
fn the_pending_filter_means_never_contacted() {
    let never = BrokerWithStatus::new(broker("a", "", ""), None);
    let sent = BrokerWithStatus::new(broker("b", "", ""), Some(&broker_status(Status::Sent, 1)));

    assert!(never.matches(&filters("pending")));
    assert!(!sent.matches(&filters("pending")));
    assert!(sent.matches(&filters("sent")));
    assert!(!never.matches(&filters("sent")));
}

#[test]
fn an_empty_status_filter_matches_everything() {
    let never = BrokerWithStatus::new(broker("a", "", ""), None);
    let failed =
        BrokerWithStatus::new(broker("b", "", ""), Some(&broker_status(Status::Failed, 1)));

    assert!(never.matches(&filters("")));
    assert!(failed.matches(&filters("")));
}

#[test]
fn filters_are_trimmed_before_use() {
    let filters = BrokerFilters {
        search: "  acme  ".into(),
        category: " marketing ".into(),
        region: " us ".into(),
        status: " sent ".into(),
    }
    .normalized();

    assert_eq!(filters.search, "acme");
    assert_eq!(filters.category, "marketing");
    assert_eq!(filters.region, "us");
    assert_eq!(filters.status, "sent");
}

// -------------------------------------------------------------------
// Pipeline stats
// -------------------------------------------------------------------

#[test]
fn pipeline_stages_are_read_out_of_the_counts() {
    let stages = std::collections::HashMap::from([
        (PipelineStatus::EmailSent, 700),
        (PipelineStatus::FormRequired, 30),
        (PipelineStatus::Confirmed, 20),
    ]);

    let stats = PipelineStats::new(&stages, 0, 0, 0);

    assert_eq!(stats.email_sent, 700);
    assert_eq!(stats.form_required, 30);
    assert_eq!(stats.confirmed, 20);
    assert_eq!(stats.awaiting_response, 0, "a missing stage counts as zero");
}

/// The number on the dashboard has to match what the tasks page actually
/// lists, or it sends people to an empty page.
#[test]
fn the_action_needed_count_adds_forms_tasks_and_reviews() {
    let stats = PipelineStats::new(&Default::default(), 5, 12, 3);

    assert_eq!(stats.pending_tasks, 20);
    assert_eq!(stats.needs_review, 3);
}

// -------------------------------------------------------------------
// History rows
// -------------------------------------------------------------------

#[test]
fn a_history_row_carries_the_status_and_error_across() {
    let record = Record {
        id: 1,
        user_id: DEFAULT_USER_ID,
        broker_id: "acme".into(),
        broker_name: "Acme Data".into(),
        email: "privacy@acme.example".into(),
        template: "gdpr".into(),
        status: Status::Failed,
        message_id: String::new(),
        error: "the mail server rejected the recipient".into(),
        sent_at: Some(chrono::Utc::now()),
        created_at: None,
        pipeline_status: PipelineStatus::EmailSent,
    };

    let row = HistoryRow::from(record);

    assert_eq!(row.status, "failed");
    assert_eq!(row.broker_name, "Acme Data");
    assert!(row.error.contains("rejected"));
    assert_eq!(row.pipeline_status, "email_sent");
}

/// The template formats timestamps itself, so they have to arrive in a form
/// the filter can parse.
#[test]
fn timestamps_are_serialized_as_rfc_3339() {
    let record = Record {
        id: 1,
        user_id: DEFAULT_USER_ID,
        broker_id: "acme".into(),
        broker_name: "Acme".into(),
        email: "a@b.example".into(),
        template: "generic".into(),
        status: Status::Sent,
        message_id: String::new(),
        error: String::new(),
        sent_at: Some(
            chrono::DateTime::parse_from_rfc3339("2026-08-19T15:04:05Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        ),
        created_at: None,
        pipeline_status: PipelineStatus::EmailSent,
    };

    let row = HistoryRow::from(record);
    assert_eq!(row.sent_at.as_deref(), Some("2026-08-19T15:04:05+00:00"));
}

#[test]
fn a_row_that_was_never_sent_has_no_timestamp() {
    let record = Record {
        id: 1,
        user_id: DEFAULT_USER_ID,
        broker_id: "acme".into(),
        broker_name: "Acme".into(),
        email: "a@b.example".into(),
        template: "generic".into(),
        status: Status::Pending,
        message_id: String::new(),
        error: String::new(),
        sent_at: None,
        created_at: None,
        pipeline_status: PipelineStatus::EmailSent,
    };

    assert!(HistoryRow::from(record).sent_at.is_none());
}

// -------------------------------------------------------------------
// Filter menus
// -------------------------------------------------------------------

/// Go returned these in database order, so the dropdowns reshuffled whenever
/// brokers.yaml was edited.
#[test]
fn menu_values_are_deduplicated_and_sorted() {
    let brokers = vec![
        broker("a", "people-search", "us"),
        broker("b", "marketing", "eu"),
        broker("c", "people-search", "us"),
        broker("d", "background-check", "global"),
    ];

    assert_eq!(
        unique_values(&brokers, |b| &b.category),
        ["background-check", "marketing", "people-search"]
    );
    assert_eq!(
        unique_values(&brokers, |b| &b.region),
        ["eu", "global", "us"]
    );
}

#[test]
fn blank_values_are_left_out_of_the_menus() {
    let brokers = vec![broker("a", "", "us"), broker("b", "marketing", "")];

    assert_eq!(unique_values(&brokers, |b| &b.category), ["marketing"]);
    assert_eq!(unique_values(&brokers, |b| &b.region), ["us"]);
}

#[test]
fn an_empty_database_produces_empty_menus() {
    assert!(unique_values(&[], |b| &b.category).is_empty());
}
