//! Browser driver tests.
//!
//! Nothing here launches Chrome. The decisions worth pinning — what goes in
//! which box, whether a page needs a person, what the screenshots are called
//! — are all reachable without one; the CDP conversation is chromiumoxide's
//! job. A launch test would need a browser installed on every machine and in
//! CI, and would still only prove that chromiumoxide works.

use super::*;

use crate::automation::captcha::CaptchaKind;
use crate::automation::filler::{FieldKind, PlannedFill};

fn profile() -> Profile {
    Profile {
        first_name: "Jane".into(),
        last_name: "Doe".into(),
        email: "jane@example.com".into(),
        ..Default::default()
    }
}

fn field(name: &str, required: bool) -> FormField {
    FormField {
        selector: format!("[data-eruser-field=\"{name}\"]"),
        name: name.to_string(),
        input_type: "text".to_string(),
        required,
        ..Default::default()
    }
}

fn outcome(plan: FillPlan, captcha: Option<Captcha>, submitted: bool) -> FormOutcome {
    FormOutcome {
        url: "https://acme.example/opt-out".into(),
        final_url: "https://acme.example/opt-out".into(),
        title: "Opt out".into(),
        plan,
        captcha,
        submitted,
        screenshot: None,
    }
}

fn filled_plan() -> FillPlan {
    FillPlan {
        fills: vec![PlannedFill {
            field: field("email", true),
            kind: FieldKind::Email,
            value: "jane@example.com".into(),
            score: 140,
        }],
        ..Default::default()
    }
}

fn challenge(kind: CaptchaKind) -> Captcha {
    Captcha {
        kind,
        confidence: 0.85,
        matched: "test".into(),
    }
}

// -------------------------------------------------------------------
// Defaults
// -------------------------------------------------------------------

/// A form that submits the wrong thing cannot be un-submitted. Filling and
/// screenshotting is the safe default for a tool acting on someone's behalf.
#[test]
fn forms_are_not_submitted_unless_asked() {
    let options = BrowserOptions::default();

    assert!(!options.submit);
    assert!(options.headless);
    assert!(options.screenshot_dir.is_none());
}

// -------------------------------------------------------------------
// Reporting
// -------------------------------------------------------------------

#[test]
fn a_filled_form_says_what_is_left_to_do() {
    let result = outcome(filled_plan(), None, false);

    assert!(result.summary().contains("press submit"));
    assert!(
        !result.needs_a_person(),
        "everything answerable was answered"
    );
}

#[test]
fn a_submitted_form_says_so() {
    assert!(
        outcome(filled_plan(), None, true)
            .summary()
            .contains("submitted")
    );
}

/// Anything typed under a challenge is likely to be thrown away, so the page
/// has to go to a person.
#[test]
fn a_challenge_sends_the_page_to_a_person() {
    let result = outcome(filled_plan(), Some(challenge(CaptchaKind::HCaptcha)), false);

    assert!(result.needs_a_person());
    assert!(result.summary().contains("blocked by a challenge"));
    assert!(result.summary().contains("images"), "{}", result.summary());
}

/// v3 scores the visitor in the background rather than asking anything, so a
/// page carrying one is still fillable.
#[test]
fn an_invisible_challenge_does_not_send_the_page_to_a_person() {
    let result = outcome(
        filled_plan(),
        Some(challenge(CaptchaKind::RecaptchaV3)),
        false,
    );

    assert!(!result.needs_a_person());
    assert!(!result.summary().contains("blocked"));
}

#[test]
fn a_page_with_nothing_fillable_says_so() {
    let result = outcome(FillPlan::default(), None, false);

    assert!(result.needs_a_person());
    assert!(result.summary().contains("nothing on the page"));
}

/// The form will refuse to submit, so say why rather than reporting a
/// successful fill.
#[test]
fn a_missing_required_answer_is_called_out() {
    let plan = FillPlan {
        unanswered: vec![field("phone", true)],
        ..filled_plan()
    };
    let result = outcome(plan, None, false);

    assert!(result.needs_a_person());
    assert!(result.summary().contains("required"));
}

/// An optional box left blank is not a problem worth stopping for.
#[test]
fn an_unanswered_optional_box_is_not_a_problem() {
    let plan = FillPlan {
        unanswered: vec![field("phone", false)],
        ..filled_plan()
    };
    let result = outcome(plan, None, false);

    assert!(!result.needs_a_person());
    assert!(!result.summary().contains("required"));
}

// -------------------------------------------------------------------
// Screenshots
// -------------------------------------------------------------------

