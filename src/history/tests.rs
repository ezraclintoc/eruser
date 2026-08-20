use super::*;

const OTHER_USER: i64 = 2;

async fn store() -> Store {
    let store = Store::open_in_memory().await.expect("in-memory store");
    sqlx::query("INSERT INTO users (id, username) VALUES (?, 'second')")
        .bind(OTHER_USER)
        .execute(store.pool())
        .await
        .expect("seed a second user");
    store
}

fn sent_record(broker_id: &str) -> NewRecord {
    NewRecord::sent(
        broker_id,
        &format!("Broker {broker_id}"),
        &format!("privacy@{broker_id}.example"),
        "generic",
        "<abc@example.com>",
    )
}

fn failed_record(broker_id: &str) -> NewRecord {
    NewRecord::failed(
        broker_id,
        &format!("Broker {broker_id}"),
        &format!("privacy@{broker_id}.example"),
        "generic",
        "SMTP authentication failed",
    )
}

fn response(broker_id: &str, subject: &str) -> NewBrokerResponse {
    NewBrokerResponse {
        broker_id: broker_id.to_string(),
        broker_name: format!("Broker {broker_id}"),
        response_type: ResponseType::FormRequired,
        email_from: format!("privacy@{broker_id}.example"),
        email_subject: subject.to_string(),
        email_body: "Please use our web form.".to_string(),
        form_url: format!("https://{broker_id}.example/optout"),
        confidence: 0.9,
        ..Default::default()
    }
}

fn task(broker_id: &str, task_type: TaskType) -> NewPendingTask {
    NewPendingTask {
        broker_id: broker_id.to_string(),
        broker_name: format!("Broker {broker_id}"),
        task_type,
        form_url: format!("https://{broker_id}.example/optout"),
        ..Default::default()
    }
}

// -------------------------------------------------------------------
// Schema and enums
// -------------------------------------------------------------------

#[tokio::test]
async fn migrations_run_and_seed_the_default_user() {
    let store = Store::open_in_memory().await.unwrap();
    let username: String = sqlx::query_scalar("SELECT username FROM users WHERE id = ?")
        .bind(DEFAULT_USER_ID)
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(username, "default");
}

/// The default user must not be loggable-in before authentication exists.
#[tokio::test]
async fn the_default_user_has_no_password_hash() {
    let store = Store::open_in_memory().await.unwrap();
    let hash: Option<String> = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?")
        .bind(DEFAULT_USER_ID)
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert!(hash.is_none());
}

#[test]
fn enums_round_trip_through_their_database_representation() {
    for status in Status::ALL {
        assert_eq!(Status::from_db(status.as_str()), *status);
    }
    for status in PipelineStatus::ALL {
        assert_eq!(PipelineStatus::from_db(status.as_str()), *status);
    }
    for task_type in TaskType::ALL {
        assert_eq!(TaskType::from_db(task_type.as_str()), *task_type);
    }
    for status in TaskStatus::ALL {
        assert_eq!(TaskStatus::from_db(status.as_str()), *status);
    }
    for response_type in ResponseType::ALL {
        assert_eq!(
            ResponseType::from_db(response_type.as_str()),
            *response_type
        );
    }
}

/// A row written by a newer eruser should not make an older one unable to
/// read its own history.
#[test]
fn unknown_enum_values_fall_back_rather_than_failing() {
    assert_eq!(
        PipelineStatus::from_db("teleported"),
        PipelineStatus::EmailSent
    );
    assert_eq!(Status::from_db("exploded"), Status::Failed);
    assert_eq!(ResponseType::from_db("shrug"), ResponseType::Unknown);
    assert_eq!(TaskStatus::from_db("later"), TaskStatus::Pending);
}

#[test]
fn parsing_an_unknown_enum_value_is_still_an_error() {
    assert!("teleported".parse::<PipelineStatus>().is_err());
}

// -------------------------------------------------------------------
// Removal requests
// -------------------------------------------------------------------

#[tokio::test]
async fn a_recorded_request_reads_back_intact() {
    let store = store().await;
    let id = store.add_record(&sent_record("acme")).await.unwrap();

    let record = store
        .last_request_for_broker(DEFAULT_USER_ID, "acme")
        .await
        .unwrap()
        .expect("the record should be there");

    assert_eq!(record.id, id);
    assert_eq!(record.user_id, DEFAULT_USER_ID);
    assert_eq!(record.broker_name, "Broker acme");
    assert_eq!(record.status, Status::Sent);
    assert_eq!(record.message_id, "<abc@example.com>");
    assert_eq!(record.error, "");
    assert_eq!(record.pipeline_status, PipelineStatus::EmailSent);
    assert!(record.sent_at.is_some());
    assert!(record.created_at.is_some());
}

