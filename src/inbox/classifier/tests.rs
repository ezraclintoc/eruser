//! Classifier tests.
//!
//! The wording in these cases comes from `internal/inbox/classifier_test.go`,
//! which upstream built out of real broker replies. They are the closest
//! thing this project has to a regression corpus, so they are carried over
//! verbatim rather than reworded.

use super::*;

fn reply(body: &str) -> Email {
    Email {
        subject: "Re: Personal Data Removal Request".into(),
        body: body.into(),
        ..Default::default()
    }
}

fn reply_with_subject(subject: &str, body: &str) -> Email {
    Email {
        subject: subject.into(),
        body: body.into(),
        ..Default::default()
    }
}

#[track_caller]
fn assert_classifies(email: &Email, expected: ResponseType, case: &str) {
    let result = classify(email);
    assert_eq!(
        result.response_type, expected,
        "{case}: got {} at {:.2} confidence",
        result.response_type, result.confidence
    );
}

// -------------------------------------------------------------------
// Forms — real replies from Blackbaud, FamilyTreeNow, Civis, and others
// -------------------------------------------------------------------

#[test]
fn brokers_redirecting_to_a_form_are_recognised() {
    let cases = [
        (
            "Blackbaud",
            "please submit your request at https://www.blackbaud.com/company/Data-Subject-Rights-Request",
        ),
        (
            "FamilyTreeNow",
            "please use our Opt-Out Form, linked below, to submit your request",
        ),
        (
            "Civis",
            "please complete the Data Subject Request Form, which you can find at",
        ),
        (
            "Affinity",
            "does not accept privacy requests via email. You can submit a privacy request via our web form",
        ),
        (
            "33Across",
            "Please visit our opt-out page located at https://www.33across.com/opt-out",
        ),
        (
            "PeopleFinders",
            "Right to Opt-out: If you wish to opt out of our website, you may do so at this link",
        ),
        (
            "01Advertising",
            "please click on the following link to submit your data deletion request",
        ),
        (
            "Mediaocean",
            "we have established a dedicated online form that helps us verify your identity",
        ),
        (
            "no email processing",
            "We do not process requests via email alone. Please submit via our web form.",
        ),
        (
            "customer service",
            "Dear Customer, Please send your request to Customer Service at custserv@example.com",
        ),
    ];

    for (case, body) in cases {
        assert_classifies(&reply(body), ResponseType::FormRequired, case);
    }
}

// -------------------------------------------------------------------
// Confirmation links
// -------------------------------------------------------------------

#[test]
fn replies_asking_for_a_click_are_recognised() {
    let cases = [
        ("click here", "Please click here to confirm your request"),
        ("click below", "Click below to verify your email address"),
        (
            "confirm email",
            "Please confirm your email address to process your request",
        ),
        (
            "verification link",
            "We have sent you a verification link. Please click it to continue.",
        ),
        (
            "click to confirm",
            "Click to confirm your data removal request",
        ),
        (
            "verify identity",
            "For security purposes, please verify your identity by clicking the link below",
        ),
        (
            "confirm with link",
            "Click the link to confirm your request: https://example.com/confirm/abc123",
        ),
        (
            "verify last 4",
            "Can you please verify last 4 of your social? We have several individuals with your name.",
        ),
    ];

    for (case, body) in cases {
        assert_classifies(&reply(body), ResponseType::ConfirmationRequired, case);
    }
}

// -------------------------------------------------------------------
// Refusals and "we hold nothing"
// -------------------------------------------------------------------

#[test]
fn refusals_are_recognised() {
    let cases = [
        (
            "ACUTRAQ",
            "we do not have any record of a report in our system. ACUTRAQ maintains no files on this person.",
        ),
        (
            "Atlantic Fox",
            "Atlantic Fox is no longer registered as an active data broker in any jurisdictions",
        ),
        (
            "Checkr",
            "The privacy@checkr.com email list is no longer in use",
        ),
        (
            "no data linked",
            "We have no data linked to your email in our system.",
        ),
        (
            "b2b platform",
            "Tyler - we are a b2b platform and do not have your information.",
        ),
        (
            "never existed",
            "the information below has never existed in our database.",
        ),
        (
            "FCRA exempt",
            "consumer reporting agencies are exempt as Fair Credit Reporting Act related data so we do not remove data by request.",
        ),
        (
            "not identified",
            "Your email was not identified in our database. This means that we don't have any personal information.",
        ),
    ];

    for (case, body) in cases {
        assert_classifies(&reply(body), ResponseType::Rejected, case);
    }
}

