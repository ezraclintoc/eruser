use super::*;

fn full_profile() -> Profile {
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

fn minimal_profile() -> Profile {
    Profile {
        first_name: "Jane".into(),
        last_name: "Doe".into(),
        email: "jane@example.com".into(),
        ..Default::default()
    }
}

fn test_broker() -> Broker {
    Broker {
        id: "acme".into(),
        name: "Acme Data".into(),
        email: "privacy@acme.example".into(),
        website: "https://acme.example".into(),
        opt_out_url: "https://acme.example/optout".into(),
        region: "us".into(),
        category: "people-search".into(),
        notes: String::new(),
        requires_id: false,
        tags: Vec::new(),
    }
}

#[test]
fn all_embedded_templates_parse() {
    let engine = Engine::new().expect("embedded templates must parse");
    assert_eq!(engine.available_templates(), ["ccpa", "gdpr", "generic"]);
}

#[test]
fn every_template_renders_with_a_full_profile() {
    let engine = Engine::new().unwrap();
    for name in engine.available_templates() {
        let email = engine
            .render(&name, &full_profile(), &test_broker())
            .unwrap_or_else(|e| panic!("{name} failed to render: {e}"));
        assert!(!email.subject.is_empty(), "{name} produced no subject");
        assert!(
            email.body.len() > 200,
            "{name} produced a suspiciously short body"
        );
        assert!(
            !email.body.contains("{{") && !email.body.contains("{%"),
            "{name} left unrendered template syntax in the body"
        );
    }
}

#[test]
fn every_template_renders_with_only_required_fields() {
    let engine = Engine::new().unwrap();
    for name in engine.available_templates() {
        let email = engine
            .render(&name, &minimal_profile(), &test_broker())
            .unwrap_or_else(|e| panic!("{name} failed to render: {e}"));
        assert!(email.body.contains("Jane Doe"));
    }
}

#[test]
fn optional_fields_are_omitted_when_blank() {
    let engine = Engine::new().unwrap();
    let email = engine
        .render("generic", &minimal_profile(), &test_broker())
        .unwrap();

    assert!(!email.body.contains("- Address:"));
    assert!(!email.body.contains("- City:"));
    assert!(!email.body.contains("- Phone:"));
    assert!(!email.body.contains("- Date of Birth:"));
    // The blank lines around the omitted block should not pile up.
    assert!(!email.body.contains("\n\n\n"));
}

#[test]
fn optional_fields_are_included_when_present() {
    let engine = Engine::new().unwrap();
    let email = engine
        .render("generic", &full_profile(), &test_broker())
        .unwrap();

    assert!(email.body.contains("- Address: 123 Main Street"));
    assert!(email.body.contains("- City: San Francisco"));
    assert!(email.body.contains("- Postal Code: 94102"));
    assert!(email.body.contains("- Phone: +1-555-123-4567"));
    assert!(email.body.contains("- Date of Birth: 1990-01-15"));
}

#[test]
fn body_is_addressed_to_the_broker_and_signed_by_the_user() {
    let engine = Engine::new().unwrap();
    let email = engine
        .render("generic", &full_profile(), &test_broker())
        .unwrap();

    assert!(
        email
            .body
            .starts_with("To Whom It May Concern at Acme Data,")
    );
    assert!(email.body.contains("Sincerely,\nJane Doe"));
}

#[test]
fn gdpr_template_cites_article_17() {
    let engine = Engine::new().unwrap();
    let email = engine
        .render("gdpr", &full_profile(), &test_broker())
        .unwrap();
    assert!(email.body.contains("Article 17"));
    assert!(email.subject.contains("GDPR"));
}

#[test]
fn ccpa_template_cites_california_law() {
    let engine = Engine::new().unwrap();
    let email = engine
        .render("ccpa", &full_profile(), &test_broker())
        .unwrap();
    assert!(email.body.contains("1798.105"));
    assert!(email.subject.contains("CCPA"));
}

#[test]
fn subjects_are_distinct_per_template() {
    let engine = Engine::new().unwrap();
    let subjects: Vec<String> = engine
        .available_templates()
        .iter()
        .map(|n| {
            engine
                .render(n, &full_profile(), &test_broker())
                .unwrap()
                .subject
        })
        .collect();

    let unique: std::collections::HashSet<_> = subjects.iter().collect();
    assert_eq!(
        unique.len(),
        subjects.len(),
        "subjects collide: {subjects:?}"
    );
}

/// A subject reaching the SMTP layer with a newline in it would be a header
/// injection. Subjects are constants, so this can never regress silently.
#[test]
fn subjects_contain_no_line_breaks() {
    let engine = Engine::new().unwrap();
    for name in engine.available_templates() {
        let subject = engine
            .render(&name, &full_profile(), &test_broker())
            .unwrap()
            .subject;
        assert!(!subject.contains('\r') && !subject.contains('\n'), "{name}");
    }
}

#[test]
fn unknown_template_is_rejected() {
    let engine = Engine::new().unwrap();
    let err = engine
        .render("nonexistent", &full_profile(), &test_broker())
        .unwrap_err();
    assert!(matches!(err, Error::Unknown(name) if name == "nonexistent"));
}

#[test]
fn has_template_reports_membership() {
    let engine = Engine::new().unwrap();
    assert!(engine.has_template("gdpr"));
    assert!(!engine.has_template("hipaa"));
}

#[test]
fn every_template_has_a_description() {
    let engine = Engine::new().unwrap();
    let descriptions = Engine::descriptions();
    for name in engine.available_templates() {
        assert!(
            descriptions.contains_key(name.as_str()),
            "{name} has no description"
        );
    }
}

#[test]
fn default_template_exists() {
    assert!(Engine::new().unwrap().has_template(DEFAULT_TEMPLATE));
}

/// Names and addresses routinely contain apostrophes and ampersands. HTML
/// escaping them would produce "O&#x27;Brien" in a plain-text email.
#[test]
fn plain_text_output_is_not_html_escaped() {
    let engine = Engine::new().unwrap();
    let profile = Profile {
        last_name: "O'Brien & Sons".into(),
        ..minimal_profile()
    };
    let email = engine.render("generic", &profile, &test_broker()).unwrap();
    assert!(email.body.contains("Jane O'Brien & Sons"));
    assert!(!email.body.contains("&#"));
    assert!(!email.body.contains("&amp;"));
}

#[test]
fn email_data_formats_the_date_like_the_go_version() {
    let data = EmailData::new(&full_profile(), &test_broker(), "generic");
    // e.g. "August 19, 2026" — no zero padding on the day.
    assert!(
        data.date.contains(&data.year.to_string()),
        "date {:?} should contain the year",
        data.date
    );
    assert!(
        data.date.contains(", "),
        "date {:?} should be long-form",
        data.date
    );
    let day = data.date.split(' ').nth(1).unwrap().trim_end_matches(',');
    assert!(
        !day.starts_with('0'),
        "day {day:?} should not be zero padded"
    );
    assert_eq!(data.month, data.date.split(' ').next().unwrap());
}

#[test]
fn broker_fields_reach_the_data_struct() {
    let data = EmailData::new(&full_profile(), &test_broker(), "gdpr");
    assert_eq!(data.broker_name, "Acme Data");
    assert_eq!(data.broker_email, "privacy@acme.example");
    assert_eq!(data.broker_website, "https://acme.example");
    assert_eq!(data.broker_opt_out, "https://acme.example/optout");
    assert_eq!(data.template, "gdpr");
}
