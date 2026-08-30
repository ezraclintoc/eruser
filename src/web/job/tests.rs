use super::*;

fn manager() -> JobManager {
    JobManager::new()
}

#[test]
fn a_new_job_starts_running_at_zero() {
    let job = manager().create(750, None);
    let snapshot = job.snapshot();

    assert_eq!(snapshot.status, JobStatus::Running);
    assert_eq!(snapshot.progress, 0);
    assert_eq!(snapshot.total, 750);
    assert!(snapshot.completed_at.is_none());
    assert!(snapshot.error.is_none());
    assert!(!job.is_cancelled());
}

#[test]
fn job_ids_are_unique() {
    let manager = manager();
    assert_ne!(manager.create(1, None).id(), manager.create(1, None).id());
}

#[test]
fn progress_counts_every_handled_broker() {
    let job = manager().create(10, None);
    job.update(3, 1, 1, "Acme Data");

    let snapshot = job.snapshot();
    assert_eq!(snapshot.progress, 50, "3 sent + 1 failed + 1 skipped of 10");
    assert_eq!(snapshot.current_broker, "Acme Data");
}

/// An empty run is finished the moment it starts; reporting 0% would leave
/// the progress bar stuck.
#[test]
fn a_job_with_no_brokers_is_already_complete() {
    assert_eq!(manager().create(0, None).snapshot().progress, 100);
}

#[test]
fn progress_never_exceeds_one_hundred() {
    let job = manager().create(2, None);
    job.update(5, 5, 0, "");
    assert_eq!(job.snapshot().progress, 100);
}

#[test]
fn completing_a_job_stamps_it_and_clears_the_current_broker() {
    let job = manager().create(2, None);
    job.update(2, 0, 0, "Acme Data");
    job.complete();

    let snapshot = job.snapshot();
    assert_eq!(snapshot.status, JobStatus::Completed);
    assert!(snapshot.completed_at.is_some());
    assert_eq!(snapshot.current_broker, "");
}

#[test]
fn cancelling_stops_the_pipeline_and_records_the_status() {
    let job = manager().create(750, None);
    let token = job.cancellation_token();

    job.cancel();

    assert!(
        token.is_cancelled(),
        "the send pipeline must see the cancel"
    );
    assert_eq!(job.snapshot().status, JobStatus::Cancelled);
}

/// Go reported an authentication failure as `completed` with an error
/// attached, so a failed run looked successful in the UI.
#[test]
fn a_failed_job_is_reported_as_failed_not_completed() {
    let job = manager().create(750, None);
    job.fail(
        FailureKind::Authentication,
        "the mail server rejected the password",
    );

    let snapshot = job.snapshot();
    assert_eq!(snapshot.status, JobStatus::Failed);
    assert_eq!(snapshot.failure_kind, Some(FailureKind::Authentication));
    assert!(snapshot.error.unwrap().contains("password"));
    assert!(job.is_cancelled(), "failing must stop the pipeline too");
}

#[test]
fn pausing_at_the_daily_limit_leaves_the_job_resumable() {
    let job = manager().create(750, Some(250));
    job.update(250, 0, 0, "");
    job.pause_at_limit("daily limit reached");

    let snapshot = job.snapshot();
    assert_eq!(snapshot.status, JobStatus::Paused);
    assert_eq!(snapshot.daily_limit, Some(250));
    assert!(!job.is_cancelled(), "a paused job is not a cancelled one");
}

/// Whichever terminal transition happens first wins, so a cancel racing a
/// completion cannot end up reported as a clean finish.
#[test]
fn a_terminal_job_does_not_change_status_again() {
    let job = manager().create(2, None);
    job.cancel();
    job.complete();
    job.fail(FailureKind::Other, "too late");

    assert_eq!(job.snapshot().status, JobStatus::Cancelled);
}

#[test]
fn a_run_of_auth_failures_stops_the_job_but_one_does_not() {
    let job = manager().create(750, None);

    assert!(!job.record_auth_failure());
    assert!(!job.record_auth_failure());
    assert!(
        job.record_auth_failure(),
        "three in a row means the password is wrong, not that one broker is odd"
    );
}

