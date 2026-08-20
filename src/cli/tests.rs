use super::*;

use crate::broker::Broker;
use crate::history::{PipelineStatus, Record, Stats, Status};
use crate::send::{Outcome, Progress, Summary};
use clap::Parser;

fn broker(id: &str) -> Broker {
    Broker {
        id: id.to_string(),
        name: format!("Broker {id}"),
        email: format!("privacy@{id}.example"),
        website: format!("https://{id}.example"),
        opt_out_url: String::new(),
        region: "us".to_string(),
        category: "people-search".to_string(),
        notes: String::new(),
        requires_id: false,
        tags: Vec::new(),
    }
}

// -------------------------------------------------------------------
// Argument parsing
// -------------------------------------------------------------------

/// clap's own assertions catch conflicting flags, duplicate short options,
/// and bad defaults, which are easy to introduce and invisible until runtime.
#[test]
fn the_command_line_definition_is_valid() {
    use clap::CommandFactory;
    Cli::command().debug_assert();
}

#[test]
fn send_defaults_to_actually_sending() {
    let cli = Cli::parse_from(["eruser", "send"]);
    let Command::Send(args) = cli.command else {
        panic!("expected the send command");
    };
    assert!(!args.dry_run);
    assert!(args.template.is_none());
    assert!(args.limit.is_none());
}

#[test]
fn send_accepts_its_flags() {
    let cli = Cli::parse_from([
        "eruser",
        "send",
        "--dry-run",
        "--template",
        "gdpr",
        "--limit",
        "250",
        "--rate-limit-ms",
        "500",
    ]);
    let Command::Send(args) = cli.command else {
        panic!("expected the send command");
    };
    assert!(args.dry_run);
    assert_eq!(args.template.as_deref(), Some("gdpr"));
    assert_eq!(args.limit, Some(250));
    assert_eq!(args.rate_limit_ms, Some(500));
}

#[test]
fn regions_can_be_repeated_or_comma_separated() {
    let repeated = Cli::parse_from(["eruser", "send", "--region", "us", "--region", "eu"]);
    let Command::Send(args) = repeated.command else {
        panic!("expected the send command");
    };
    assert_eq!(args.region, ["us", "eu"]);

    let joined = Cli::parse_from(["eruser", "send", "--region", "us,eu"]);
    let Command::Send(args) = joined.command else {
        panic!("expected the send command");
    };
    assert_eq!(args.region, ["us", "eu"]);
}

#[test]
fn individual_brokers_can_be_targeted() {
    let cli = Cli::parse_from(["eruser", "send", "--broker", "spokeo", "--broker", "acme"]);
    let Command::Send(args) = cli.command else {
        panic!("expected the send command");
    };
    assert_eq!(args.brokers, ["spokeo", "acme"]);
}

#[test]
fn global_flags_work_before_and_after_the_subcommand() {
    for argv in [
        ["eruser", "--config", "/tmp/c.yaml", "status"],
        ["eruser", "status", "--config", "/tmp/c.yaml"],
    ] {
        let cli = Cli::parse_from(argv);
        assert_eq!(cli.config.as_deref(), Some(Path::new("/tmp/c.yaml")));
    }
}

#[test]
fn status_defaults_to_twenty_entries() {
    let cli = Cli::parse_from(["eruser", "status"]);
    let Command::Status(args) = cli.command else {
        panic!("expected the status command");
    };
    assert_eq!(args.limit, 20);
    assert!(!args.failed);
}

#[test]
fn serve_defaults_to_localhost_only() {
    let cli = Cli::parse_from(["eruser", "serve"]);
    let Command::Serve(args) = cli.command else {
        panic!("expected the serve command");
    };
    assert_eq!(args.port, 8080);
    assert_eq!(
        args.host, "127.0.0.1",
        "the web UI has no authentication yet, so it must not bind publicly by default"
    );
}

#[test]
fn serve_accepts_a_short_port_flag() {
    let cli = Cli::parse_from(["eruser", "serve", "-p", "3000"]);
    let Command::Serve(args) = cli.command else {
        panic!("expected the serve command");
    };
    assert_eq!(args.port, 3000);
}