/// Two brokers must not overwrite each other's screenshots, and a broker id
/// must not be able to escape the directory.
#[test]
fn a_screenshot_name_is_safe_and_specific() {
    let name = screenshot_name("acme-data");

    assert!(name.starts_with("acme-data-"));
    assert!(name.ends_with(".png"));
    assert!(!name.contains('/'));
}

#[test]
fn a_broker_id_cannot_escape_the_screenshot_directory() {
    for id in ["../../etc/passwd", "a/b", "a\\b", "..", "a:b"] {
        let name = screenshot_name(id);

        assert!(!name.contains('/'), "{id} produced {name}");
        assert!(!name.contains('\\'), "{id} produced {name}");
        assert!(!name.contains(".."), "{id} produced {name}");
        assert!(
            std::path::Path::new(&name).components().count() == 1,
            "{id} produced {name}"
        );
    }
}

#[test]
fn screenshots_default_to_the_eruser_directory() {
    assert!(default_screenshot_dir().ends_with("screenshots"));
}

#[test]
fn a_usable_screenshot_directory_is_recognised() {
    let dir = tempfile::tempdir().unwrap();

    assert!(is_usable_screenshot_dir(dir.path()));
    // A directory that does not exist yet is fine; it gets created.
    assert!(is_usable_screenshot_dir(&dir.path().join("nested")));
    assert!(!is_usable_screenshot_dir(std::path::Path::new("")));

    // A path that is a file is not.
    let file = dir.path().join("a-file");
    std::fs::write(&file, "").unwrap();
    assert!(!is_usable_screenshot_dir(&file));
}

// -------------------------------------------------------------------
// The scripts the driver injects
// -------------------------------------------------------------------

/// These run in a page rather than being compiled, so a typo is only found
/// at runtime against a live broker. Checking the shape here is cheap.
#[test]
fn the_field_script_reads_everything_the_matcher_needs() {
    for key in [
        "selector",
        "name",
        "id",
        "placeholder",
        "input_type",
        "autocomplete",
        "label",
        "required",
    ] {
        assert!(
            COLLECT_FIELDS.contains(key),
            "the collector never reports {key}, which the matcher relies on"
        );
    }

    assert!(COLLECT_FIELDS.contains("input, textarea, select"));
    assert!(balanced(COLLECT_FIELDS), "unbalanced brackets");
}

#[test]
fn the_submit_script_looks_for_the_usual_wording() {
    for word in ["submit", "send", "opt", "remove", "confirm"] {
        assert!(FIND_SUBMIT.contains(word), "{word} is not looked for");
    }
    assert!(balanced(FIND_SUBMIT), "unbalanced brackets");
}

/// Neither script may contain a double quote, because both are embedded in
/// Rust raw strings and a stray one would end them early.
#[test]
fn the_scripts_use_single_quotes_for_their_own_strings() {
    for script in [COLLECT_FIELDS, FIND_SUBMIT] {
        // The only double quotes are inside the selectors being built.
        let quotes = script.matches('"').count();
        assert!(quotes <= 6, "too many double quotes to be safe: {quotes}");
    }
}

fn balanced(script: &str) -> bool {
    let mut depth: i32 = 0;
    for c in script.chars() {
        match c {
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => depth -= 1,
            _ => {}
        }
        if depth < 0 {
            return false;
        }
    }
    depth == 0
}

// -------------------------------------------------------------------
// Field collection round trip
// -------------------------------------------------------------------

/// The script's output is deserialized straight into FormField, so the two
/// have to agree on every name.
#[test]
fn the_scripts_output_shape_deserializes_into_a_form_field() {
    let from_page = serde_json::json!([{
        "selector": "[data-eruser-field=\"0\"]",
        "name": "email",
        "id": "email-box",
        "placeholder": "Your email",
        "input_type": "email",
        "autocomplete": "email",
        "label": "Email address",
        "required": true,
    }]);

    let fields: Vec<FormField> = serde_json::from_value(from_page).expect("the shapes must agree");

    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].name, "email");
    assert!(fields[0].required);

    // And what comes back is immediately usable by the matcher.
    let plan = filler::plan(&profile(), &fields);
    assert_eq!(plan.fills.len(), 1);
    assert_eq!(plan.fills[0].kind, FieldKind::Email);
}

/// A page that reports fewer keys than expected should still be readable,
/// rather than losing the whole form to a deserialization error.
#[test]
fn a_partial_field_from_the_page_still_deserializes() {
    let sparse = serde_json::json!([{ "selector": "#a", "name": "email" }]);
    let fields: Vec<FormField> =
        serde_json::from_value(sparse).expect("missing keys should default");

    assert_eq!(fields[0].name, "email");
    assert_eq!(fields[0].input_type, "");
    assert!(!fields[0].required);
}