#[test]
fn a_success_clears_the_auth_failure_streak() {
    let job = manager().create(750, None);
    job.record_auth_failure();
    job.record_auth_failure();
    job.reset_auth_failures();

    assert!(
        !job.record_auth_failure(),
        "the streak should have restarted"
    );
}

#[test]
fn a_job_can_be_looked_up_by_id() {
    let manager = manager();
    let job = manager.create(5, None);

    assert_eq!(manager.get(job.id()).unwrap().id(), job.id());
    assert!(manager.get("nonexistent").is_none());
}

#[test]
fn the_active_job_is_the_running_one() {
    let manager = manager();
    assert!(manager.active().is_none());

    let job = manager.create(5, None);
    assert_eq!(manager.active().unwrap().id(), job.id());

    job.complete();
    assert!(manager.active().is_none());
}

#[test]
fn cleanup_removes_old_finished_jobs_and_keeps_running_ones() {
    let manager = manager();
    let running = manager.create(5, None);
    let finished = manager.create(5, None);
    finished.complete();

    // Nothing is old enough yet.
    assert_eq!(manager.cleanup(chrono::Duration::hours(1)), 0);

    // Backdate the finished job past the cutoff.
    finished.lock().completed_at = Some(Utc::now() - chrono::Duration::hours(2));

    assert_eq!(manager.cleanup(chrono::Duration::hours(1)), 1);
    assert!(manager.get(finished.id()).is_none());
    assert!(manager.get(running.id()).is_some());
}

#[test]
fn a_snapshot_serializes_without_empty_optional_fields() {
    let job = manager().create(5, None);
    let json = serde_json::to_string(&job.snapshot()).unwrap();

    assert!(json.contains("\"status\":\"running\""));
    assert!(!json.contains("error"));
    assert!(!json.contains("completed_at"));
    assert!(!json.contains("daily_limit"));
}

// -------------------------------------------------------------------
// Persistence
// -------------------------------------------------------------------

fn pending() -> PendingJob {
    PendingJob {
        id: "job-1".into(),
        status: JobStatus::Paused,
        sent: 250,
        failed: 3,
        total: 750,
        started_at: Utc::now(),
        remaining_brokers: vec!["acme".into(), "other".into()],
        search: "data".into(),
        category: "marketing".into(),
        region: "us".into(),
        status_filter: "never".into(),
        daily_limit: Some(250),
    }
}

#[test]
fn a_pending_job_round_trips_through_disk() {
    let dir = tempfile::tempdir().unwrap();
    let persistence = JobPersistence::new(dir.path().join("nested"));

    persistence.save(&pending()).unwrap();
    let loaded = persistence.load().expect("the job should be there");

    assert_eq!(loaded.remaining_brokers, ["acme", "other"]);
    assert_eq!(loaded.sent, 250);
    assert_eq!(loaded.region, "us");
}

#[test]
fn loading_finds_nothing_when_no_job_is_pending() {
    let dir = tempfile::tempdir().unwrap();
    assert!(JobPersistence::new(dir.path()).load().is_none());
}

/// A half-written file must not stop the server from starting.
#[test]
fn a_corrupt_pending_job_file_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let persistence = JobPersistence::new(dir.path());
    std::fs::write(persistence.file_path(), "{ not json").unwrap();

    assert!(persistence.load().is_none());
}

#[test]
fn clearing_removes_the_file_and_is_safe_to_repeat() {
    let dir = tempfile::tempdir().unwrap();
    let persistence = JobPersistence::new(dir.path());
    persistence.save(&pending()).unwrap();

    persistence.clear().unwrap();
    persistence.clear().unwrap();
    assert!(persistence.load().is_none());
}

/// The file names every broker still to be contacted — a list of who this
/// person is trying to remove themselves from.
#[cfg(unix)]
#[test]
fn the_pending_job_file_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let persistence = JobPersistence::new(dir.path());
    persistence.save(&pending()).unwrap();

    let mode = std::fs::metadata(persistence.file_path())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}
