use super::*;

fn profile() -> Profile {
    Profile {
        first_name: "Jane".into(),
        last_name: "Doe".into(),
        email: "jane@example.com".into(),
        address: "123 Main Street".into(),
        city: "San Francisco".into(),
        state: "CA".into(),
        zip_code: "94102".into(),
        country: "USA".into(),
        phone: "+1-555-123-4567".into(),
        date_of_birth: "1990-01-15".into(),
    }
}

/// A text input identified only by its name.
fn named(name: &str) -> FormField {
    FormField {
        selector: format!("input[name='{name}']"),
        name: name.to_string(),
        input_type: "text".to_string(),
        ..Default::default()
    }
}

fn filled(plan: &FillPlan, kind: FieldKind) -> Option<&PlannedFill> {
    plan.fills.iter().find(|fill| fill.kind == kind)
}

// -------------------------------------------------------------------
// Matching one field
// -------------------------------------------------------------------

#[test]
fn a_field_named_for_its_purpose_is_recognised() {
    let cases = [
        ("email", FieldKind::Email),
        ("first_name", FieldKind::FirstName),
        ("lastName", FieldKind::LastName),
        ("phone", FieldKind::Phone),
        ("city", FieldKind::City),
        ("zip", FieldKind::ZipCode),
        ("country", FieldKind::Country),
        ("dob", FieldKind::DateOfBirth),
    ];

    for (name, expected) in cases {
        let plan = plan(&profile(), &[named(name)]);
        assert_eq!(
            plan.fills.first().map(|fill| fill.kind),
            Some(expected),
            "a field named {name:?} should be the {expected}"
        );
    }
}

/// `first_name`, `first-name`, and `firstName` are the same field.
#[test]
fn separators_and_case_do_not_matter() {
    for name in ["first_name", "first-name", "firstName", "FIRSTNAME"] {
        let plan = plan(&profile(), &[named(name)]);
        assert_eq!(
            plan.fills.first().map(|fill| fill.kind),
            Some(FieldKind::FirstName),
            "for {name}"
        );
    }
}

/// The strongest signal there is: the page saying outright what the box is
/// for, rather than a name that has to be guessed at.
#[test]
fn an_autocomplete_attribute_outranks_the_name() {
    let field = FormField {
        selector: "#f1".into(),
        // A name that suggests nothing useful.
        name: "field_2".into(),
        autocomplete: "given-name".into(),
        input_type: "text".into(),
        ..Default::default()
    };

    let plan = plan(&profile(), &[field]);
    assert_eq!(filled(&plan, FieldKind::FirstName).unwrap().value, "Jane");
}

#[test]
fn an_input_type_is_a_strong_signal_on_its_own() {
    let field = FormField {
        selector: "#f1".into(),
        name: "contact".into(),
        input_type: "email".into(),
        ..Default::default()
    };

    let plan = plan(&profile(), &[field]);
    assert_eq!(
        filled(&plan, FieldKind::Email).unwrap().value,
        "jane@example.com"
    );
}

#[test]
fn a_placeholder_or_label_is_used_when_the_name_says_nothing() {
    let by_placeholder = FormField {
        selector: "#f1".into(),
        name: "q1".into(),
        placeholder: "Your phone number".into(),
        input_type: "text".into(),
        ..Default::default()
    };
    let by_label = FormField {
        selector: "#f2".into(),
        name: "q2".into(),
        label: "City".into(),
        input_type: "text".into(),
        ..Default::default()
    };

    let plan = plan(&profile(), &[by_placeholder, by_label]);
    assert!(filled(&plan, FieldKind::Phone).is_some());
    assert!(filled(&plan, FieldKind::City).is_some());
}

#[test]
fn a_field_nothing_matches_is_left_alone() {
    let plan = plan(&profile(), &[named("favourite_colour")]);

    assert!(plan.fills.is_empty());
    assert_eq!(plan.unrecognized.len(), 1);
}