#[tokio::test]
async fn a_failed_request_records_its_error() {
    let store = store().await;
    store.add_record(&failed_record("acme")).await.unwrap();

    let record = store
        .last_request_for_broker(DEFAULT_USER_ID, "acme")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, Status::Failed);
    assert_eq!(record.error, "SMTP authentication failed");
    assert_eq!(record.message_id, "");
}

#[tokio::test]
async fn last_request_for_an_unknown_broker_is_none() {
    let store = store().await;
    assert!(
        store
            .last_request_for_broker(DEFAULT_USER_ID, "nobody")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn recent_requests_are_newest_first_and_respect_the_limit() {
    let store = store().await;
    for broker in ["one", "two", "three"] {
        store.add_record(&sent_record(broker)).await.unwrap();
    }

    let recent = store.recent_requests(DEFAULT_USER_ID, 2).await.unwrap();
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].broker_id, "three");
    assert_eq!(recent[1].broker_id, "two");
}

#[tokio::test]
async fn stats_count_sent_and_failed() {
    let store = store().await;
    store.add_record(&sent_record("a")).await.unwrap();
    store.add_record(&sent_record("b")).await.unwrap();
    store.add_record(&failed_record("c")).await.unwrap();

    assert_eq!(
        store.stats(DEFAULT_USER_ID).await.unwrap(),
        Stats {
            total: 3,
            sent: 2,
            failed: 1
        }
    );
}

/// SUM over zero rows is NULL in SQLite, not 0.
#[tokio::test]
async fn stats_on_an_empty_database_are_zero() {
    let store = store().await;
    assert_eq!(
        store.stats(DEFAULT_USER_ID).await.unwrap(),
        Stats::default()
    );
    assert_eq!(
        store.monthly_stats(DEFAULT_USER_ID).await.unwrap(),
        Stats::default()
    );
}

#[tokio::test]
async fn monthly_stats_exclude_older_requests() {
    let store = store().await;
    store.add_record(&sent_record("recent")).await.unwrap();

    // Backdate one request well past any month boundary.
    let old = store.add_record(&sent_record("old")).await.unwrap();
    sqlx::query("UPDATE removal_requests SET sent_at = ? WHERE id = ?")
        .bind(Utc::now() - chrono::Duration::days(400))
        .bind(old)
        .execute(store.pool())
        .await
        .unwrap();

    assert_eq!(store.stats(DEFAULT_USER_ID).await.unwrap().sent, 2);
    assert_eq!(store.monthly_stats(DEFAULT_USER_ID).await.unwrap().sent, 1);
}

#[tokio::test]
async fn broker_statuses_summarize_the_latest_attempt_per_broker() {
    let store = store().await;
    store.add_record(&failed_record("acme")).await.unwrap();
    store.add_record(&sent_record("acme")).await.unwrap();
    store.add_record(&sent_record("other")).await.unwrap();

    let statuses = store.all_broker_statuses(DEFAULT_USER_ID).await.unwrap();
    assert_eq!(statuses.len(), 2);

    let acme = &statuses["acme"];
    assert_eq!(acme.total_sent, 2);
    assert_eq!(acme.status, Status::Sent, "the latest attempt succeeded");
    assert!(acme.last_sent.is_some());
}

#[tokio::test]
async fn delete_by_status_removes_only_that_status() {
    let store = store().await;
    store.add_record(&sent_record("a")).await.unwrap();
    store.add_record(&failed_record("b")).await.unwrap();
    store.add_record(&failed_record("c")).await.unwrap();

    assert_eq!(
        store
            .delete_by_status(DEFAULT_USER_ID, Status::Failed)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        store.stats(DEFAULT_USER_ID).await.unwrap(),
        Stats {
            total: 1,
            sent: 1,
            failed: 0
        }
    );
}

// -------------------------------------------------------------------
// Broker responses
// -------------------------------------------------------------------

#[tokio::test]
async fn a_stored_response_reads_back_intact() {
    let store = store().await;
    let id = store
        .upsert_broker_response(&response("acme", "Re: your request"))
        .await
        .unwrap();

    let stored = store
        .find_response_by_subject(DEFAULT_USER_ID, "acme", "Re: your request")
        .await
        .unwrap()
        .expect("the response should be there");

    assert_eq!(stored.id, id);
    assert_eq!(stored.response_type, ResponseType::FormRequired);
    assert_eq!(stored.email_body, "Please use our web form.");
    assert_eq!(stored.form_url, "https://acme.example/optout");
    assert!((stored.confidence - 0.9).abs() < f64::EPSILON);
    assert!(!stored.needs_review);
}

