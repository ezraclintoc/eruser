use super::*;
use crate::inbox::classifier::Summary;
use clap::Parser;

fn summary(by_type: Summary, fetched: usize, matched: usize) -> ScanSummary {
    ScanSummary {
        fetched,
        matched,
        by_type,
        ..Default::default()
    }
}

// -------------------------------------------------------------------
// Arguments
// -------------------------------------------------------------------

#[test]
fn monitor_defaults_to_a_week_of_mail() {
    let cli = crate::cli::Cli::parse_from(["eruser", "monitor"]);
    let crate::cli::Command::Monitor(args) = cli.command else {
        panic!("expected the monitor command");
    };

    assert_eq!(args.days, scan::DEFAULT_DAYS);
    assert!(!args.include_unmatched);
    assert!(!args.reclassify);
}

#[test]
fn monitor_accepts_its_flags() {
    let cli =
        crate::cli::Cli::parse_from(["eruser", "monitor", "--days", "30", "--include-unmatched"]);
    let crate::cli::Command::Monitor(args) = cli.command else {
        panic!("expected the monitor command");
    };

    assert_eq!(args.days, 30);
    assert!(args.include_unmatched);
}

/// Re-reading what is already stored fetches nothing, so a day range would
/// silently do nothing.
#[test]
fn reclassify_cannot_be_combined_with_fetch_options() {
    assert!(
        crate::cli::Cli::try_parse_from(["eruser", "monitor", "--reclassify", "--days", "30"])
            .is_err()
    );
    assert!(crate::cli::Cli::try_parse_from(["eruser", "monitor", "--reclassify"]).is_ok());
}

// -------------------------------------------------------------------
// Output
// -------------------------------------------------------------------

#[test]
fn an_empty_mailbox_says_so_plainly() {
    let out = format_progress(&Progress::Fetched { count: 0 });
    assert!(out.contains("No mail in that period"));

    // And nothing further is printed for a scan that read nothing.
    assert_eq!(format_summary(&ScanSummary::default()), "");
}

#[test]
fn message_counts_read_naturally() {
    assert!(format_progress(&Progress::Fetched { count: 1 }).contains("1 message"));
    assert!(format_progress(&Progress::Fetched { count: 12 }).contains("12 messages"));
}

#[test]
fn each_classified_reply_names_the_broker_and_what_it_wants() {
    let out = format_progress(&Progress::Classified {
        index: 3,
        total: 12,
        broker_name: "Acme Data".into(),
        response_type: ResponseType::FormRequired,
        confidence: 0.85,
    });

    assert!(out.contains("[ 3/12]"), "{out:?}");
    assert!(out.contains("Acme Data"));
    assert!(out.contains("form"));
    assert!(out.contains("85%"));
}

#[test]
fn counters_are_aligned() {
    let event = |index| Progress::Classified {
        index,
        total: 100,
        broker_name: "Acme".into(),
        response_type: ResponseType::Success,
        confidence: 1.0,
    };

    assert!(format_progress(&event(1)).starts_with("[  1/100]"));
    assert!(format_progress(&event(100)).starts_with("[100/100]"));
}

#[test]
fn a_bounce_stands_out_in_the_listing() {
    let out = format_progress(&Progress::Classified {
        index: 1,
        total: 1,
        broker_name: "Dead Broker".into(),
        response_type: ResponseType::Bounced,
        confidence: 0.95,
    });
    assert!(out.contains("BOUNCED"));
}

#[test]
fn the_summary_counts_only_the_categories_that_occurred() {
    let out = format_summary(&summary(
        Summary {
            total: 10,
            success: 4,
            form_required: 3,
            ..Default::default()
        },
        20,
        10,
    ));

    assert!(out.contains("10 of 20 messages"));
    assert!(out.contains("4  removed"));
    assert!(out.contains("3  need a form"));
    assert!(!out.contains("refused"), "nothing was refused: {out}");
    assert!(!out.contains("bounced"));
}

#[test]
fn the_summary_points_at_what_needs_a_person() {
    let out = format_summary(&summary(
        Summary {
            total: 5,
            form_required: 2,
            confirmation_required: 1,
            unknown: 2,
            needs_review: 2,
            ..Default::default()
        },
        5,
        5,
    ));

    assert!(out.contains("2 need a look"));
    assert!(out.contains("3 are waiting on you"));
    assert!(out.contains("eruser serve"));
}

#[test]
fn a_scan_with_nothing_outstanding_asks_nothing_of_you() {
    let out = format_summary(&summary(
        Summary {
            total: 3,
            success: 3,
            ..Default::default()
        },
        3,
        3,
    ));

    assert!(out.contains("3  removed"));
    assert!(!out.contains("waiting on you"));
    assert!(!out.contains("need a look"));
}

#[test]
fn reclassifying_reports_how_much_moved() {
    assert!(format_reclassify(0).contains("Nothing changed"));
    assert!(format_reclassify(1).contains("1 was filed differently"));
    assert!(format_reclassify(7).contains("7 were filed differently"));
}

#[test]
fn every_reply_type_has_a_label() {
    for response_type in [
        ResponseType::Success,
        ResponseType::FormRequired,
        ResponseType::ConfirmationRequired,
        ResponseType::Rejected,
        ResponseType::Pending,
        ResponseType::Bounced,
        ResponseType::Unknown,
    ] {
        assert!(!label(response_type).is_empty(), "{response_type}");
    }
}