// -------------------------------------------------------------------
// Fields that must never be typed into
// -------------------------------------------------------------------

/// Filling a hidden field, or typing a home address into a password box, is
/// worse than leaving the form half-done.
#[test]
fn fields_that_are_not_for_typing_are_skipped() {
    for input_type in [
        "hidden", "submit", "button", "reset", "image", "file", "password", "checkbox", "radio",
    ] {
        let field = FormField {
            selector: "#f".into(),
            // A name that would otherwise match.
            name: "email".into(),
            input_type: input_type.to_string(),
            ..Default::default()
        };

        assert!(!field.is_fillable(), "{input_type} should not be fillable");
        assert_eq!(score(&field, FieldKind::Email), 0, "{input_type}");
        assert!(plan(&profile(), &[field]).fills.is_empty(), "{input_type}");
    }
}

#[test]
fn ordinary_text_inputs_are_fillable() {
    for input_type in ["text", "email", "tel", "date", "search", ""] {
        let field = FormField {
            input_type: input_type.to_string(),
            ..Default::default()
        };
        assert!(field.is_fillable(), "{input_type:?} should be fillable");
    }
}

// -------------------------------------------------------------------
// Whole forms
// -------------------------------------------------------------------

#[test]
fn a_typical_opt_out_form_fills_completely() {
    let fields = [
        named("first_name"),
        named("last_name"),
        named("email"),
        named("phone"),
        named("address"),
        named("city"),
        named("state"),
        named("zip"),
    ];

    let plan = plan(&profile(), &fields);

    assert_eq!(plan.fills.len(), 8);
    assert!(plan.unrecognized.is_empty());
    assert_eq!(filled(&plan, FieldKind::FirstName).unwrap().value, "Jane");
    assert_eq!(
        filled(&plan, FieldKind::Address).unwrap().value,
        "123 Main Street"
    );
    assert_eq!(filled(&plan, FieldKind::ZipCode).unwrap().value, "94102");
}

/// Go tried each mapping against its own selector list independently, so a
/// box named `email_address` matched the email mapping and then the address
/// mapping — and the second overwrote the first with a street address.
#[test]
fn a_field_named_email_address_gets_the_email_and_not_the_street() {
    let plan = plan(&profile(), &[named("email_address")]);

    assert_eq!(plan.fills.len(), 1, "one box, one value");
    assert_eq!(plan.fills[0].kind, FieldKind::Email);
    assert_eq!(plan.fills[0].value, "jane@example.com");
}

/// The same guard from the other direction: with both boxes present, each
/// gets its own value.
#[test]
fn an_email_box_and_a_street_box_get_different_values() {
    let plan = plan(
        &profile(),
        &[named("email_address"), named("street_address")],
    );

    assert_eq!(
        filled(&plan, FieldKind::Email).unwrap().value,
        "jane@example.com"
    );
    assert_eq!(
        filled(&plan, FieldKind::Address).unwrap().value,
        "123 Main Street"
    );
}

/// A form with one name box should get the whole name, not just the first.
#[test]
fn a_single_name_box_gets_the_full_name() {
    let plan = plan(&profile(), &[named("name")]);

    assert_eq!(plan.fills.len(), 1);
    assert_eq!(plan.fills[0].kind, FieldKind::FullName);
    assert_eq!(plan.fills[0].value, "Jane Doe");
}

/// With both halves present, the full name should not also be typed
/// somewhere it does not belong.
#[test]
fn split_name_boxes_are_filled_separately() {
    let plan = plan(&profile(), &[named("first_name"), named("last_name")]);

    assert_eq!(filled(&plan, FieldKind::FirstName).unwrap().value, "Jane");
    assert_eq!(filled(&plan, FieldKind::LastName).unwrap().value, "Doe");
    assert!(filled(&plan, FieldKind::FullName).is_none());
}

