use super::*;

fn question() -> Choice {
    Choice::new(
        "Which reply does this email deserve?",
        [
            Choice::option("missing_info", "they asked for details"),
            Choice::option("none", "no reply is warranted"),
        ],
    )
}

fn decision(choice: &str, confidence: f32) -> Decision {
    Decision {
        choice: choice.to_string(),
        confidence,
        probabilities: std::collections::BTreeMap::new(),
        model: "jev-1.13.0".to_string(),
    }
}

// -------------------------------------------------------------------
// The answers a question allows
// -------------------------------------------------------------------

#[test]
fn a_choice_offers_only_the_labels_it_was_built_with() {
    let question = question();
    assert!(question.offers("missing_info"));
    assert!(question.offers("none"));
    assert!(!question.offers("id_verification"));
    assert!(!question.offers(""));
}

// -------------------------------------------------------------------
// The confidence floor
// -------------------------------------------------------------------

/// The floor is what stops a model that is barely better than guessing from
/// acting on somebody's behalf.
#[test]
fn a_decision_below_the_floor_is_not_accepted() {
    assert!(
        decision("none", 0.41)
            .accepted_at(DEFAULT_MIN_CONFIDENCE)
            .is_none()
    );
    assert!(
        decision("none", 0.59)
            .accepted_at(DEFAULT_MIN_CONFIDENCE)
            .is_none()
    );
}

#[test]
fn a_decision_at_or_above_the_floor_is_accepted() {
    let answer = decision("missing_info", 0.6);
    let accepted = answer
        .accepted_at(DEFAULT_MIN_CONFIDENCE)
        .expect("the floor is met");
    assert_eq!(accepted.choice, "missing_info");

    assert!(
        decision("missing_info", 0.99)
            .accepted_at(DEFAULT_MIN_CONFIDENCE)
            .is_some()
    );
}

/// A floor of zero accepts anything that came back, including a model that
/// says it knows nothing.
#[test]
fn a_floor_of_zero_accepts_every_confidence() {
    assert!(decision("none", 0.0).accepted_at(0.0).is_some());
}

// -------------------------------------------------------------------
// The state a question is asked about
// -------------------------------------------------------------------

#[test]
fn the_state_names_the_broker_and_quotes_the_email() {
    let state = state_of("Acme Data", "Your request", "Please confirm your request.");

    assert!(state.contains("Acme Data"));
    assert!(state.contains("Your request"));
    assert!(state.contains("Please confirm your request."));
    assert!(state.contains("quoted as data"));
}

/// A quoted thread is mostly what was said weeks ago; the answer is at the
/// top. Cutting it is a silent change to what the model sees, so the cut is
/// marked in the text.
#[test]
fn a_very_long_email_is_cut_with_a_marker() {
    let body = "a".repeat(MAX_STATE_CHARS + 500);
    let state = state_of("Acme Data", "Your request", &body);

    assert!(state.contains("[...the rest of this email is omitted...]"));
    assert!(
        state.chars().count() < MAX_STATE_CHARS + 400,
        "the state was not actually cut"
    );
}

#[test]
fn an_email_that_fits_is_left_whole() {
    let state = state_of("Acme Data", "Your request", "  A short reply.  ");
    assert!(state.contains("A short reply."));
    assert!(!state.contains("omitted"));
}

// -------------------------------------------------------------------
// Describing the answer
// -------------------------------------------------------------------

#[test]
fn a_decision_describes_itself_for_the_log() {
    assert_eq!(
        decision("missing_info", 0.82).describe(),
        "missing_info (confidence 0.82)"
    );
}
