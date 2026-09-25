//! Scan tests.
//!
//! `scan` itself needs a live mailbox, so what is covered here is everything
//! around it: the stage a reply moves a broker to, what gets stored, and
//! re-reading stored replies after the patterns change.

use super::*;

use crate::history::{NewRecord, ResponseFilter, ResponseType as StoredType, Status, Store};

async fn store_with_request(broker_id: &str) -> Store {
    let store = Store::open_in_memory().await.expect("an in-memory store");
    store
        .add_record(&NewRecord::sent(
            broker_id,
            &format!("Broker {broker_id}"),
            &format!("privacy@{broker_id}.example"),
            "generic",
            "<id@example.com>",
        ))
        .await
        .expect("the request should record");
    store
}

fn reply(broker_id: &str, subject: &str, body: &str) -> Email {
    Email {
        from: format!("privacy@{broker_id}.example"),
        from_domain: format!("{broker_id}.example"),
        subject: subject.to_string(),
        body: body.to_string(),
        broker_id: broker_id.to_string(),
        broker_name: format!("Broker {broker_id}"),
        received_at: Some(chrono::Utc::now()),
        ..Default::default()
    }
}

// -------------------------------------------------------------------
// Pipeline stages
// -------------------------------------------------------------------

#[test]
fn each_reply_moves_the_broker_to_the_matching_stage() {
    assert_eq!(stage_for(ResponseType::Success), PipelineStatus::Confirmed);
    assert_eq!(
        stage_for(ResponseType::FormRequired),
        PipelineStatus::FormRequired
    );
    assert_eq!(
        stage_for(ResponseType::ConfirmationRequired),
        PipelineStatus::AwaitingConfirmation
    );
    assert_eq!(stage_for(ResponseType::Rejected), PipelineStatus::Rejected);
    assert_eq!(
        stage_for(ResponseType::Pending),
        PipelineStatus::AwaitingResponse
    );
}

/// A bounce means the request never arrived. That is a failure of the send,
/// not a refusal by the broker, and filing it as a rejection would say the
/// company answered when it never saw the request.
#[test]
fn a_bounce_is_a_failure_rather_than_a_rejection() {
    assert_eq!(stage_for(ResponseType::Bounced), PipelineStatus::Failed);
    assert_ne!(stage_for(ResponseType::Bounced), PipelineStatus::Rejected);
}

/// An unreadable reply still means the broker answered, so the broker is
/// waiting on something rather than finished.
#[test]
fn an_unreadable_reply_leaves_the_broker_awaiting_a_response() {
    assert_eq!(
        stage_for(ResponseType::Unknown),
        PipelineStatus::AwaitingResponse
    );
}

// -------------------------------------------------------------------
// Storing a reply
// -------------------------------------------------------------------

#[tokio::test]
async fn a_classified_reply_is_stored_against_its_broker() {
    let store = store_with_request("acme").await;
    let email = reply(
        "acme",
        "Re: Personal Data Removal Request",
        "Please use our opt-out form: https://acme.example/opt-out",
    );
    let result = classifier::classify(&email);

    assert!(
        store_response(&store, DEFAULT_USER_ID, &email, &result)
            .await
            .unwrap()
    );

    let stored = store
        .find_response_by_subject(DEFAULT_USER_ID, "acme", "Re: Personal Data Removal Request")
        .await
        .unwrap()
        .expect("the reply should be stored");

    assert_eq!(stored.response_type, StoredType::FormRequired);
    assert_eq!(stored.form_url, "https://acme.example/opt-out");
    assert_eq!(stored.broker_name, "Broker acme");
    assert!(stored.received_at.is_some());
}

/// The body is kept so a later classifier change can re-read the reply
/// without going back to the mailbox — which by then may no longer have it.
#[tokio::test]
async fn the_reply_body_is_kept_for_reclassification() {
    let store = store_with_request("acme").await;
    let email = reply("acme", "Re: request", "We have removed your data.");
    let result = classifier::classify(&email);

    store_response(&store, DEFAULT_USER_ID, &email, &result)
        .await
        .unwrap();

    let stored = store
        .find_response_by_subject(DEFAULT_USER_ID, "acme", "Re: request")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.email_body, "We have removed your data.");
}

