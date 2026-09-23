use super::*;

// -------------------------------------------------------------------
// What a run says
// -------------------------------------------------------------------

#[test]
fn a_run_that_drafted_says_so_and_that_nothing_was_sent() {
    let summary = DraftRunSummary {
        drafted: 3,
        ..Default::default()
    };

    let out = format_summary(&summary, false);
    assert!(out.contains("Drafted 3 replies"), "{out}");
    assert!(out.contains("Nothing has been sent"), "{out}");
    assert!(out.contains("task list"), "{out}");
}

#[test]
fn the_wording_is_right_for_one_draft() {
    let summary = DraftRunSummary {
        drafted: 1,
        ..Default::default()
    };

    let out = format_summary(&summary, false);
    assert!(out.contains("Drafted 1 reply."), "{out}");
}

#[test]
fn a_refusal_is_reported() {
    let summary = DraftRunSummary {
        drafted: 1,
        refused: 2,
        ..Default::default()
    };

    let out = format_summary(&summary, false);
    assert!(out.contains("2 drafts were refused"), "{out}");
}

#[test]
fn a_run_that_found_existing_drafts_says_so() {
    let summary = DraftRunSummary {
        drafted: 0,
        already_answered: 2,
        ..Default::default()
    };

    let out = format_summary(&summary, false);
    assert!(out.contains("2 replies already have a draft"), "{out}");
}

#[test]
fn a_quiet_run_with_nothing_to_say_says_nothing() {
    let summary = DraftRunSummary::default();
    assert!(format_summary(&summary, true).is_empty());
}

#[test]
fn a_loud_run_with_nothing_to_say_still_says_something() {
    let summary = DraftRunSummary::default();
    assert!(format_summary(&summary, false).contains("No replies"));
}