#[test]
fn an_unknown_command_is_rejected() {
    assert!(Cli::try_parse_from(["eruser", "teleport"]).is_err());
}

// -------------------------------------------------------------------
// Path resolution
// -------------------------------------------------------------------

#[test]
fn an_explicit_config_path_wins() {
    let paths = Paths {
        config: Some(PathBuf::from("/tmp/custom.yaml")),
        brokers: None,
    };
    assert_eq!(paths.config_path(), Path::new("/tmp/custom.yaml"));
}

#[test]
fn without_a_flag_the_default_config_path_is_used() {
    assert_eq!(
        Paths::default().config_path(),
        config::default_config_path()
    );
}

/// A typo in --brokers should be reported, not silently swapped for a
/// different database than the one asked for.
#[test]
fn an_explicit_broker_path_is_used_even_when_missing() {
    let paths = Paths {
        config: None,
        brokers: Some(PathBuf::from("/nonexistent/brokers.yaml")),
    };
    assert_eq!(
        paths.broker_path().as_deref(),
        Some(Path::new("/nonexistent/brokers.yaml"))
    );
    assert!(matches!(
        paths.load_brokers().unwrap_err(),
        Error::Broker(crate::broker::Error::Read { .. })
    ));
}

#[test]
fn a_broker_file_is_loaded_when_one_is_given() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brokers.yaml");
    crate::broker::BrokerDatabase {
        brokers: vec![broker("only")],
    }
    .save(&path)
    .unwrap();

    let paths = Paths {
        config: None,
        brokers: Some(path),
    };
    let db = paths.load_brokers().unwrap();
    assert_eq!(db.brokers.len(), 1);
    assert_eq!(db.brokers[0].id, "only");
}

#[test]
fn a_missing_config_says_how_to_create_one() {
    let paths = Paths {
        config: Some(PathBuf::from("/nonexistent/config.yaml")),
        brokers: None,
    };
    let message = paths.load_config().unwrap_err().to_string();
    assert!(message.contains("eruser init"), "{message}");
}

// -------------------------------------------------------------------
// list-brokers output
// -------------------------------------------------------------------

#[test]
fn the_listing_shows_every_populated_field() {
    let brokers = [broker("acme")];
    let refs: Vec<&Broker> = brokers.iter().collect();
    let out = list_brokers::format_brokers(&refs, 1, &list_brokers::Args::default());

    assert!(out.contains("Broker acme  [acme]"));
    assert!(out.contains("privacy@acme.example"));
    assert!(out.contains("https://acme.example"));
    assert!(out.contains("people-search"));
    // opt_out_url is empty on this fixture, so its label must not appear.
    assert!(!out.contains("opt out"));
}

#[test]
fn the_listing_says_how_many_of_the_total_matched() {
    let brokers = [broker("acme")];
    let refs: Vec<&Broker> = brokers.iter().collect();
    assert!(
        list_brokers::format_brokers(&refs, 764, &list_brokers::Args::default())
            .contains("1 of 764")
    );
    assert!(
        list_brokers::format_brokers(&refs, 1, &list_brokers::Args::default())
            .starts_with("1 data brokers")
    );
}

#[test]
fn an_empty_listing_explains_itself() {
    let out = list_brokers::format_brokers(&[], 764, &list_brokers::Args::default());
    assert!(out.contains("No brokers matched"));
    assert!(out.contains("764"));
}

#[test]
fn ids_only_prints_one_id_per_line() {
    let brokers = [broker("a"), broker("b")];
    let refs: Vec<&Broker> = brokers.iter().collect();
    let out = list_brokers::format_brokers(
        &refs,
        2,
        &list_brokers::Args {
            ids_only: true,
            ..Default::default()
        },
    );
    assert_eq!(out, "a\nb\n");
}