#[test]
fn rejections_in_the_subject_line_are_recognised() {
    let cases = [
        ("Not Found - Personal Data Removal Request", ""),
        ("No record of your request", ""),
        ("Unable to locate your information", ""),
    ];

    for (subject, body) in cases {
        assert_classifies(
            &reply_with_subject(subject, body),
            ResponseType::Rejected,
            subject,
        );
    }
}

// -------------------------------------------------------------------
// Acknowledgements and auto-replies
// -------------------------------------------------------------------

#[test]
fn acknowledgements_are_recognised() {
    let cases = [
        (
            "Your Request Has Been Received",
            "we have received your request. One of our Privacy Specialists will reach out",
        ),
        (
            "Request Received - [#REQ-195698]",
            "ticket has been created. Please reference #REQ-195698",
        ),
        (
            "Automatic reply: Personal Data Removal Request",
            "Thank you for contacting us.",
        ),
        (
            "Out of Office Re: Personal Data Removal Request",
            "I am currently out of the office",
        ),
        (
            "Re: Personal Data Removal Request",
            "Thank you for your inquiry. This email confirms that we have received your request.",
        ),
    ];

    for (subject, body) in cases {
        assert_classifies(
            &reply_with_subject(subject, body),
            ResponseType::Pending,
            subject,
        );
    }
}

/// Several brokers reply with nothing but a ticket number in the subject.
#[test]
fn a_ticket_number_in_the_subject_reads_as_an_acknowledgement() {
    for subject in [
        "Support Request #123456",
        "Request Received - [#REQ-195698]",
        "Your ticket (259135) has been logged",
        "Thanks for reaching out",
    ] {
        assert_classifies(
            &reply_with_subject(subject, "See attached."),
            ResponseType::Pending,
            subject,
        );
    }
}

// -------------------------------------------------------------------
// Success
// -------------------------------------------------------------------

#[test]
fn completed_removals_are_recognised() {
    let cases = [
        (
            "we have removed",
            "We have removed your information from our database.",
        ),
        (
            "successfully deleted",
            "Your data has been successfully deleted.",
        ),
        ("request completed", "Your request has been completed."),
        ("no longer hold", "We no longer hold your data."),
    ];

    for (case, body) in cases {
        assert_classifies(&reply(body), ResponseType::Success, case);
    }
}

// -------------------------------------------------------------------
// Bounces
// -------------------------------------------------------------------

#[test]
fn a_delivery_failure_is_recognised_as_a_bounce() {
    let bounce = Email {
        from: "mailer-daemon@googlemail.com".into(),
        from_name: "Mail Delivery Subsystem".into(),
        subject: "Delivery Status Notification (Failure)".into(),
        body: "Your message could not be delivered to: privacy@deadbroker.example\n\
               The response was: 550 5.1.1 The email account does not exist."
            .into(),
        ..Default::default()
    };

    let result = classify(&bounce);
    assert_eq!(result.response_type, ResponseType::Bounced);
    assert_eq!(
        result.bounced_recipient.as_deref(),
        Some("privacy@deadbroker.example")
    );
    assert!(!result.needs_review, "a bounce is unambiguous");
}

/// A bounce quotes the original request back, so the request's own wording
/// scores as whatever the request said. Checking for a bounce first is what
/// stops a failed delivery from being filed as a successful removal.
#[test]
fn a_bounce_quoting_the_original_request_is_still_a_bounce() {
    let bounce = Email {
        from: "postmaster@acme.example".into(),
        subject: "Undeliverable: Personal Data Removal Request".into(),
        body: "Delivery has failed to these recipients: privacy@acme.example\n\n\
               ----- Original message -----\n\
               I formally request that you remove all personal information \
               associated with me and confirm the completion of this removal."
            .into(),
        ..Default::default()
    };

    assert_eq!(classify(&bounce).response_type, ResponseType::Bounced);
}

