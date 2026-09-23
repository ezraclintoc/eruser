use super::*;
use crate::email::Sent;
use crate::history::{
    DEFAULT_USER_ID, DraftStatus, NewBrokerResponse, NewRecord, NewReplyDraft, ReplyType, Store,
};
use std::sync::Mutex;

/// A sender that records what it was handed, so the tests can assert on the
/// mail that went out.
struct Recording {
    messages: Mutex<Vec<Message>>,
    refuse: bool,
}

impl Recording {
    fn new() -> Self {
        Self {
            messages: Mutex::new(Vec::new()),
            refuse: false,
        }
    }
}

#[async_trait::async_trait]
impl Sender for Recording {
    async fn send(&self, message: &Message) -> Result<Sent, crate::email::Error> {
        if self.refuse {
            return Err(crate::email::Error::Authentication);
        }
        self.messages.lock().unwrap().push(message.clone());
        Ok(Sent {
            message_id: "<sent@example>".into(),
            response: String::new(),
        })
    }

    fn name(&self) -> &'static str {
        "recording"
    }
}

async fn store() -> Store {
    Store::open_in_memory().await.expect("in-memory store")
}

async fn store_draft(store: &Store, reply_type: ReplyType) -> crate::history::ReplyDraft {
    // The reply the draft answers, so the sender has an address to use.
    store
        .add_record(&NewRecord::sent(
            "acme",
            "Broker acme",
            "jane@example.com",
            "generic",
            "<req@example>",
        ))
        .await
        .unwrap();
    store
        .upsert_broker_response(&NewBrokerResponse {
            user_id: DEFAULT_USER_ID,
            broker_id: "acme".into(),
            broker_name: "Broker acme".into(),
            response_type: crate::history::ResponseType::Pending,
            email_from: "privacy@acme.example".into(),
            email_subject: "Your removal request".into(),
            email_body: "Please confirm.".into(),
            form_url: String::new(),
            confirm_url: String::new(),
            confidence: 0.9,
            needs_review: false,
            received_at: None,
        })
        .await
        .unwrap();

    let id = store
        .upsert_draft(&NewReplyDraft {
            user_id: DEFAULT_USER_ID,
            broker_id: "acme".into(),
            broker_name: "Broker acme".into(),
            reply_type,
            in_reply_to: "Your removal request".into(),
            subject: "Re: Your removal request".into(),
            body: "Hello, this confirms my opt-out request stands. Please \
                   complete the removal of my details."
                .into(),
            model: "test".into(),
            validation: "{}".into(),
        })
        .await
        .unwrap();

    store.draft(DEFAULT_USER_ID, id).await.unwrap()
}

fn profile() -> crate::config::Profile {
    crate::config::Profile {
        first_name: "Jane".into(),
        email: "jane@example.com".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_whitelisted_confirm_draft_goes_out() {
    let store = store().await;
    let draft = store_draft(&store, ReplyType::Confirm).await;
    let sender = Recording::new();

    let outcome = send_draft(
        &store,
        &sender,
        "jane@example.com",
        draft.clone(),
        &["confirm".to_string()],
        &profile(),
    )
    .await
    .unwrap();

    match outcome {
        SendResult::Sent { to, subject } => {
            assert_eq!(to, "privacy@acme.example");
            assert_eq!(subject, "Re: Your removal request");
        }
        other => panic!("expected sent, got {other:?}"),
    }

    // Recorded as sent, so it cannot go out twice.
    let after = store.draft(DEFAULT_USER_ID, draft.id).await.unwrap();
    assert_eq!(after.status, DraftStatus::Sent);
}

/// Identity verification is refused here even with the widest whitelist
/// imaginable. This is the test that fails if anyone edits
/// `may_auto_send` to trust the caller's list.
#[tokio::test]
async fn identity_verification_is_refused_on_an_empty_whitelist_of_everything() {
    let store = store().await;
    let draft = store_draft(&store, ReplyType::IdVerification).await;
    let sender = Recording::new();

    let whitelist = vec![
        "confirm".to_string(),
        "missing_info".to_string(),
        "id_verification".to_string(),
    ];

    let outcome = send_draft(
        &store,
        &sender,
        "jane@example.com",
        draft,
        &whitelist,
        &profile(),
    )
    .await
    .unwrap();

    assert!(matches!(outcome, SendResult::NotEligible(_)));
    assert!(sender.messages.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_empty_whitelist_sends_nothing() {
    let store = store().await;
    let draft = store_draft(&store, ReplyType::Confirm).await;
    let sender = Recording::new();

    let outcome = send_draft(&store, &sender, "jane@example.com", draft, &[], &profile())
        .await
        .unwrap();

    assert!(matches!(outcome, SendResult::NotEligible(_)));
    assert!(sender.messages.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_draft_that_was_already_sent_is_not_sent_again() {
    let store = store().await;
    let mut draft = store_draft(&store, ReplyType::Confirm).await;
    store
        .mark_draft_sent(DEFAULT_USER_ID, draft.id)
        .await
        .unwrap();
    draft.status = DraftStatus::Sent;
    let sender = Recording::new();

    let outcome = send_draft(
        &store,
        &sender,
        "jane@example.com",
        draft,
        &["confirm".to_string()],
        &profile(),
    )
    .await
    .unwrap();

    assert!(matches!(outcome, SendResult::NotEligible(_)));
    assert!(sender.messages.lock().unwrap().is_empty());
}

/// The send-time re-validation: a draft that would promise documents does
/// not go out even when the whitelist allows its type, because the rules in
/// force now are the ones that count.
#[tokio::test]
async fn a_draft_that_breaks_the_current_rules_is_refused_at_send_time() {
    let store = store().await;
    let mut draft = store_draft(&store, ReplyType::Confirm).await;
    draft.body = "I will send my passport in the morning post.".into();
    let sender = Recording::new();

    let outcome = send_draft(
        &store,
        &sender,
        "jane@example.com",
        draft,
        &["confirm".to_string()],
        &profile(),
    )
    .await;

    let Err(problem) = outcome else {
        panic!("a rule-breaking draft must not be sent");
    };
    assert!(problem.to_string().contains("refused"), "{problem}");
    assert!(sender.messages.lock().unwrap().is_empty());
}

/// A mail server refusing is an error the caller reports, not a silent skip.
#[tokio::test]
async fn a_server_refusal_is_an_error() {
    let store = store().await;
    let draft = store_draft(&store, ReplyType::Confirm).await;
    let sender = Recording {
        messages: Mutex::new(Vec::new()),
        refuse: true,
    };

    let outcome = send_draft(
        &store,
        &sender,
        "jane@example.com",
        draft,
        &["confirm".to_string()],
        &profile(),
    )
    .await;

    assert!(outcome.is_err());
}
