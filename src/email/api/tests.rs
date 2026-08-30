use super::*;

use crate::config::{ApiKeyConfig, EmailConfig};

fn message() -> Message {
    Message {
        to: "privacy@acme.example".into(),
        from: "jane@example.com".into(),
        subject: "Personal Data Removal Request".into(),
        body: "Please remove my data.".into(),
    }
}

fn sender(provider: Provider) -> ApiSender {
    ApiSender::new(provider, "test-key".into(), "jane@example.com".into())
        .expect("the sender should build")
}

// -------------------------------------------------------------------
// Configuration
// -------------------------------------------------------------------

/// The point of these providers is that setup is one value, so a missing
/// key should say where to get one rather than failing at send time.
#[test]
fn a_missing_key_says_where_to_get_one() {
    for provider in [Provider::Resend, Provider::SendGrid] {
        let error = ApiSender::new(provider, String::new(), "jane@example.com".into())
            .expect_err("an empty key should be refused");

        let message = error.to_string();
        assert!(message.contains(provider.as_str()), "{message}");
        assert!(message.contains("https://"), "{message}");
    }
}

#[test]
fn a_whitespace_only_key_is_treated_as_missing() {
    assert!(ApiSender::new(Provider::Resend, "   ".into(), "a@b.example".into()).is_err());
}

#[test]
fn providers_are_recognised_by_name() {
    assert_eq!(Provider::from_name("resend"), Some(Provider::Resend));
    assert_eq!(Provider::from_name("sendgrid"), Some(Provider::SendGrid));
    assert_eq!(Provider::from_name("smtp"), None);
    assert_eq!(Provider::from_name(""), None);
}

#[test]
fn debug_output_does_not_leak_the_key() {
    let debug = format!("{:?}", sender(Provider::Resend));
    assert!(!debug.contains("test-key"), "{debug}");
    assert!(debug.contains("jane@example.com"));
}

#[test]
fn new_sender_builds_each_api_provider_from_the_config() {
    let config = |provider: &str, key: &str| EmailConfig {
        provider: provider.into(),
        from: "jane@example.com".into(),
        resend: ApiKeyConfig {
            api_key: key.into(),
        },
        sendgrid: ApiKeyConfig {
            api_key: key.into(),
        },
        ..Default::default()
    };

    assert_eq!(
        crate::email::new_sender(&config("resend", "re_abc"))
            .unwrap()
            .name(),
        "resend"
    );
    assert_eq!(
        crate::email::new_sender(&config("sendgrid", "SG.abc"))
            .unwrap()
            .name(),
        "sendgrid"
    );
}

// -------------------------------------------------------------------
// Request shapes
//
// These go to someone else's API, so getting the shape wrong is only
// discovered against a live account. Checking it here is free.
// -------------------------------------------------------------------

#[test]
fn the_resend_request_has_the_shape_resend_expects() {
    let body = sender(Provider::Resend).body(&message(), "<id@example.com>");

    assert_eq!(body["from"], "jane@example.com");
    assert_eq!(body["to"][0], "privacy@acme.example");
    assert_eq!(body["subject"], "Personal Data Removal Request");
    assert_eq!(body["text"], "Please remove my data.");
}

#[test]
fn the_sendgrid_request_has_the_shape_sendgrid_expects() {
    let body = sender(Provider::SendGrid).body(&message(), "<id@example.com>");

    assert_eq!(
        body["personalizations"][0]["to"][0]["email"],
        "privacy@acme.example"
    );
    assert_eq!(body["from"]["email"], "jane@example.com");
    assert_eq!(body["subject"], "Personal Data Removal Request");
    assert_eq!(body["content"][0]["type"], "text/plain");
    assert_eq!(body["content"][0]["value"], "Please remove my data.");
}

/// The generated Message-ID is what later lets a broker's reply be matched
/// to the request it answers, so it has to survive the provider.
#[test]
fn both_providers_carry_the_message_id_through() {
    for provider in [Provider::Resend, Provider::SendGrid] {
        let body = sender(provider).body(&message(), "<abc@example.com>");
        assert_eq!(
            body["headers"]["Message-ID"], "<abc@example.com>",
            "{provider} dropped the Message-ID"
        );
    }
}

#[test]
fn the_body_is_plain_text_not_html() {
    let resend = sender(Provider::Resend).body(&message(), "<id@x>");
    assert!(resend.get("html").is_none());

    let sendgrid = sender(Provider::SendGrid).body(&message(), "<id@x>");
    assert_eq!(sendgrid["content"][0]["type"], "text/plain");
}

// -------------------------------------------------------------------
// Responses
// -------------------------------------------------------------------

#[test]
fn a_rejected_key_is_reported_as_an_authentication_problem() {
    assert!(matches!(classify_status(401, ""), Error::Authentication));
    assert!(matches!(classify_status(403, ""), Error::Authentication));
}

/// A provider will not send from an address you have not proved you own,
/// and it is the commonest first failure with these services.
#[test]
fn an_unverified_sender_address_says_what_to_do() {
    let error = classify_status(422, r#"{"message":"The from domain is not verified"}"#);
    let message = error.to_string();

    assert!(message.contains("verify"), "{message}");
    assert!(
        message.contains("domain") || message.contains("sender"),
        "{message}"
    );
}

#[test]
fn rate_limiting_says_to_try_later() {
    assert!(classify_status(429, "").to_string().contains("later"));
}

#[test]
fn a_provider_outage_reads_as_a_connection_problem() {
    assert!(matches!(classify_status(500, ""), Error::Connection));
    assert!(matches!(classify_status(503, ""), Error::Connection));
}

/// The provider's own body can echo back the From address and parts of the
/// request, and these strings reach the UI and the logs.
#[test]
fn the_providers_response_body_is_not_passed_through() {
    let leaky = r#"{"message":"invalid","from":"jane@example.com","key":"re_secret"}"#;
    let shown = classify_status(400, leaky).to_string();

    assert!(!shown.contains("re_secret"), "{shown}");
    assert!(!shown.contains("jane@example.com"), "{shown}");
}

#[test]
fn the_providers_own_id_is_kept_when_it_gives_one() {
    assert_eq!(
        provider_reference(r#"{"id":"49a3999c-0ce1-4ea6-ab68-afcd6dc2e794"}"#).as_deref(),
        Some("49a3999c-0ce1-4ea6-ab68-afcd6dc2e794")
    );
    assert!(provider_reference("").is_none());
    assert!(provider_reference("not json").is_none());
    assert!(provider_reference(r#"{"accepted":true}"#).is_none());
}

/// Nothing should reach the provider that would not have reached SMTP.
#[tokio::test]
async fn header_injection_is_refused_before_any_request_is_made() {
    let mut bad = message();
    bad.to = "a@b.example\r\nBcc: attacker@evil.example".into();

    assert!(matches!(
        sender(Provider::Resend).send(&bad).await.unwrap_err(),
        Error::Invalid(_)
    ));
}