/// Wording alone, without a mail-system sender, needs to be conclusive.
#[test]
fn a_single_bounce_phrase_from_a_person_is_not_a_bounce() {
    let reply = Email {
        from: "privacy@acme.example".into(),
        subject: "Re: Personal Data Removal Request".into(),
        body: "Our previous message to you was undeliverable, so we are writing again. \
               We have removed your information."
            .into(),
        ..Default::default()
    };

    assert_ne!(classify(&reply).response_type, ResponseType::Bounced);
}

// -------------------------------------------------------------------
// The test message eruser sends during setup
// -------------------------------------------------------------------

#[test]
fn erusers_own_test_message_is_not_treated_as_a_broker_reply() {
    let cases = [
        ("eruser is set up", "This is the test message from eruser."),
        ("Eraser Test Email", "Checking configuration"),
        (
            "Re: something",
            "This is a test email from eruser to check your settings",
        ),
    ];

    for (subject, body) in cases {
        let result = classify(&reply_with_subject(subject, body));
        assert_eq!(result.response_type, ResponseType::Success, "{subject}");
        assert_eq!(result.confidence, 1.0);
        assert!(!result.needs_review);
    }
}

// -------------------------------------------------------------------
// Scoring, confidence, and review
// -------------------------------------------------------------------

#[test]
fn a_reply_matching_nothing_is_unknown_and_flagged() {
    let result = classify(&reply("Hello. Regards, the team."));

    assert_eq!(result.response_type, ResponseType::Unknown);
    assert_eq!(result.confidence, 0.0);
    assert!(result.needs_review, "an unreadable reply needs a person");
}

/// Go chose the winner while ranging over a map, whose order is randomised,
/// so an email scoring equally in two categories classified differently from
/// one run to the next — and the stored classification depended on when the
/// mailbox happened to be scanned.
#[test]
fn classification_is_stable_across_runs() {
    let email = reply(
        "We have received your request. Please complete the form at \
         https://acme.example/opt-out and we will remove your data.",
    );

    let first = classify(&email).response_type;
    for _ in 0..25 {
        assert_eq!(classify(&email).response_type, first);
    }
}

#[test]
fn a_clear_single_category_match_is_confident() {
    let result = classify(&reply(
        "We have removed your information from our database.",
    ));

    assert_eq!(result.response_type, ResponseType::Success);
    assert!(result.confidence >= 0.85, "{}", result.confidence);
    assert!(!result.needs_review);
}

/// A link is harder evidence than a phrase.
#[test]
fn a_form_link_raises_confidence() {
    let with_link = classify(&reply(
        "Please use our opt-out form: https://acme.example/opt-out",
    ));

    assert_eq!(with_link.response_type, ResponseType::FormRequired);
    assert!(with_link.confidence >= 0.85, "{}", with_link.confidence);
    assert_eq!(
        with_link.form_url.as_deref(),
        Some("https://acme.example/opt-out")
    );
}

/// Some brokers send a bare link with no explanation at all. The link
/// itself scores, so it classifies on that alone.
#[test]
fn a_reply_with_only_a_link_is_classified_from_the_link() {
    let form_only = classify(&reply("https://acme.example/do-not-sell"));
    assert_eq!(form_only.response_type, ResponseType::FormRequired);
    assert!(form_only.confidence >= 0.85);

    let confirm_only = classify(&reply("https://acme.example/confirm?token=abc123"));
    assert_eq!(
        confirm_only.response_type,
        ResponseType::ConfirmationRequired
    );
}

#[test]
fn the_primary_links_are_carried_on_the_result() {
    let result = classify(&Email {
        from_domain: "acme.example".into(),
        subject: "Re: Personal Data Removal Request".into(),
        body: "Confirm at https://acme.example/confirm?token=abc \
               or use the form at https://acme.example/opt-out"
            .into(),
        ..Default::default()
    });

    assert_eq!(
        result.form_url.as_deref(),
        Some("https://acme.example/opt-out")
    );
    assert_eq!(
        result.confirm_url.as_deref(),
        Some("https://acme.example/confirm?token=abc")
    );
}