/// A response row keyed to no broker cannot be acted on by anything.
#[tokio::test]
async fn a_reply_matching_no_broker_is_not_stored() {
    let store = Store::open_in_memory().await.unwrap();
    let email = Email {
        from: "newsletter@unrelated.example".into(),
        subject: "Our weekly update".into(),
        body: "Hello!".into(),
        ..Default::default()
    };
    let result = classifier::classify(&email);

    assert!(
        !store_response(&store, DEFAULT_USER_ID, &email, &result)
            .await
            .unwrap()
    );
    assert!(
        store
            .broker_responses(DEFAULT_USER_ID, ResponseFilter::default())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn storing_a_reply_advances_the_brokers_stage() {
    let store = store_with_request("acme").await;
    let email = reply("acme", "Re: request", "We have removed your data.");

    assert!(
        advance_pipeline(&store, DEFAULT_USER_ID, &email, ResponseType::Success)
            .await
            .unwrap()
    );

    let record = store
        .last_request_for_broker(DEFAULT_USER_ID, "acme")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.pipeline_status, PipelineStatus::Confirmed);
    assert_eq!(
        record.status,
        Status::Sent,
        "the send itself still succeeded"
    );
}

/// A broker that replied before any request was recorded — a stale mailbox,
/// a database that was reset — should not fail the scan.
#[tokio::test]
async fn a_reply_from_a_broker_with_no_request_does_not_fail() {
    let store = Store::open_in_memory().await.unwrap();
    let email = reply("acme", "Re: request", "We have removed your data.");

    assert!(
        !advance_pipeline(&store, DEFAULT_USER_ID, &email, ResponseType::Success)
            .await
            .unwrap(),
        "there was nothing to advance"
    );
}

/// Scanning twice must not file the same reply twice.
#[tokio::test]
async fn the_same_reply_scanned_twice_is_stored_once() {
    let store = store_with_request("acme").await;
    let email = reply("acme", "Re: request", "We have removed your data.");
    let result = classifier::classify(&email);

    store_response(&store, DEFAULT_USER_ID, &email, &result)
        .await
        .unwrap();
    store_response(&store, DEFAULT_USER_ID, &email, &result)
        .await
        .unwrap();

    let all = store
        .broker_responses(DEFAULT_USER_ID, ResponseFilter::default())
        .await
        .unwrap();
    assert_eq!(all.len(), 1);
}

// -------------------------------------------------------------------
// Re-reading stored replies
// -------------------------------------------------------------------

#[tokio::test]
async fn reclassifying_leaves_an_unchanged_reply_alone() {
    let store = store_with_request("acme").await;
    let email = reply("acme", "Re: request", "We have removed your data.");
    let result = classifier::classify(&email);
    store_response(&store, DEFAULT_USER_ID, &email, &result)
        .await
        .unwrap();

    assert_eq!(
        reclassify_stored(&store, DEFAULT_USER_ID).await.unwrap(),
        0,
        "nothing changed, so nothing should be rewritten"
    );
}

/// A reply that was filed wrong should be fixable without re-fetching the
/// mailbox, which by then may have been cleared.
#[tokio::test]
async fn reclassifying_corrects_a_stored_reply_from_its_body() {
    let store = store_with_request("acme").await;
    let email = reply(
        "acme",
        "Re: request",
        "Please use our opt-out form: https://acme.example/opt-out",
    );

    // Store it as something it is not, as an older classifier might have.
    store
        .upsert_broker_response(&crate::history::NewBrokerResponse {
            broker_id: "acme".into(),
            broker_name: "Broker acme".into(),
            response_type: StoredType::Unknown,
            email_from: email.from.clone(),
            email_subject: email.subject.clone(),
            email_body: email.body.clone(),
            needs_review: true,
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(reclassify_stored(&store, DEFAULT_USER_ID).await.unwrap(), 1);

    let stored = store
        .find_response_by_subject(DEFAULT_USER_ID, "acme", "Re: request")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.response_type, StoredType::FormRequired);
    assert_eq!(stored.form_url, "https://acme.example/opt-out");
    assert!(!stored.needs_review, "it is no longer ambiguous");
}

/// Rows stored before the body column existed have only their subject.
#[tokio::test]
async fn a_reply_with_no_body_is_reclassified_from_its_subject() {
    let store = store_with_request("acme").await;

    store
        .upsert_broker_response(&crate::history::NewBrokerResponse {
            broker_id: "acme".into(),
            broker_name: "Broker acme".into(),
            response_type: StoredType::Unknown,
            email_subject: "Your Request Has Been Received".into(),
            email_body: String::new(),
            needs_review: true,
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(reclassify_stored(&store, DEFAULT_USER_ID).await.unwrap(), 1);

    let stored = store
        .find_response_by_subject(DEFAULT_USER_ID, "acme", "Your Request Has Been Received")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.response_type, StoredType::Pending);
}

#[tokio::test]
async fn reclassifying_also_moves_the_brokers_stage() {
    let store = store_with_request("acme").await;

    store
        .upsert_broker_response(&crate::history::NewBrokerResponse {
            broker_id: "acme".into(),
            broker_name: "Broker acme".into(),
            response_type: StoredType::Unknown,
            email_subject: "Re: request".into(),
            email_body: "We have removed your data.".into(),
            needs_review: true,
            ..Default::default()
        })
        .await
        .unwrap();

    reclassify_stored(&store, DEFAULT_USER_ID).await.unwrap();

    let record = store
        .last_request_for_broker(DEFAULT_USER_ID, "acme")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.pipeline_status, PipelineStatus::Confirmed);
}

#[tokio::test]
async fn reclassifying_an_empty_store_does_nothing() {
    let store = Store::open_in_memory().await.unwrap();
    assert_eq!(reclassify_stored(&store, DEFAULT_USER_ID).await.unwrap(), 0);
}

// -------------------------------------------------------------------
// Filing what the patterns could not place
// -------------------------------------------------------------------
async fn store_unplaced_reply(store: &Store, broker_id: &str, subject: &str, body: &str) {
    store
        .upsert_broker_response(&crate::history::NewBrokerResponse {
            broker_id: broker_id.into(),
            broker_name: format!("Broker {broker_id}"),
            response_type: StoredType::Unknown,
            email_subject: subject.into(),
            email_body: body.into(),
            needs_review: true,
            ..Default::default()
        })
        .await
        .unwrap();
}

/// A decider that answers from a script.
struct Scripted {
    answer: &'static str,
    confidence: f32,
}

#[async_trait::async_trait]
impl Decider for Scripted {
    async fn choose(
        &self,
        _state: &str,
        _question: &Choice,
    ) -> Result<crate::decision::Decision, crate::decision::Error> {
        Ok(crate::decision::Decision {
            choice: self.answer.to_string(),
            confidence: self.confidence,
            probabilities: Default::default(),
            model: "stub-1".to_string(),
        })
    }

    fn name(&self) -> &'static str {
        "stub"
    }
}

/// A decider that is always down.
struct Unreachable;

#[async_trait::async_trait]
impl Decider for Unreachable {
    async fn choose(
        &self,
        _state: &str,
        _question: &Choice,
    ) -> Result<crate::decision::Decision, crate::decision::Error> {
        Err(crate::decision::Error::Unreachable(
            "connection refused".into(),
        ))
    }

    fn name(&self) -> &'static str {
        "stub"
    }
}

