use super::*;
use crate::history::{DEFAULT_USER_ID, NewBrokerResponse, Store, TaskType};
use crate::reply::{RawDraft, ReplyContext};

/// A drafter that answers from a script, so the pipeline can be tested
/// without a model.
struct Scripted {
    answer: String,
    name: &'static str,
}

#[async_trait::async_trait]
impl Drafter for Scripted {
    async fn draft(&self, _context: &ReplyContext) -> Result<RawDraft, crate::reply::Error> {
        Ok(RawDraft {
            body: self.answer.clone(),
        })
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

/// A drafter that is always down.
struct Unreachable;

#[async_trait::async_trait]
impl Drafter for Unreachable {
    async fn draft(&self, _context: &ReplyContext) -> Result<RawDraft, crate::reply::Error> {
        Err(crate::reply::Error::DrafterUnavailable(
            "connection refused".into(),
        ))
    }

    fn name(&self) -> &'static str {
        "unreachable"
    }
}

const GOOD_REPLY: &str = "Hello, this confirms my opt-out request stands. \
Please complete the removal of my details from your records.";

fn profile() -> Profile {
    Profile {
        first_name: "Jane".into(),
        last_name: "Doe".into(),
        email: "jane@example.com".into(),
        ..Default::default()
    }
}

async fn store() -> Store {
    Store::open_in_memory().await.expect("in-memory store")
}

async fn store_pending_reply(store: &Store, broker_id: &str, subject: &str) {
    store
        .upsert_broker_response(&NewBrokerResponse {
            user_id: DEFAULT_USER_ID,
            broker_id: broker_id.into(),
            broker_name: format!("Broker {broker_id}"),
            response_type: crate::history::ResponseType::Pending,
            email_from: format!("privacy@{broker_id}.example"),
            email_subject: subject.into(),
            email_body: "We are reviewing your request. Please reply with any missing details."
                .into(),
            form_url: String::new(),
            confirm_url: String::new(),
            confidence: 0.9,
            needs_review: false,
            received_at: None,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn a_pending_reply_gets_a_draft_and_a_task() {
    let store = store().await;
    store_pending_reply(&store, "acme", "Your removal request").await;

    let summary = draft_replies(
        &store,
        &Scripted {
            answer: GOOD_REPLY.into(),
            name: "scripted",
        },
        &profile(),
        DEFAULT_USER_ID,
        50,
    )
    .await
    .unwrap();

    assert_eq!(summary.drafted, 1);
    assert_eq!(summary.refused, 0);

    let drafts = store.pending_drafts(DEFAULT_USER_ID).await.unwrap();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].subject, "Re: Your removal request");

    let tasks = store
        .tasks(DEFAULT_USER_ID, Default::default())
        .await
        .unwrap();
    assert!(
        tasks
            .iter()
            .any(|task| task.task_type == TaskType::DraftReply)
    );
}

/// A second run adds nothing: the draft exists and the task is queued once.
#[tokio::test]
async fn a_second_run_does_not_duplicate_work() {
    let store = store().await;
    store_pending_reply(&store, "acme", "Your removal request").await;

    let drafter = Scripted {
        answer: GOOD_REPLY.into(),
        name: "scripted",
    };

    draft_replies(&store, &drafter, &profile(), DEFAULT_USER_ID, 50)
        .await
        .unwrap();
    let summary = draft_replies(&store, &drafter, &profile(), DEFAULT_USER_ID, 50)
        .await
        .unwrap();

    assert_eq!(summary.drafted, 0);
    assert_eq!(summary.already_answered, 1);
    assert_eq!(
        store.pending_drafts(DEFAULT_USER_ID).await.unwrap().len(),
        1
    );
}

/// Only the draftable verdicts are drafted: a confirmed removal gets no
/// reply, and a bounce is nobody to write to.
#[tokio::test]
async fn replies_that_need_no_answer_are_not_drafted() {
    let store = store().await;
    for verdict in [
        crate::history::ResponseType::Success,
        crate::history::ResponseType::Rejected,
        crate::history::ResponseType::Bounced,
    ] {
        store
            .upsert_broker_response(&NewBrokerResponse {
                user_id: DEFAULT_USER_ID,
                broker_id: format!("broker-{verdict:?}"),
                broker_name: "Broker".into(),
                response_type: verdict,
                email_from: "privacy@broker.example".into(),
                email_subject: format!("reply {verdict:?}"),
                email_body: "Whatever it says, it does not need an answer.".into(),
                form_url: String::new(),
                confirm_url: String::new(),
                confidence: 0.9,
                needs_review: false,
                received_at: None,
            })
            .await
            .unwrap();
    }

    let summary = draft_replies(
        &store,
        &Scripted {
            answer: GOOD_REPLY.into(),
            name: "scripted",
        },
        &profile(),
        DEFAULT_USER_ID,
        50,
    )
    .await
    .unwrap();

    assert_eq!(summary.drafted, 0);
    assert_eq!(summary.considered, 3);
}

/// A sidecar that promises documents produces nothing, and the run keeps
/// going: one bad draft does not stop the others.
#[tokio::test]
async fn a_refused_draft_is_counted_and_skipped() {
    let store = store().await;
    store_pending_reply(&store, "acme", "Your removal request").await;
    store_pending_reply(&store, "globex", "One more thing").await;

    let summary = draft_replies(
        &store,
        &Scripted {
            answer: "I will send my passport immediately.".into(),
            name: "scripted",
        },
        &profile(),
        DEFAULT_USER_ID,
        50,
    )
    .await
    .unwrap();

    assert_eq!(summary.drafted, 0);
    assert_eq!(summary.refused, 2);
    assert!(
        store
            .pending_drafts(DEFAULT_USER_ID)
            .await
            .unwrap()
            .is_empty()
    );
}

/// A sidecar that is down leaves everything as it was: no error, no drafts,
/// and the replies stay in history for the next run.
#[tokio::test]
async fn an_unreachable_drafter_is_a_log_line_not_an_error() {
    let store = store().await;
    store_pending_reply(&store, "acme", "Your removal request").await;

    let summary = draft_replies(&store, &Unreachable, &profile(), DEFAULT_USER_ID, 50)
        .await
        .unwrap();

    assert_eq!(summary.drafted, 0);
    assert!(
        store
            .pending_drafts(DEFAULT_USER_ID)
            .await
            .unwrap()
            .is_empty()
    );
}

/// The cap exists so the first scan of a busy mailbox does not turn into a
/// hundred prompts against someone's laptop.
#[tokio::test]
async fn the_cap_limits_how_much_one_run_asks_for() {
    let store = store().await;
    for index in 0..5 {
        store_pending_reply(&store, "acme", &format!("Reply {index}")).await;
    }

    let summary = draft_replies(
        &store,
        &Scripted {
            answer: GOOD_REPLY.into(),
            name: "scripted",
        },
        &profile(),
        DEFAULT_USER_ID,
        2,
    )
    .await
    .unwrap();

    assert_eq!(summary.drafted, 2);
    assert_eq!(summary.considered, 5);
}

/// Identity requests come through the same pipeline but can never ride the
/// auto-send whitelist, whatever it says.
#[tokio::test]
async fn identity_verification_cannot_auto_send_even_when_whitelisted() {
    let whitelist = vec![
        "confirm".to_string(),
        "id_verification".to_string(),
        "missing_info".to_string(),
    ];

    assert!(!may_auto_send(ReplyType::IdVerification, &whitelist));
    assert!(may_auto_send(ReplyType::Confirm, &whitelist));
    assert!(may_auto_send(ReplyType::MissingInfo, &whitelist));
    assert!(!may_auto_send(ReplyType::Confirm, &[]));
}
