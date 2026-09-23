use super::*;
use crate::history::{DEFAULT_USER_ID, Store};

async fn store() -> Store {
    Store::open_in_memory().await.expect("in-memory store")
}

fn draft(broker_id: &str, subject: &str, reply_type: ReplyType) -> NewReplyDraft {
    NewReplyDraft {
        user_id: DEFAULT_USER_ID,
        broker_id: broker_id.into(),
        broker_name: format!("Broker {broker_id}"),
        reply_type,
        in_reply_to: subject.into(),
        subject: format!("Re: {subject}"),
        body: "A polite, rule-abiding reply.".into(),
        model: "test-model".into(),
        validation: "{\"length\": 30}".into(),
    }
}

#[tokio::test]
async fn a_draft_round_trips_through_the_store() {
    let store = store().await;
    let saved = store
        .upsert_draft(&draft("acme", "Your request", ReplyType::Confirm))
        .await
        .unwrap();

    let read = store.draft(DEFAULT_USER_ID, saved).await.unwrap();
    assert_eq!(read.broker_id, "acme");
    assert_eq!(read.reply_type, ReplyType::Confirm);
    assert_eq!(read.status, DraftStatus::Draft);
    assert_eq!(read.model, "test-model");
}

/// A re-run of the monitor re-reads the same mailbox; the upsert is what
/// keeps it from leaving a second copy of the same draft every run.
#[tokio::test]
async fn re_drafting_the_same_reply_replaces_rather_than_duplicates() {
    let store = store().await;

    store
        .upsert_draft(&draft("acme", "Your request", ReplyType::Confirm))
        .await
        .unwrap();
    store
        .upsert_draft(&draft("acme", "Your request", ReplyType::Confirm))
        .await
        .unwrap();

    assert_eq!(
        store.pending_drafts(DEFAULT_USER_ID).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn different_reply_types_are_separate_drafts() {
    let store = store().await;

    store
        .upsert_draft(&draft("acme", "Your request", ReplyType::Confirm))
        .await
        .unwrap();
    store
        .upsert_draft(&draft("acme", "Your request", ReplyType::IdVerification))
        .await
        .unwrap();

    assert_eq!(
        store.pending_drafts(DEFAULT_USER_ID).await.unwrap().len(),
        2
    );
}

/// The task list is the only way a draft reaches a person, but noticing a
/// draft twice — two runs, a re-scan — must not queue it twice.
#[tokio::test]
async fn a_draft_is_only_queued_once() {
    let store = store().await;
    let saved = store
        .upsert_draft(&draft("acme", "Your request", ReplyType::Confirm))
        .await
        .unwrap();
    let stored = store.draft(DEFAULT_USER_ID, saved).await.unwrap();

    store.queue_draft_task(&stored).await.unwrap();
    store.queue_draft_task(&stored).await.unwrap();

    let tasks = store
        .tasks(DEFAULT_USER_ID, Default::default())
        .await
        .unwrap();
    let draft_tasks = tasks
        .iter()
        .filter(|task| task.task_type == TaskType::DraftReply)
        .count();
    assert_eq!(draft_tasks, 1);
}

#[tokio::test]
async fn sending_removes_the_draft_from_the_pending_list() {
    let store = store().await;
    let saved = store
        .upsert_draft(&draft("acme", "Your request", ReplyType::Confirm))
        .await
        .unwrap();

    store.mark_draft_sent(DEFAULT_USER_ID, saved).await.unwrap();

    assert!(
        store
            .pending_drafts(DEFAULT_USER_ID)
            .await
            .unwrap()
            .is_empty()
    );
    let read = store.draft(DEFAULT_USER_ID, saved).await.unwrap();
    assert_eq!(read.status, DraftStatus::Sent);
    assert!(read.sent_at.is_some());
}

#[tokio::test]
async fn discarding_does_the_same_without_a_send_time() {
    let store = store().await;
    let saved = store
        .upsert_draft(&draft("acme", "Your request", ReplyType::Confirm))
        .await
        .unwrap();

    store.discard_draft(DEFAULT_USER_ID, saved).await.unwrap();

    let read = store.draft(DEFAULT_USER_ID, saved).await.unwrap();
    assert_eq!(read.status, DraftStatus::Discarded);
    assert!(read.sent_at.is_none());
}

/// The same isolation every other per-person record here has.
#[tokio::test]
async fn drafts_are_not_readable_across_users() {
    let store = store().await;
    let saved = store
        .upsert_draft(&draft("acme", "Your request", ReplyType::Confirm))
        .await
        .unwrap();

    assert!(store.draft(2, saved).await.is_err());
}

#[tokio::test]
async fn has_draft_answers_the_question_it_is_asked() {
    let store = store().await;

    assert!(
        !store
            .has_draft(DEFAULT_USER_ID, "acme", "Your request", ReplyType::Confirm)
            .await
            .unwrap()
    );

    store
        .upsert_draft(&draft("acme", "Your request", ReplyType::Confirm))
        .await
        .unwrap();

    assert!(
        store
            .has_draft(DEFAULT_USER_ID, "acme", "Your request", ReplyType::Confirm)
            .await
            .unwrap()
    );
    assert!(
        !store
            .has_draft(
                DEFAULT_USER_ID,
                "acme",
                "Your request",
                ReplyType::MissingInfo
            )
            .await
            .unwrap()
    );
    assert!(
        !store
            .has_draft(
                DEFAULT_USER_ID,
                "globex",
                "Your request",
                ReplyType::Confirm
            )
            .await
            .unwrap()
    );
}
