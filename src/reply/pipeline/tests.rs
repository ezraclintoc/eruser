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
        &Drafting::generated(&Scripted {
            answer: GOOD_REPLY.into(),
            name: "scripted",
        }),
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

    draft_replies(
        &store,
        &Drafting::generated(&drafter),
        &profile(),
        DEFAULT_USER_ID,
        50,
    )
    .await
    .unwrap();
    let summary = draft_replies(
        &store,
        &Drafting::generated(&drafter),
        &profile(),
        DEFAULT_USER_ID,
        50,
    )
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
        &Drafting::generated(&Scripted {
            answer: GOOD_REPLY.into(),
            name: "scripted",
        }),
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
        &Drafting::generated(&Scripted {
            answer: "I will send my passport immediately.".into(),
            name: "scripted",
        }),
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

    let summary = draft_replies(
        &store,
        &Drafting::generated(&Unreachable),
        &profile(),
        DEFAULT_USER_ID,
        50,
    )
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
        &Drafting::generated(&Scripted {
            answer: GOOD_REPLY.into(),
            name: "scripted",
        }),
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

// -------------------------------------------------------------------
// The pre-written replies
// -------------------------------------------------------------------

/// A decider that answers from a script: the label it wants when that label
/// was offered, and otherwise the first option. That is the shape of a real
/// answer — something the caller offered, or something unusable.
struct Fixed {
    answer: &'static str,
    confidence: f32,
}

#[async_trait::async_trait]
impl Decider for Fixed {
    async fn choose(
        &self,
        _state: &str,
        question: &crate::decision::Choice,
    ) -> Result<crate::decision::Decision, crate::decision::Error> {
        let choice = if question.offers(self.answer) {
            self.answer
        } else {
            question.options[0].label.as_str()
        };

        Ok(crate::decision::Decision {
            choice: choice.to_string(),
            confidence: self.confidence,
            probabilities: Default::default(),
            model: "stub-1".to_string(),
        })
    }

    fn name(&self) -> &'static str {
        "stub"
    }
}

/// The settings for a run that uses the shipped wording.
fn canned<'a>(decider: Option<&'a dyn Decider>, route: bool) -> Drafting<'a> {
    Drafting {
        drafter: None,
        decider,
        wording: Wording::Canned,
        min_confidence: 0.6,
        route,
    }
}

/// The shipped wording is a complete answer on its own: no model anywhere,
/// and a reply still lands on the task list.
#[tokio::test]
async fn the_shipped_wording_needs_no_model_at_all() {
    let store = store().await;
    store_pending_reply(&store, "acme", "Your removal request").await;

    let summary = draft_replies(
        &store,
        &canned(None, false),
        &profile(),
        DEFAULT_USER_ID,
        50,
    )
    .await
    .unwrap();

    assert_eq!(summary.drafted, 1);

    let drafts = store.pending_drafts(DEFAULT_USER_ID).await.unwrap();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].model, "canned:provide_details");
    // Rendered against the person's own details, not left as a template.
    assert!(drafts[0].body.contains("Jane Doe"), "{}", drafts[0].body);
    assert!(!drafts[0].body.contains("{{"), "{}", drafts[0].body);
}

/// The model picks which pre-written reply to use — and nothing it says can
/// reach the email, because the words are eruser's own either way.
#[tokio::test]
async fn the_model_only_names_the_reply_it_wants() {
    let store = store().await;
    store_pending_reply(&store, "acme", "Your removal request").await;

    let decider = Fixed {
        answer: "ask_what_is_missing",
        confidence: 0.9,
    };

    let summary = draft_replies(
        &store,
        &canned(Some(&decider), false),
        &profile(),
        DEFAULT_USER_ID,
        50,
    )
    .await
    .unwrap();

    assert_eq!(summary.drafted, 1);
    let drafts = store.pending_drafts(DEFAULT_USER_ID).await.unwrap();
    assert_eq!(drafts[0].model, "canned:ask_what_is_missing");
}

/// A route the pattern table cannot produce: a broker asking for a reply by
/// email, as opposed to a link to click.
#[tokio::test]
async fn a_decider_can_route_to_a_reply_the_rules_would_not_draft() {
    let store = store().await;
    store_pending_reply(&store, "acme", "Your removal request").await;

    let decider = Fixed {
        answer: "confirm",
        confidence: 0.9,
    };

    let summary = draft_replies(
        &store,
        &canned(Some(&decider), true),
        &profile(),
        DEFAULT_USER_ID,
        50,
    )
    .await
    .unwrap();

    assert_eq!(summary.drafted, 1);
    let drafts = store.pending_drafts(DEFAULT_USER_ID).await.unwrap();
    assert_eq!(drafts[0].reply_type, ReplyType::Confirm);
    assert_eq!(drafts[0].model, "canned:confirm_stands");
}

/// A model that says the email needs no answer leaves it alone, even where
/// the rule table would have drafted one. Nothing is lost: the reply is
/// still in history and still on the task list for a person.
#[tokio::test]
async fn a_decider_may_decline_to_answer_at_all() {
    let store = store().await;
    store_pending_reply(&store, "acme", "Your removal request").await;

    let decider = Fixed {
        answer: "none",
        confidence: 0.9,
    };

    let summary = draft_replies(
        &store,
        &canned(Some(&decider), true),
        &profile(),
        DEFAULT_USER_ID,
        50,
    )
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