#[tokio::test]
async fn a_reply_the_rules_could_not_place_is_filed_by_the_model() {
    let store = store_with_request("acme").await;
    store_unplaced_reply(
        &store,
        "acme",
        "Re: request",
        "We are looking into your request and will be in touch.",
    )
    .await;

    let decider = Scripted {
        answer: "pending",
        confidence: 0.83,
    };
    let summary = classify_unknown(&store, &decider, 0.6, DEFAULT_USER_ID, 50)
        .await
        .unwrap();

    assert_eq!(summary.considered, 1);
    assert_eq!(summary.filed, 1);
    assert_eq!(summary.unsure, 0);

    let stored = store
        .find_response_by_subject(DEFAULT_USER_ID, "acme", "Re: request")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.response_type, StoredType::Pending);
    assert!(!stored.needs_review, "it has been placed");
    assert!((stored.confidence - 0.83).abs() < 0.001);
}

#[tokio::test]
async fn filing_also_moves_the_brokers_stage() {
    let store = store_with_request("acme").await;
    store_unplaced_reply(&store, "acme", "Re: request", "Your data has been deleted.").await;

    let decider = Scripted {
        answer: "success",
        confidence: 0.9,
    };
    classify_unknown(&store, &decider, 0.6, DEFAULT_USER_ID, 50)
        .await
        .unwrap();

    let record = store
        .last_request_for_broker(DEFAULT_USER_ID, "acme")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.pipeline_status, PipelineStatus::Confirmed);
}