#[test]
fn every_classification_carries_a_reason() {
    for body in [
        "We have removed your data.",
        "Please complete our opt-out form.",
        "Click here to confirm your request.",
        "We have no record of you.",
        "Your request has been received.",
        "Nothing matches this at all.",
    ] {
        let result = classify(&reply(body));
        assert!(!result.reason.is_empty(), "{body:?} produced no reason");
    }
}

#[test]
fn actionable_replies_are_the_ones_needing_a_person() {
    assert!(ResponseType::FormRequired.is_actionable());
    assert!(ResponseType::ConfirmationRequired.is_actionable());

    assert!(!ResponseType::Success.is_actionable());
    assert!(!ResponseType::Rejected.is_actionable());
    assert!(!ResponseType::Pending.is_actionable());
    assert!(!ResponseType::Bounced.is_actionable());
}

// -------------------------------------------------------------------
// Subject-only classification, for re-reading stored replies
// -------------------------------------------------------------------

#[test]
fn a_strong_subject_classifies_without_a_body() {
    let (response_type, confidence, needs_review) =
        classify_by_subject("Your Request Has Been Received");

    assert_eq!(response_type, ResponseType::Pending);
    assert!(confidence >= 0.7);
    assert!(!needs_review);
}

#[test]
fn a_subject_matching_nothing_stays_unknown() {
    let (response_type, confidence, needs_review) = classify_by_subject("Hello");

    assert_eq!(response_type, ResponseType::Unknown);
    assert_eq!(confidence, 0.0);
    assert!(needs_review);
}

/// A subject is a fraction of the evidence, so it never reaches the
/// confidence a full classification can.
#[test]
fn subject_only_confidence_is_capped_below_full_classification() {
    let (_, subject_only, _) = classify_by_subject("Your data has been removed");
    let full = classify(&reply("Your data has been removed")).confidence;

    assert!(subject_only <= 0.7);
    assert!(full > subject_only, "{full} should beat {subject_only}");
}

// -------------------------------------------------------------------
// Summaries
// -------------------------------------------------------------------

#[test]
fn a_batch_summary_counts_every_category() {
    let responses: Vec<_> = [
        "We have removed your information.",
        "Please complete our opt-out form at https://acme.example/opt-out",
        "Click here to confirm your request.",
        "We have no record of you.",
        "Your request has been received.",
        "Nothing matches this at all.",
    ]
    .iter()
    .map(|body| classify(&reply(body)))
    .collect();

    let summary = summarize(&responses);

    assert_eq!(summary.total, 6);
    assert_eq!(summary.success, 1);
    assert_eq!(summary.form_required, 1);
    assert_eq!(summary.confirmation_required, 1);
    assert_eq!(summary.rejected, 1);
    assert_eq!(summary.pending, 1);
    assert_eq!(summary.unknown, 1);
    assert_eq!(summary.needs_review, 1, "only the unreadable one");
}

#[test]
fn an_empty_batch_summarizes_to_zero() {
    assert_eq!(summarize(&[]), Summary::default());
}

// -------------------------------------------------------------------
// Storage
// -------------------------------------------------------------------

#[test]
fn every_response_type_maps_onto_a_stored_one() {
    let cases = [
        (ResponseType::Success, crate::history::ResponseType::Success),
        (ResponseType::Bounced, crate::history::ResponseType::Bounced),
        (
            ResponseType::FormRequired,
            crate::history::ResponseType::FormRequired,
        ),
        (
            ResponseType::ConfirmationRequired,
            crate::history::ResponseType::ConfirmationRequired,
        ),
        (
            ResponseType::Rejected,
            crate::history::ResponseType::Rejected,
        ),
        (ResponseType::Pending, crate::history::ResponseType::Pending),
        (ResponseType::Unknown, crate::history::ResponseType::Unknown),
    ];

    for (from, expected) in cases {
        assert_eq!(crate::history::ResponseType::from(from), expected);
        // The names have to agree, since one is written and the other read.
        assert_eq!(from.as_str(), expected.as_str());
    }
}