/// Upstream inserted unconditionally after a separate lookup, so two monitor
/// runs could store the same reply twice.
#[tokio::test]
async fn storing_the_same_reply_twice_updates_rather_than_duplicates() {
    let store = store().await;
    let first = store
        .upsert_broker_response(&response("acme", "Re: your request"))
        .await
        .unwrap();

    let mut changed = response("acme", "Re: your request");
    changed.response_type = ResponseType::Success;
    changed.email_body = "Your data has been removed.".into();
    let second = store.upsert_broker_response(&changed).await.unwrap();

    assert_eq!(first, second, "the same row should be reused");
    let all = store
        .broker_responses(DEFAULT_USER_ID, ResponseFilter::default())
        .await
        .unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].response_type, ResponseType::Success);
    assert_eq!(all[0].email_body, "Your data has been removed.");
}

#[tokio::test]
async fn responses_can_be_filtered_by_type_review_flag_and_limit() {
    let store = store().await;
    store
        .upsert_broker_response(&response("a", "one"))
        .await
        .unwrap();

    let mut needs_review = response("b", "two");
    needs_review.needs_review = true;
    needs_review.response_type = ResponseType::Unknown;
    store.upsert_broker_response(&needs_review).await.unwrap();

    let forms = store
        .broker_responses(
            DEFAULT_USER_ID,
            ResponseFilter {
                response_type: Some(ResponseType::FormRequired),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(forms.len(), 1);
    assert_eq!(forms[0].broker_id, "a");

    let flagged = store
        .broker_responses(
            DEFAULT_USER_ID,
            ResponseFilter {
                needs_review: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(flagged.len(), 1);
    assert_eq!(flagged[0].broker_id, "b");

    let limited = store
        .broker_responses(
            DEFAULT_USER_ID,
            ResponseFilter {
                limit: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(limited.len(), 1);
}

#[tokio::test]
async fn a_response_can_be_reclassified() {
    let store = store().await;
    let id = store
        .upsert_broker_response(&response("acme", "subject"))
        .await
        .unwrap();

    store
        .update_response_classification(
            id,
            ResponseType::ConfirmationRequired,
            "",
            "https://acme.example/confirm?token=x",
            0.75,
            true,
        )
        .await
        .unwrap();

    let stored = store
        .find_response_by_subject(DEFAULT_USER_ID, "acme", "subject")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.response_type, ResponseType::ConfirmationRequired);
    assert_eq!(stored.confirm_url, "https://acme.example/confirm?token=x");
    assert_eq!(stored.form_url, "");
    assert!(stored.needs_review);
    // The body survives reclassification so it can be classified again.
    assert_eq!(stored.email_body, "Please use our web form.");
}

#[tokio::test]
async fn a_response_body_can_be_backfilled() {
    let store = store().await;
    let id = store
        .upsert_broker_response(&response("acme", "subject"))
        .await
        .unwrap();
    store.update_response_body(id, "new body").await.unwrap();

    let stored = store
        .find_response_by_subject(DEFAULT_USER_ID, "acme", "subject")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.email_body, "new body");
}

#[tokio::test]
async fn responses_can_be_cleared_for_a_full_rescan() {
    let store = store().await;
    store
        .upsert_broker_response(&response("a", "one"))
        .await
        .unwrap();
    store
        .upsert_broker_response(&response("b", "two"))
        .await
        .unwrap();

    assert_eq!(
        store.clear_broker_responses(DEFAULT_USER_ID).await.unwrap(),
        2
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
async fn response_stats_count_by_type() {
    let store = store().await;
    store
        .upsert_broker_response(&response("a", "one"))
        .await
        .unwrap();
    store
        .upsert_broker_response(&response("b", "two"))
        .await
        .unwrap();

    let mut success = response("c", "three");
    success.response_type = ResponseType::Success;
    store.upsert_broker_response(&success).await.unwrap();

    let stats = store.response_stats(DEFAULT_USER_ID).await.unwrap();
    assert_eq!(stats[&ResponseType::FormRequired], 2);
    assert_eq!(stats[&ResponseType::Success], 1);
}

// -------------------------------------------------------------------
// Pending tasks
// -------------------------------------------------------------------

#[tokio::test]
async fn a_task_reads_back_intact_and_starts_pending() {
    let store = store().await;
    let id = store
        .add_task(&task("acme", TaskType::Captcha))
        .await
        .unwrap();

    let stored = store
        .task_by_id(DEFAULT_USER_ID, id)
        .await
        .unwrap()
        .expect("the task should be there");
    assert_eq!(stored.task_type, TaskType::Captcha);
    assert_eq!(stored.status, TaskStatus::Pending);
    assert!(stored.opened_at.is_none());
    assert!(stored.completed_at.is_none());
}

#[tokio::test]
async fn tasks_can_be_filtered_by_type_and_status() {
    let store = store().await;
    let captcha = store.add_task(&task("a", TaskType::Captcha)).await.unwrap();
    store
        .add_task(&task("b", TaskType::ManualForm))
        .await
        .unwrap();
    store
        .complete_task(DEFAULT_USER_ID, captcha, TaskStatus::Completed)
        .await
        .unwrap();

    let captchas = store
        .tasks(
            DEFAULT_USER_ID,
            TaskFilter {
                task_type: Some(TaskType::Captcha),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(captchas.len(), 1);

    let pending = store
        .tasks(
            DEFAULT_USER_ID,
            TaskFilter {
                status: Some(TaskStatus::Pending),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].broker_id, "b");
}

#[tokio::test]
async fn completing_a_task_stamps_it_and_reports_whether_it_existed() {
    let store = store().await;
    let id = store
        .add_task(&task("acme", TaskType::Captcha))
        .await
        .unwrap();

    assert!(
        store
            .complete_task(DEFAULT_USER_ID, id, TaskStatus::Completed)
            .await
            .unwrap()
    );
    assert!(
        !store
            .complete_task(DEFAULT_USER_ID, 9999, TaskStatus::Completed)
            .await
            .unwrap(),
        "completing a nonexistent task should report false, not succeed silently"
    );

    let stored = store
        .task_by_id(DEFAULT_USER_ID, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, TaskStatus::Completed);
    assert!(stored.completed_at.is_some());
}

#[tokio::test]
async fn opened_at_records_only_the_first_visit() {
    let store = store().await;
    let id = store
        .add_task(&task("acme", TaskType::Captcha))
        .await
        .unwrap();

    store.mark_task_opened(DEFAULT_USER_ID, id).await.unwrap();
    let first = store
        .task_by_id(DEFAULT_USER_ID, id)
        .await
        .unwrap()
        .unwrap()
        .opened_at;
    assert!(first.is_some());

    store.mark_task_opened(DEFAULT_USER_ID, id).await.unwrap();
    let second = store
        .task_by_id(DEFAULT_USER_ID, id)
        .await
        .unwrap()
        .unwrap()
        .opened_at;
    assert_eq!(first, second, "opened_at should not move on later visits");
}

#[tokio::test]
async fn task_stats_count_by_status() {
    let store = store().await;
    let a = store.add_task(&task("a", TaskType::Captcha)).await.unwrap();
    let b = store
        .add_task(&task("b", TaskType::ManualForm))
        .await
        .unwrap();
    store.add_task(&task("c", TaskType::Review)).await.unwrap();

    store
        .complete_task(DEFAULT_USER_ID, a, TaskStatus::Completed)
        .await
        .unwrap();
    store
        .complete_task(DEFAULT_USER_ID, b, TaskStatus::Skipped)
        .await
        .unwrap();

    assert_eq!(
        store.task_stats(DEFAULT_USER_ID).await.unwrap(),
        TaskStats {
            pending: 1,
            completed: 1,
            skipped: 1
        }
    );
}

// -------------------------------------------------------------------
// Pipeline
// -------------------------------------------------------------------

#[tokio::test]
async fn pipeline_status_advances_the_latest_request_for_a_broker() {
    let store = store().await;
    store.add_record(&sent_record("acme")).await.unwrap();

    assert!(
        store
            .update_pipeline_status(DEFAULT_USER_ID, "acme", PipelineStatus::FormRequired)
            .await
            .unwrap()
    );
    let record = store
        .last_request_for_broker(DEFAULT_USER_ID, "acme")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.pipeline_status, PipelineStatus::FormRequired);
}

#[tokio::test]
async fn advancing_an_unknown_broker_reports_false() {
    let store = store().await;
    assert!(
        !store
            .update_pipeline_status(DEFAULT_USER_ID, "nobody", PipelineStatus::Confirmed)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn pipeline_stats_count_each_broker_once() {
    let store = store().await;
    store.add_record(&sent_record("acme")).await.unwrap();
    store.add_record(&sent_record("acme")).await.unwrap();
    store.add_record(&sent_record("other")).await.unwrap();
    store
        .update_pipeline_status(DEFAULT_USER_ID, "acme", PipelineStatus::Confirmed)
        .await
        .unwrap();

    let stats = store.pipeline_stats(DEFAULT_USER_ID).await.unwrap();
    assert_eq!(stats[&PipelineStatus::Confirmed], 1);
    assert_eq!(stats[&PipelineStatus::EmailSent], 1);
    assert_eq!(stats.values().sum::<i64>(), 2, "two brokers, two rows");
}

#[tokio::test]
async fn forms_are_listed_once_per_broker_with_a_derived_status() {
    let store = store().await;
    store.add_record(&sent_record("acme")).await.unwrap();
    store
        .upsert_broker_response(&response("acme", "Re: request"))
        .await
        .unwrap();

    let forms = store.forms_with_status(DEFAULT_USER_ID).await.unwrap();
    assert_eq!(forms.len(), 1);
    assert_eq!(forms[0].broker_id, "acme");
    assert_eq!(forms[0].form_url, "https://acme.example/optout");
    assert_eq!(forms[0].status, FormStatus::Pending);
    assert_eq!(forms[0].task_id, 0);
}

#[tokio::test]
async fn a_response_without_a_form_url_is_not_a_form() {
    let store = store().await;
    let mut no_form = response("acme", "Re: request");
    no_form.form_url = String::new();
    no_form.response_type = ResponseType::Success;
    store.upsert_broker_response(&no_form).await.unwrap();

    assert!(
        store
            .forms_with_status(DEFAULT_USER_ID)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_pending_captcha_task_makes_a_form_show_as_captcha() {
    let store = store().await;
    store
        .upsert_broker_response(&response("acme", "Re: request"))
        .await
        .unwrap();
    let task_id = store
        .add_task(&task("acme", TaskType::Captcha))
        .await
        .unwrap();

    let forms = store.forms_with_status(DEFAULT_USER_ID).await.unwrap();
    assert_eq!(forms[0].status, FormStatus::Captcha);
    assert_eq!(forms[0].task_id, task_id);
}

#[tokio::test]
async fn a_completed_task_wins_over_the_pipeline_stage() {
    let store = store().await;
    store.add_record(&sent_record("acme")).await.unwrap();
    store
        .upsert_broker_response(&response("acme", "Re: request"))
        .await
        .unwrap();
    let task_id = store
        .add_task(&task("acme", TaskType::ManualForm))
        .await
        .unwrap();

    store
        .update_pipeline_status(DEFAULT_USER_ID, "acme", PipelineStatus::Failed)
        .await
        .unwrap();
    store
        .complete_task(DEFAULT_USER_ID, task_id, TaskStatus::Completed)
        .await
        .unwrap();

    let forms = store.forms_with_status(DEFAULT_USER_ID).await.unwrap();
    assert_eq!(forms[0].status, FormStatus::Filled);
}

#[tokio::test]
async fn pipeline_stage_drives_form_status_when_there_is_no_task() {
    let cases = [
        (PipelineStatus::FormFilled, FormStatus::Filled),
        (PipelineStatus::Confirmed, FormStatus::Filled),
        (PipelineStatus::Failed, FormStatus::Failed),
        (PipelineStatus::Rejected, FormStatus::Skipped),
        (PipelineStatus::AwaitingResponse, FormStatus::Pending),
    ];

    for (pipeline_status, expected) in cases {
        let store = store().await;
        store.add_record(&sent_record("acme")).await.unwrap();
        store
            .upsert_broker_response(&response("acme", "Re: request"))
            .await
            .unwrap();
        store
            .update_pipeline_status(DEFAULT_USER_ID, "acme", pipeline_status)
            .await
            .unwrap();

        let forms = store.forms_with_status(DEFAULT_USER_ID).await.unwrap();
        assert_eq!(forms[0].status, expected, "for {pipeline_status}");
    }
}

#[tokio::test]
async fn form_stats_count_by_derived_status() {
    let store = store().await;
    store.add_record(&sent_record("a")).await.unwrap();
    store
        .upsert_broker_response(&response("a", "one"))
        .await
        .unwrap();
    store.add_record(&sent_record("b")).await.unwrap();
    store
        .upsert_broker_response(&response("b", "two"))
        .await
        .unwrap();
    store
        .update_pipeline_status(DEFAULT_USER_ID, "b", PipelineStatus::FormFilled)
        .await
        .unwrap();

    assert_eq!(
        store.form_stats(DEFAULT_USER_ID).await.unwrap(),
        FormStats {
            pending: 1,
            filled: 1,
            ..Default::default()
        }
    );
}

// -------------------------------------------------------------------
// User scoping
//
// Multi-user is not exposed yet, but the queries are scoped now so the
// feature does not arrive on top of a store that leaks between users.
// -------------------------------------------------------------------

#[tokio::test]
async fn requests_do_not_leak_between_users() {
    let store = store().await;
    store.add_record(&sent_record("acme")).await.unwrap();
    store
        .add_record(&sent_record("acme").for_user(OTHER_USER))
        .await
        .unwrap();
    store
        .add_record(&sent_record("theirs").for_user(OTHER_USER))
        .await
        .unwrap();

    assert_eq!(store.stats(DEFAULT_USER_ID).await.unwrap().total, 1);
    assert_eq!(store.stats(OTHER_USER).await.unwrap().total, 2);
    assert_eq!(
        store
            .recent_requests(DEFAULT_USER_ID, 50)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .last_request_for_broker(DEFAULT_USER_ID, "theirs")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .all_broker_statuses(DEFAULT_USER_ID)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn deleting_by_status_does_not_touch_another_user() {
    let store = store().await;
    store.add_record(&failed_record("a")).await.unwrap();
    store
        .add_record(&failed_record("a").for_user(OTHER_USER))
        .await
        .unwrap();

    assert_eq!(
        store
            .delete_by_status(DEFAULT_USER_ID, Status::Failed)
            .await
            .unwrap(),
        1
    );
    assert_eq!(store.stats(OTHER_USER).await.unwrap().failed, 1);
}

#[tokio::test]
async fn the_same_subject_can_exist_for_two_users() {
    let store = store().await;
    store
        .upsert_broker_response(&response("acme", "Re: request"))
        .await
        .unwrap();

    let mut theirs = response("acme", "Re: request");
    theirs.user_id = OTHER_USER;
    store.upsert_broker_response(&theirs).await.unwrap();

    assert_eq!(
        store
            .broker_responses(DEFAULT_USER_ID, ResponseFilter::default())
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .broker_responses(OTHER_USER, ResponseFilter::default())
            .await
            .unwrap()
            .len(),
        1
    );
}

/// Fetching a task by a guessed id must not reach another user's row.
#[tokio::test]
async fn tasks_are_not_readable_across_users() {
    let store = store().await;
    let mut theirs = task("acme", TaskType::Captcha);
    theirs.user_id = OTHER_USER;
    let id = store.add_task(&theirs).await.unwrap();

    assert!(
        store
            .task_by_id(DEFAULT_USER_ID, id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(store.task_by_id(OTHER_USER, id).await.unwrap().is_some());
    assert!(
        !store
            .complete_task(DEFAULT_USER_ID, id, TaskStatus::Completed)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn forms_and_pipeline_stats_are_scoped_per_user() {
    let store = store().await;
    let mut theirs = response("acme", "Re: request");
    theirs.user_id = OTHER_USER;
    store.upsert_broker_response(&theirs).await.unwrap();
    store
        .add_record(&sent_record("acme").for_user(OTHER_USER))
        .await
        .unwrap();

    assert!(
        store
            .forms_with_status(DEFAULT_USER_ID)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.forms_with_status(OTHER_USER).await.unwrap().len(), 1);
    assert!(
        store
            .pipeline_stats(DEFAULT_USER_ID)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.pipeline_stats(OTHER_USER).await.unwrap().len(), 1);
}

// -------------------------------------------------------------------
// On-disk behaviour
// -------------------------------------------------------------------

#[tokio::test]
async fn an_on_disk_database_persists_across_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("history.db");

    let store = Store::open(&path).await.unwrap();
    store.add_record(&sent_record("acme")).await.unwrap();
    store.close().await;

    let reopened = Store::open(&path).await.unwrap();
    assert_eq!(reopened.stats(DEFAULT_USER_ID).await.unwrap().sent, 1);
}

#[tokio::test]
async fn opening_an_existing_database_twice_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");

    Store::open(&path).await.unwrap().close().await;
    // Migrations must not re-run and fail on the second open.
    Store::open(&path).await.unwrap().close().await;
}

#[test]
fn default_path_sits_beside_the_config() {
    assert!(Store::default_path().ends_with("history.db"));
}