#[test]
fn filters_match_case_insensitively_across_name_id_and_email() {
    let target = broker("acme");
    let args = |search: &str| list_brokers::Args {
        search: Some(search.to_string()),
        ..Default::default()
    };

    assert!(list_brokers::matches(&target, &args("ACME")));
    assert!(list_brokers::matches(&target, &args("broker acme")));
    assert!(list_brokers::matches(&target, &args("privacy@acme")));
    assert!(!list_brokers::matches(&target, &args("nothing")));
}

#[test]
fn region_and_category_filters_are_exact_but_case_insensitive() {
    let target = broker("acme");

    assert!(list_brokers::matches(
        &target,
        &list_brokers::Args {
            region: Some("US".into()),
            ..Default::default()
        }
    ));
    assert!(!list_brokers::matches(
        &target,
        &list_brokers::Args {
            region: Some("eu".into()),
            ..Default::default()
        }
    ));
    assert!(list_brokers::matches(
        &target,
        &list_brokers::Args {
            category: Some("People-Search".into()),
            ..Default::default()
        }
    ));
}

// -------------------------------------------------------------------
// send output
// -------------------------------------------------------------------

fn broker_event(index: usize, outcome: Outcome) -> Progress {
    Progress::Broker {
        index,
        total: 12,
        broker_id: "acme".into(),
        broker_name: "Acme Data".into(),
        broker_email: "privacy@acme.example".into(),
        outcome,
    }
}

#[test]
fn send_progress_reads_differently_in_a_dry_run() {
    let event = broker_event(
        1,
        Outcome::Sent {
            message_id: "<x@y>".into(),
        },
    );

    assert!(send::format_progress(&event, false).contains("sent to Acme Data"));

    let preview = send::format_progress(&event, true);
    assert!(preview.contains("would send to Acme Data"));
    assert!(preview.contains("privacy@acme.example"));
}

#[test]
fn send_progress_counters_are_aligned() {
    let sent = Outcome::Sent {
        message_id: String::new(),
    };
    let first = send::format_progress(&broker_event(1, sent.clone()), false);
    let tenth = send::format_progress(&broker_event(10, sent), false);

    assert!(first.starts_with("[ 1/12]"), "{first:?}");
    assert!(tenth.starts_with("[10/12]"), "{tenth:?}");
}

#[test]
fn a_failure_shows_the_reason_on_the_same_line() {
    let out = send::format_progress(
        &broker_event(
            3,
            Outcome::Failed {
                error: "invalid recipient address: not-an-address".into(),
            },
        ),
        false,
    );
    assert!(out.contains("FAILED Acme Data"));
    assert!(out.contains("not-an-address"));
}

#[test]
fn a_skipped_broker_says_why_it_was_skipped() {
    let out = send::format_progress(&broker_event(9, Outcome::SkippedOverLimit), false);
    assert!(out.contains("daily limit"));
}

#[test]
fn the_summary_counts_only_what_happened() {
    let out = send::format_summary(
        &Summary {
            sent: 700,
            failed: 0,
            skipped: 0,
            cancelled: false,
        },
        false,
    );
    assert!(out.contains("700 sent."));
    assert!(!out.contains("failed"));
    assert!(!out.contains("skipped"));
}

#[test]
fn the_summary_points_at_status_when_something_failed() {
    let out = send::format_summary(
        &Summary {
            sent: 5,
            failed: 2,
            skipped: 0,
            cancelled: false,
        },
        false,
    );
    assert!(out.contains("5 sent, 2 failed."));
    assert!(out.contains("eruser status"));
}

#[test]
fn a_cancelled_run_says_how_to_resume() {
    let out = send::format_summary(
        &Summary {
            sent: 3,
            failed: 0,
            skipped: 9,
            cancelled: true,
        },
        false,
    );
    assert!(out.contains("Stopped early"));
    assert!(out.contains("eruser send"));
}

#[test]
fn a_dry_run_summary_does_not_claim_anything_was_sent() {
    let out = send::format_summary(
        &Summary {
            sent: 764,
            failed: 0,
            skipped: 0,
            cancelled: false,
        },
        true,
    );
    assert!(out.contains("would be contacted"));
    assert!(!out.contains("764 sent"));
}

// -------------------------------------------------------------------
// status output
// -------------------------------------------------------------------