/// A reply a person has already dealt with is not re-opened, and one the
/// rules already placed is not second-guessed.
#[tokio::test]
async fn only_the_unplaced_replies_are_considered() {
    let store = store_with_request("acme").await;
    store_unplaced_reply(&store, "acme", "Re: unplaced", "Hmm.").await;

    store
        .upsert_broker_response(&crate::history::NewBrokerResponse {
            broker_id: "globex".into(),
            broker_name: "Broker globex".into(),
            response_type: StoredType::Success,
            email_subject: "Re: placed".into(),
            email_body: "We have removed your data.".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    let decider = Scripted {
        answer: "pending",
        confidence: 0.9,
    };
    let summary = classify_unknown(&store, &decider, 0.6, DEFAULT_USER_ID, 50)
        .await
        .unwrap();

    assert_eq!(summary.considered, 1, "only the unplaced one");
    assert_eq!(summary.filed, 1);

    let placed = store
        .find_response_by_subject(DEFAULT_USER_ID, "globex", "Re: placed")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(placed.response_type, StoredType::Success);
}

#[tokio::test]
async fn a_hesitant_answer_leaves_the_reply_for_a_person() {
    let store = store_with_request("acme").await;
    store_unplaced_reply(&store, "acme", "Re: request", "Whatever.").await;

    let decider = Scripted {
        answer: "pending",
        confidence: 0.2,
    };
    let summary = classify_unknown(&store, &decider, 0.6, DEFAULT_USER_ID, 50)
        .await
        .unwrap();

    assert_eq!(summary.considered, 1);
    assert_eq!(summary.filed, 0);
    assert_eq!(summary.unsure, 1);

    let stored = store
        .find_response_by_subject(DEFAULT_USER_ID, "acme", "Re: request")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.response_type, StoredType::Unknown);
    assert!(stored.needs_review, "still waiting for a person");
}

#[tokio::test]
async fn an_unreachable_model_leaves_every_reply_where_it_was() {
    let store = store_with_request("acme").await;
    store_unplaced_reply(&store, "acme", "Re: request", "Whatever.").await;

    let summary = classify_unknown(&store, &Unreachable, 0.6, DEFAULT_USER_ID, 50)
        .await
        .unwrap();

    assert_eq!(summary.considered, 1);
    assert_eq!(summary.filed, 0);
    assert_eq!(summary.unsure, 1);
}

/// The verdicts that need a URL to act on are not offered, because the URL
/// comes from the page rather than the email.
#[test]
fn the_question_does_not_offer_verdicts_that_need_a_form() {
    let question = classification_question();
    assert!(question.offers("success"));
    assert!(question.offers("rejected"));
    assert!(question.offers("pending"));
    assert!(!question.offers("form_required"));
    assert!(!question.offers("confirmation_required"));
    assert!(!question.offers("unknown"));
}

#[test]
fn only_the_three_verdicts_parse() {
    assert_eq!(verdict_for("success"), Some(ResponseType::Success));
    assert_eq!(verdict_for("rejected"), Some(ResponseType::Rejected));
    assert_eq!(verdict_for("pending"), Some(ResponseType::Pending));
    assert_eq!(verdict_for("form_required"), None);
    assert_eq!(verdict_for("whatever"), None);
}

// -------------------------------------------------------------------
// Options and naming
// -------------------------------------------------------------------

/// A mailbox holds a great deal that has nothing to do with eruser.
#[test]
fn unmatched_mail_is_left_out_by_default() {
    assert!(!ScanOptions::default().include_unmatched);
    assert_eq!(ScanOptions::default().days, DEFAULT_DAYS);
}

#[test]
fn a_sender_is_named_by_the_best_thing_available() {
    let mut email = Email::default();
    assert_eq!(display_name(&email), "unknown sender");

    email.from = "privacy@acme.example".into();
    assert_eq!(display_name(&email), "privacy@acme.example");

    email.from_name = "Acme Privacy Team".into();
    assert_eq!(display_name(&email), "Acme Privacy Team");

    email.broker_name = "Acme Data".into();
    assert_eq!(display_name(&email), "Acme Data");
}

#[test]
fn an_address_reduces_to_its_domain() {
    assert_eq!(domain_of_address("Privacy@ACME.example"), "acme.example");
    assert_eq!(domain_of_address("not-an-address"), "");
}
