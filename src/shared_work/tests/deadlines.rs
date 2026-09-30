use super::*;

fn request(key: &str, environment: &str, deadline: Option<u64>) -> SubmitJob {
    SubmitJob {
        request_id: key.into(),
        project_id: "alpha".into(),
        project_participation: None,
        conversation_id: None,
        revises_job_id: None,
        expected_revised_revision: None,
        environment_id: environment.into(),
        origin_session_ref: None,
        origin_turn_ref: None,
        origin_turn_epoch: None,
        title: key.into(),
        input: json!({"version":1,"prompt":"test"}),
        descendant_budget: 2,
        start_before_ms: deadline,
    }
}
fn report_at(
    f: &mut Fixture,
    assignment: &Assignment,
    event: &str,
    outcome: ReportOutcome,
    now: u64,
) -> Result<Job> {
    f.work.report(
        &assignment.runner_id,
        &Report {
            event_id: event.into(),
            attempt_id: assignment.attempt_id.clone(),
            generation: assignment.generation,
            outcome,
        },
        now,
    )
}

const LONG_WAIT: u64 = 73 * 60 * 60 * 1000;

#[test]
fn ordinary_queue_and_receipt_survive_more_than_seventy_two_hours() {
    let mut f = Fixture::new();
    let original = request("long-queue", "solver", None);
    let job = f.work.submit(&f.alice, &original, 10).unwrap();
    assert_eq!(job.start_before_ms, None);
    f.reopen();
    assert_eq!(
        f.work.submit(&f.alice, &original, LONG_WAIT).unwrap().id,
        job.id
    );
    f.work.expire_start_deadlines(LONG_WAIT).unwrap();
    let assigned = f.work.claim("WinE", LONG_WAIT).unwrap().unwrap();
    assert_eq!(assigned.job.id, job.id);
    report_at(
        &mut f,
        &assigned,
        "start",
        ReportOutcome::Started,
        LONG_WAIT + 1,
    )
    .unwrap();
    report_at(&mut f, &assigned, "finish", finished(), LONG_WAIT + 2).unwrap();
    assert_eq!(
        f.work.job(&f.alice, &job.id).unwrap().state,
        JobState::Succeeded
    );
}

#[test]
fn explicit_legacy_queue_deadline_is_validated_and_receipt_replay_cannot_extend_it() {
    let mut f = Fixture::new();
    let original = request("explicit", "solver", Some(100));
    let job = f.work.submit(&f.alice, &original, 10).unwrap();
    assert_eq!(job.start_before_ms, Some(100));
    f.reopen();
    assert_eq!(
        f.work
            .submit(&f.alice, &original, 101)
            .unwrap()
            .start_before_ms,
        Some(100)
    );
    let changed = SubmitJob {
        start_before_ms: Some(1000),
        ..original
    };
    assert!(matches!(
        f.work.submit(&f.alice, &changed, 20),
        Err(WorkError::Conflict)
    ));
    for value in [0, 30, u64::MAX] {
        assert!(
            f.work
                .submit(
                    &f.alice,
                    &request(&format!("invalid-{value}"), "solver", Some(value)),
                    30
                )
                .is_err()
        );
    }
    assert!(f.work.claim("WinE", 101).unwrap().is_none());
    assert_eq!(
        f.work.job(&f.alice, &job.id).unwrap().state,
        JobState::Failed
    );
}

#[test]
fn child_and_parent_resume_have_no_inherited_deadline_even_after_restart() {
    for queued_before in [None, Some(100)] {
        let mut f = Fixture::new();
        let root = f
            .work
            .submit(&f.alice, &request("parent", "parent", queued_before), 10)
            .unwrap();
        let parent = f.work.claim("WinC", 11).unwrap().unwrap();
        report_at(&mut f, &parent, "start", ReportOutcome::Started, 12).unwrap();
        assert_eq!(
            f.work.job(&f.alice, &root.id).unwrap().start_before_ms,
            None
        );
        let waiting = report_at(&mut f, &parent, "yield", child(), LONG_WAIT).unwrap();
        assert_eq!(waiting.state, JobState::WaitingChild);
        let child_id = waiting.awaiting_child_id.clone().unwrap();
        assert_eq!(
            f.work.job(&f.alice, &child_id).unwrap().start_before_ms,
            None
        );
        f.reopen();
        let child = f.work.claim("WinE", LONG_WAIT * 2).unwrap().unwrap();
        assert_eq!(child.job.id, child_id);
        report_at(
            &mut f,
            &child,
            "start",
            ReportOutcome::Started,
            LONG_WAIT * 2 + 1,
        )
        .unwrap();
        report_at(&mut f, &child, "finish", finished(), LONG_WAIT * 3).unwrap();
        f.reopen();
        let resumed = f.work.claim("WinC", LONG_WAIT * 4).unwrap().unwrap();
        assert_eq!(resumed.job.id, root.id);
        assert!(resumed.child_result.is_some());
        report_at(
            &mut f,
            &resumed,
            "resume",
            ReportOutcome::Started,
            LONG_WAIT * 4 + 1,
        )
        .unwrap();
        report_at(&mut f, &resumed, "finish", finished(), LONG_WAIT * 4 + 2).unwrap();
        assert_eq!(
            f.work.job(&f.alice, &root.id).unwrap().state,
            JobState::Succeeded
        );
    }
}

