use super::*;
use std::sync::Mutex;

/// A sender that records what it was asked to send. Used here and by the
/// send-pipeline tests, which must never touch a real mail server.
#[derive(Debug, Default)]
pub struct RecordingSender {
    sent: Mutex<Vec<Message>>,
    /// Recipient addresses that should fail, to exercise error paths.
    fail_for: Vec<String>,
}

impl RecordingSender {
    pub fn failing_for(addresses: &[&str]) -> Self {
        Self {
            sent: Mutex::new(Vec::new()),
            fail_for: addresses.iter().map(|s| s.to_string()).collect(),
        }
    }

    pub fn sent(&self) -> Vec<Message> {
        self.sent.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl Sender for RecordingSender {
    async fn send(&self, message: &Message) -> Result<Sent, Error> {
        validate_message(message)?;
        if self.fail_for.contains(&message.to) {
            return Err(Error::Rejected("5.1".into()));
        }
        self.sent.lock().unwrap().push(message.clone());
        Ok(Sent {
            message_id: generate_message_id(&message.from),
            response: "250 OK".into(),
        })
    }

    fn name(&self) -> &'static str {
        "recording"
    }
}

fn message() -> Message {
    Message {
        to: "privacy@acme.example".into(),
        from: "jane@example.com".into(),
        subject: "Personal Data Removal Request".into(),
        body: "Please remove my data.".into(),
    }
}

fn smtp_config() -> SmtpConfig {
    SmtpConfig {
        host: "smtp.example.com".into(),
        port: 465,
        username: "jane@example.com".into(),
        password: "app-password".into(),
        use_tls: true,
    }
}

use crate::config::{EmailConfig, SmtpConfig};

#[test]
fn validate_email_accepts_ordinary_addresses() {
    assert!(validate_email("privacy@acme.example").is_ok());
    assert!(validate_email("Jane Doe <jane@example.com>").is_ok());
    assert!(validate_email("first.last+tag@sub.example.co.uk").is_ok());
}

#[test]
fn validate_email_rejects_header_injection() {
    for bad in [
        "a@b.example\r\nBcc: attacker@evil.example",
        "a@b.example\nBcc: attacker@evil.example",
        "a@b.example, attacker@evil.example",
        "a@b.example; attacker@evil.example",
    ] {
        assert!(
            matches!(
                validate_email(bad),
                Err(ValidationError::IllegalCharacters(_))
            ),
            "{bad:?} should have been rejected"
        );
    }
}

#[test]
fn validate_email_rejects_malformed_addresses() {
    for bad in ["", "not-an-address", "@example.com", "a@"] {
        assert!(
            matches!(validate_email(bad), Err(ValidationError::Malformed(_))),
            "{bad:?} should have been rejected"
        );
    }
}

#[test]
fn validate_message_rejects_a_subject_with_a_line_break() {
    let mut msg = message();
    msg.subject = "Removal Request\r\nBcc: attacker@evil.example".into();
    assert_eq!(
        validate_message(&msg),
        Err(ValidationError::SubjectLineBreak)
    );
}

#[test]
fn validate_message_distinguishes_sender_from_recipient() {
    let mut msg = message();
    msg.from = "bogus".into();
    assert!(matches!(
        validate_message(&msg),
        Err(ValidationError::Sender(_))
    ));

    let mut msg = message();
    msg.to = "bogus".into();
    assert!(matches!(
        validate_message(&msg),
        Err(ValidationError::Recipient(_))
    ));
}

#[test]
fn message_ids_are_unique_and_use_the_sender_domain() {
    let a = generate_message_id("jane@example.com");
    let b = generate_message_id("jane@example.com");
    assert_ne!(a, b);
    assert!(a.starts_with('<') && a.ends_with('>'));
    assert!(a.ends_with("@example.com>"), "{a}");
}

#[test]
fn message_id_falls_back_when_the_sender_has_no_domain() {
    assert!(generate_message_id("nonsense").ends_with("@eruser.local>"));
}

#[tokio::test]
async fn new_sender_builds_smtp_for_an_empty_or_smtp_provider() {
    for provider in ["", "smtp"] {
        let config = EmailConfig {
            provider: provider.into(),
            from: "jane@example.com".into(),
            smtp: smtp_config(),
            ..Default::default()
        };
        assert_eq!(new_sender(&config).unwrap().name(), "smtp");
    }
}

#[tokio::test]
async fn new_sender_rejects_a_provider_that_does_not_exist() {
    let config = EmailConfig {
        provider: "carrier-pigeon".into(),
        from: "jane@example.com".into(),
        smtp: smtp_config(),
        ..Default::default()
    };
    match new_sender(&config) {
        Err(Error::UnknownProvider(p)) => assert_eq!(p, "carrier-pigeon"),
        Err(other) => panic!("wrong error: {other}"),
        Ok(sender) => panic!("expected a failure, got the {} sender", sender.name()),
    }
}

/// Sending credentials over an unencrypted connection would put the app
/// password on the wire in the clear.
#[test]
fn smtp_refuses_credentials_without_tls() {
    let config = SmtpConfig {
        use_tls: false,
        ..smtp_config()
    };
    assert!(matches!(
        SmtpSender::new(config, "jane@example.com".into()).unwrap_err(),
        Error::Configuration(_)
    ));
}

#[tokio::test]
async fn smtp_allows_an_unauthenticated_cleartext_relay() {
    let config = SmtpConfig {
        username: String::new(),
        password: String::new(),
        use_tls: false,
        port: 25,
        ..smtp_config()
    };
    assert!(SmtpSender::new(config, "jane@example.com".into()).is_ok());
}

#[test]
fn smtp_requires_a_host_and_port() {
    let config = SmtpConfig {
        host: String::new(),
        ..smtp_config()
    };
    assert!(matches!(
        SmtpSender::new(config, "jane@example.com".into()).unwrap_err(),
        Error::Configuration(_)
    ));

    let config = SmtpConfig {
        port: 0,
        ..smtp_config()
    };
    assert!(matches!(
        SmtpSender::new(config, "jane@example.com".into()).unwrap_err(),
        Error::Configuration(_)
    ));
}

#[tokio::test]
async fn smtp_debug_output_does_not_leak_the_password() {
    let sender = SmtpSender::new(smtp_config(), "jane@example.com".into()).unwrap();
    let debug = format!("{sender:?}");
    assert!(!debug.contains("app-password"), "{debug}");
}

#[tokio::test]
async fn dry_run_sender_reports_success_without_sending() {
    let sent = DryRunSender.send(&message()).await.unwrap();
    assert!(sent.message_id.is_empty());
    assert_eq!(DryRunSender.name(), "dry-run");
}

/// A dry run that accepts an address the real transport would reject is a
/// misleading preview.
#[tokio::test]
async fn dry_run_sender_still_validates() {
    let mut msg = message();
    msg.to = "a@b.example\r\nBcc: attacker@evil.example".into();
    assert!(matches!(
        DryRunSender.send(&msg).await.unwrap_err(),
        Error::Invalid(_)
    ));
}

#[tokio::test]
async fn recording_sender_captures_messages_and_can_fail() {
    let sender = RecordingSender::failing_for(&["blocked@example.com"]);
    sender.send(&message()).await.unwrap();

    let mut blocked = message();
    blocked.to = "blocked@example.com".into();
    assert!(sender.send(&blocked).await.is_err());

    assert_eq!(sender.sent().len(), 1);
    assert_eq!(sender.sent()[0].to, "privacy@acme.example");
}