fn record(broker_id: &str, status: Status, error: &str) -> Record {
    Record {
        id: 1,
        user_id: crate::history::DEFAULT_USER_ID,
        broker_id: broker_id.to_string(),
        broker_name: format!("Broker {broker_id}"),
        email: format!("privacy@{broker_id}.example"),
        template: "generic".to_string(),
        status,
        message_id: String::new(),
        error: error.to_string(),
        sent_at: Some(chrono::Utc::now()),
        created_at: Some(chrono::Utc::now()),
        pipeline_status: PipelineStatus::EmailSent,
    }
}

#[test]
fn an_empty_history_suggests_a_dry_run() {
    let out = status::format_status(Stats::default(), Stats::default(), &[], 20);
    assert!(out.contains("Nothing sent yet"));
    assert!(out.contains("--dry-run"));
}

#[test]
fn the_status_report_shows_all_time_and_monthly_counts() {
    let all_time = Stats {
        total: 100,
        sent: 95,
        failed: 5,
    };
    let this_month = Stats {
        total: 20,
        sent: 20,
        failed: 0,
    };
    let out = status::format_status(all_time, this_month, &[], 20);

    assert!(out.contains("all time     95 sent, 5 failed"));
    assert!(out.contains("this month   20 sent, 0 failed"));
}

#[test]
fn failed_entries_show_their_error_underneath() {
    let records = [record("acme", Status::Failed, "SMTP authentication failed")];
    let out = status::format_status(
        Stats {
            total: 1,
            sent: 0,
            failed: 1,
        },
        Stats::default(),
        &records,
        20,
    );

    assert!(out.contains("FAIL"));
    assert!(out.contains("Broker acme"));
    assert!(out.contains("SMTP authentication failed"));
}

#[test]
fn successful_entries_carry_no_error_line() {
    let records = [record("acme", Status::Sent, "")];
    let out = status::format_status(
        Stats {
            total: 1,
            sent: 1,
            failed: 0,
        },
        Stats::default(),
        &records,
        20,
    );

    assert!(out.contains("ok  "));
    assert!(!out.contains("FAIL"));
}

// -------------------------------------------------------------------
// add-broker
// -------------------------------------------------------------------

#[test]
fn a_name_becomes_a_hyphenated_lowercase_id() {
    assert_eq!(add_broker::slugify("Acme Data"), "acme-data");
    assert_eq!(add_broker::slugify("BeenVerified"), "beenverified");
    assert_eq!(add_broker::slugify("US Search, Inc."), "us-search-inc");
    assert_eq!(add_broker::slugify("  padded  "), "padded");
}

/// Ids end up in URLs and filenames, so a slug must never carry punctuation
/// or leading and trailing separators.
#[test]
fn slugs_contain_only_letters_digits_and_inner_hyphens() {
    for name in ["A & B!", "-- weird --", "123 Data Co."] {
        let slug = add_broker::slugify(name);
        assert!(
            slug.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "{name:?} produced {slug:?}"
        );
        assert!(!slug.starts_with('-') && !slug.ends_with('-'), "{slug:?}");
    }
}

#[test]
fn an_empty_name_produces_an_empty_slug() {
    assert_eq!(add_broker::slugify("!!!"), "");
}

// -------------------------------------------------------------------
// serve
// -------------------------------------------------------------------

/// Until the web UI lands, the command must say so and point somewhere
/// useful rather than starting a server that serves nothing.
#[tokio::test]
async fn serve_explains_that_it_is_not_ready_yet() {
    let error = serve::run(&Paths::default(), serve::Args::default())
        .await
        .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("not been ported yet"), "{message}");
    assert!(message.contains("eruser send"), "{message}");
}

/// Some failures across 764 community-maintained addresses are normal, so a
/// partial failure must stay scriptable.
#[test]
fn the_all_failed_error_explains_what_to_check() {
    let message = Error::AllSendsFailed { count: 764 }.to_string();
    assert!(message.contains("764"));
    assert!(message.contains("email settings"));
    assert!(message.contains("eruser status"));
}