#[test]
fn optional_lifetime_migration_clears_implicit_queue_and_started_deadlines_only() {
    let mut f = Fixture::new();
    let implicit = f.submit("solver", "implicit", 0);
    let explicit = f
        .work
        .submit(&f.alice, &request("explicit", "solver", Some(1000)), 10)
        .unwrap();
    let root = f
        .work
        .submit(&f.alice, &request("started", "parent", Some(1000)), 10)
        .unwrap();
    let parent = f.work.claim("WinC", 11).unwrap().unwrap();
    report_at(&mut f, &parent, "start", ReportOutcome::Started, 12).unwrap();
    report_at(&mut f, &parent, "yield", child(), 20).unwrap();
    let child_id = f
        .work
        .job(&f.alice, &root.id)
        .unwrap()
        .awaiting_child_id
        .unwrap();
    f.work
        .db
        .execute(
            "UPDATE shared_jobs SET start_before_ms=?1 WHERE id IN (?2,?3,?4)",
            rusqlite::params![10 + DEFAULT_START_WINDOW_MS, implicit.id, root.id, child_id],
        )
        .unwrap();
    f.work
        .db
        .execute(
            "UPDATE shared_jobs SET required_capability='multi_device_session_v1' WHERE id=?1",
            [&explicit.id],
        )
        .unwrap();
    f.work.db.execute_batch("PRAGMA user_version=25;").unwrap();
    f.reopen();
    assert!(
        f.work
            .claim_for_capability("WinE", None, Some("multi_device_session_v1"), 30)
            .unwrap()
            .is_none()
    );
    let explicit_capability: String = f
        .work
        .db
        .query_row(
            "SELECT required_capability FROM shared_jobs WHERE id=?1",
            [&explicit.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(explicit_capability, store::MULTI_DEVICE_SESSION_CAPABILITY);
    for id in [&implicit.id, &root.id, &child_id] {
        assert_eq!(f.work.job(&f.alice, id).unwrap().start_before_ms, None);
    }
    assert_eq!(
        f.work.job(&f.alice, &explicit.id).unwrap().start_before_ms,
        Some(1000)
    );
    f.work.cancel(&f.alice, &implicit.id, 31).unwrap();
    let explicit_assignment = f.work.claim("WinE", 32).unwrap().unwrap();
    assert_eq!(explicit_assignment.job.id, explicit.id);
    assert_eq!(explicit_assignment.job.start_before_ms, Some(1000));
    assert_eq!(
        f.work
            .db
            .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
            .unwrap(),
        26
    );
    f.reopen();
    assert_eq!(
        f.work.job(&f.alice, &root.id).unwrap().start_before_ms,
        None
    );
}
#[test]
fn shared_deadline_fences_unstarted_but_holds_unknown_and_allows_started_work_to_finish() {
    let mut f = Fixture::new();
    let job = f
        .work
        .submit(&f.alice, &request("unstarted", "solver", Some(100)), 10)
        .unwrap();
    let assigned = f.work.claim("WinE", 20).unwrap().unwrap();
    assert!(report_at(&mut f, &assigned, "late-start", ReportOutcome::Started, 100).is_err());
    assert_eq!(
        f.work.job(&f.alice, &job.id).unwrap().state,
        JobState::Failed
    );
    assert!(f.work.assignments("WinE").unwrap().is_empty());
    assert!(
        report_at(
            &mut f,
            &assigned,
            "late-start-again",
            ReportOutcome::Started,
            101
        )
        .is_err()
    );
    let unknown = f
        .work
        .submit(&f.alice, &request("unknown", "solver", Some(200)), 110)
        .unwrap();
    let assigned = f.work.claim("WinE", 120).unwrap().unwrap();
    report_at(
        &mut f,
        &assigned,
        "uncertain",
        ReportOutcome::Uncertain {
            reason: "delivery needs reconciliation".into(),
        },
        130,
    )
    .unwrap();
    assert!(report_at(&mut f, &assigned, "late-start", ReportOutcome::Started, 200).is_err());
    let held = f.work.assignments("WinE").unwrap();
    assert_eq!(held.len(), 1);
    assert!(held[0].stop_requested);
    assert!(!f.work.job(&f.alice, &unknown.id).unwrap().state.terminal());
    report_at(
        &mut f,
        &assigned,
        "stopped",
        ReportOutcome::Finished {
            success: false,
            result: json!({"stopped":true}),
            resources_released: true,
        },
        210,
    )
    .unwrap();
    assert!(f.work.assignments("WinE").unwrap().is_empty());
    let started = f
        .work
        .submit(
            &f.alice,
            &request("already-started", "solver", Some(300)),
            220,
        )
        .unwrap();
    let assigned = f.work.claim("WinE", 230).unwrap().unwrap();
    report_at(&mut f, &assigned, "start", ReportOutcome::Started, 240).unwrap();
    f.work.claim("WinE", 301).unwrap();
    assert_eq!(
        f.work.job(&f.alice, &started.id).unwrap().state,
        JobState::Running
    );
    // An acknowledged start stays acknowledged after the deadline; replay cannot allocate anew.
    report_at(&mut f, &assigned, "start", ReportOutcome::Started, 301).unwrap();
    report_at(&mut f, &assigned, "finish", finished(), 310).unwrap();
    assert_eq!(
        f.work.job(&f.alice, &started.id).unwrap().state,
        JobState::Succeeded
    );
}

#[test]
fn shared_deadline_offline_reconciliation_is_persisted_and_unknown_slots_still_block_capacity() {
    let mut f = Fixture::new();
    let unknown = f
        .work
        .submit(&f.alice, &request("unknown-held", "solver", Some(100)), 10)
        .unwrap();
    let attempt = f.work.claim("WinE", 20).unwrap().unwrap();
    report_at(
        &mut f,
        &attempt,
        "uncertain",
        ReportOutcome::Uncertain {
            reason: "unknown".into(),
        },
        30,
    )
    .unwrap();
    let queued = f
        .work
        .submit(
            &f.alice,
            &request("offline-queued", "parent", Some(100)),
            40,
        )
        .unwrap();
    f.work.expire_start_deadlines(100).unwrap();
    f.reopen();
    assert_eq!(
        f.work.job(&f.alice, &queued.id).unwrap().state,
        JobState::Failed
    );
    assert_eq!(
        f.work.job(&f.alice, &unknown.id).unwrap().state,
        JobState::Assigned
    );
    let detail = f.work.job_detail(&f.alice, &unknown.id).unwrap();
    assert!(
        detail["wait_reason"]
            .as_str()
            .unwrap()
            .contains("利用枠を保持")
    );
    let next = f
        .work
        .submit(&f.alice, &request("new-solver", "solver", Some(300)), 110)
        .unwrap();
    let held = f.work.claim("WinE", 120).unwrap().unwrap();
    assert_eq!(held.attempt_id, attempt.attempt_id);
    assert!(held.stop_requested);
    assert_eq!(
        f.work.job(&f.alice, &next.id).unwrap().state,
        JobState::Queued
    );
}

#[test]
fn migrated_active_attempt_requires_current_runner_without_releasing_or_replaying_it() {
    let mut f = Fixture::new();
    let root = f.submit("solver", "old-live", 0);
    let attempt = f.work.claim("WinE", 11).unwrap().unwrap();
    f.report(&attempt, "started", ReportOutcome::Started);
    f.work.db.execute("UPDATE shared_jobs SET start_before_ms=?2,required_capability='multi_device_session_v1' WHERE id=?1",rusqlite::params![root.id,10+DEFAULT_START_WINDOW_MS]).unwrap();
    f.work.db.execute_batch("PRAGMA user_version=25;").unwrap();
    f.reopen();
    assert!(
        f.work
            .assignments_for_capability("WinE", Some("multi_device_session_v1"))
            .unwrap()
            .is_empty()
    );
    assert!(
        f.work
            .claim_for_capability("WinE", None, Some("multi_device_session_v1"), LONG_WAIT)
            .unwrap()
            .is_none()
    );
    let current = f
        .work
        .assignments_for_capability("WinE", Some(store::MULTI_DEVICE_SESSION_CAPABILITY))
        .unwrap();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].attempt_id, attempt.attempt_id);
    assert_eq!(current[0].job.state, JobState::Running);
    assert_eq!(
        current[0].required_runner_capabilities,
        [store::MULTI_DEVICE_SESSION_CAPABILITY]
    );
    let queued = f.submit("solver", "other", 0);
    assert!(f.work.claim("WinE", LONG_WAIT + 1).unwrap().is_none());
    report_at(
        &mut f,
        &attempt,
        "old-runner-unknown",
        ReportOutcome::Uncertain {
            reason: "old Runner stopped after capability fence".into(),
        },
        LONG_WAIT + 2,
    )
    .unwrap();
    assert_eq!(f.work.assignments("WinE").unwrap().len(), 1);
    assert!(f.work.claim("WinE", LONG_WAIT + 3).unwrap().is_none());
    report_at(
        &mut f,
        &attempt,
        "old-runner-drained",
        ReportOutcome::Finished {
            success: false,
            result: json!({"stopped":true}),
            resources_released: true,
        },
        LONG_WAIT + 4,
    )
    .unwrap();
    assert_eq!(
        f.work.job(&f.alice, &root.id).unwrap().state,
        JobState::Failed
    );
    assert_eq!(
        f.work.claim("WinE", LONG_WAIT + 5).unwrap().unwrap().job.id,
        queued.id
    );
}