/// Two boxes cannot both be the email.
#[test]
fn a_kind_fills_at_most_one_box() {
    let plan = plan(&profile(), &[named("email"), named("email_confirm")]);

    let emails = plan
        .fills
        .iter()
        .filter(|fill| fill.kind == FieldKind::Email)
        .count();
    assert_eq!(emails, 1);
}

#[test]
fn fills_are_reported_in_page_order() {
    let plan = plan(
        &profile(),
        &[named("zip"), named("email"), named("first_name")],
    );

    let kinds: Vec<_> = plan.fills.iter().map(|fill| fill.kind).collect();
    assert_eq!(
        kinds,
        [FieldKind::ZipCode, FieldKind::Email, FieldKind::FirstName]
    );
}

/// The same form must produce the same plan every time, or a re-run fills
/// different boxes than the screenshot showed.
#[test]
fn planning_is_deterministic() {
    let fields = [
        named("name"),
        named("email"),
        named("address"),
        named("contact"),
    ];

    let first = plan(&profile(), &fields);
    for _ in 0..10 {
        assert_eq!(plan(&profile(), &fields), first);
    }
}

// -------------------------------------------------------------------
// Incomplete profiles
// -------------------------------------------------------------------

#[test]
fn a_field_the_profile_cannot_answer_is_reported_not_guessed() {
    let sparse = Profile {
        first_name: "Jane".into(),
        last_name: "Doe".into(),
        email: "jane@example.com".into(),
        ..Default::default()
    };

    let plan = plan(&sparse, &[named("email"), named("phone"), named("zip")]);

    assert_eq!(plan.fills.len(), 1);
    assert_eq!(plan.unanswered.len(), 2);
    assert!(
        plan.unrecognized.is_empty(),
        "they were recognised, just unanswerable"
    );
}

/// A recognised box the profile cannot answer must not be filled with a
/// weaker guess — a blank phone box is better than one holding a postcode.
#[test]
fn an_unanswerable_field_is_not_given_to_a_weaker_match() {
    let no_phone = Profile {
        phone: String::new(),
        ..profile()
    };

    let plan = plan(&no_phone, &[named("phone")]);

    assert!(plan.fills.is_empty());
    assert_eq!(plan.unanswered.len(), 1);
}

/// A form that will refuse to submit is worth knowing about before trying.
#[test]
fn a_required_field_with_no_answer_is_flagged() {
    let sparse = Profile {
        first_name: "Jane".into(),
        last_name: "Doe".into(),
        email: "jane@example.com".into(),
        ..Default::default()
    };

    let required_phone = FormField {
        required: true,
        ..named("phone")
    };

    let incomplete = plan(&sparse, &[named("email"), required_phone]);
    assert!(incomplete.has_unanswered_required());

    let complete = plan(&profile(), &[named("email"), named("phone")]);
    assert!(!complete.has_unanswered_required());
}

#[test]
fn an_empty_form_plans_nothing() {
    let plan = plan(&profile(), &[]);

    assert!(plan.is_empty());
    assert!(plan.unanswered.is_empty());
    assert!(plan.unrecognized.is_empty());
}

#[test]
fn an_empty_profile_fills_nothing() {
    let plan = plan(&Profile::default(), &[named("email"), named("first_name")]);

    assert!(plan.is_empty());
    assert_eq!(plan.unanswered.len(), 2);
}

// -------------------------------------------------------------------
// Kinds
// -------------------------------------------------------------------

#[test]
fn every_kind_reads_its_value_off_the_profile() {
    let profile = profile();

    for kind in FieldKind::ALL {
        assert!(
            !kind.value_from(&profile).is_empty(),
            "{kind} has no value on a complete profile"
        );
        assert!(!kind.as_str().is_empty());
    }
}

#[test]
fn the_full_name_joins_the_two_halves() {
    assert_eq!(FieldKind::FullName.value_from(&profile()), "Jane Doe");

    let first_only = Profile {
        first_name: "Jane".into(),
        ..Default::default()
    };
    assert_eq!(FieldKind::FullName.value_from(&first_only), "Jane");
}
