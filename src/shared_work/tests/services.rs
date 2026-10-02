use super::*;

#[test]
fn completed_turn_with_retained_service_cannot_be_revised_until_service_stops() {
    let mut f = Fixture::new();
    let root = f.submit("solver", "retained-revise", 0);
    let attempt = f.work.claim("WinE", 11).unwrap().unwrap();
    f.report(&attempt, "start-retained-revise", ReportOutcome::Started);
    f.report(
        &attempt,
        "retain-before-revise",
        ReportOutcome::RetainService {
            service_id: "preview-for-revise".into(),
            expires_at_ms: Some(1000),
        },
    );
    f.report(&attempt, "finish-before-revise", finished());
    let complete = f.work.job(&f.alice, &root.id).unwrap();
    assert_eq!(complete.state, JobState::Succeeded);
    assert_eq!(
        f.work.job_detail_at(&f.alice, &root.id, 21).unwrap()["can_revise"],
        false
    );
    let revised = SubmitJob {
        request_id: "revised-after-service".into(),
        project_id: "alpha".into(),
        project_participation: None,
        conversation_id: Some(root.id.clone()),
        revises_job_id: Some(root.id.clone()),
        expected_revised_revision: Some(complete.revision),
        environment_id: "solver".into(),
        origin_session_ref: None,
        origin_turn_ref: None,
        origin_turn_epoch: None,
        title: "edited".into(),
        input: json!({"prompt":"edit"}),
        descendant_budget: 0,
        start_before_ms: None,
    };
    assert!(matches!(
        f.work.submit(&f.alice, &revised, 21),
        Err(WorkError::Conflict)
    ));
    f.work
        .request_service_stop(&f.alice, "preview-for-revise", 22)
        .unwrap();
    f.work
        .report(
            "WinE",
            &Report {
                event_id: "service-stopped-for-revise".into(),
                attempt_id: attempt.attempt_id,
                generation: attempt.generation,
                outcome: ReportOutcome::ServiceStopped {
                    service_id: "preview-for-revise".into(),
                },
            },
            23,
        )
        .unwrap();
    assert_eq!(
        f.work.job_detail_at(&f.alice, &root.id, 24).unwrap()["can_revise"],
        true
    );
    assert_eq!(
        f.work
            .submit(&f.alice, &revised, 24)
            .unwrap()
            .continued_from,
        None
    );
}

#[test]
fn retained_service_keeps_capacity_and_rejoins_only_its_conversation() {
    let mut f = Fixture::new();
    let root = f.submit("solver", "host", 1);
    let first = f.work.claim("WinE", 11).unwrap().unwrap();
    f.report(&first, "started", ReportOutcome::Started);
    f.report(
        &first,
        "retained",
        ReportOutcome::RetainService {
            service_id: "server-1".into(),
            expires_at_ms: Some(1000),
        },
    );
    f.report(
        &first,
        "yield",
        ReportOutcome::YieldToChild {
            checkpoint: json!({"version":1}),
            child_environment_id: "parent".into(),
            child_title: "Check from the other PC".into(),
            child_input: json!({"version":1,"prompt":"Check"}),
            resources_released: true,
        },
    );
    let services = f.work.runner_services("WinE", 21).unwrap();
    assert_eq!(services.len(), 1);
    assert_eq!(services[0].conversation_id, root.id);
    assert!(services[0].can_stop);
    let detail = f.work.job_detail_at(&f.alice, &root.id, 21).unwrap();
    assert_eq!(detail["retained_services"][0]["service_id"], "server-1");
    assert_eq!(detail["retained_services"][0]["can_stop"], true);
    assert_eq!(
        f.work
            .project_services(&f.alice, "alpha", Some(&root.id), 21)
            .unwrap()
            .len(),
        1
    );
    assert!(
        f.work
            .project_services(&f.bob, "alpha", Some(&root.id), 21)
            .is_err()
    );
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "alpha",
        &f.bob.user_id,
        Some(ProjectRole::Reader),
        21,
    )
    .unwrap();
    assert!(
        !f.work
            .project_services(&f.bob, "alpha", Some(&root.id), 21)
            .unwrap()[0]
            .can_stop
    );
    assert!(f.work.request_service_stop(&f.bob, "server-1", 21).is_err());

    let other = f.submit("solver", "other", 0);
    assert!(
        f.work.job_detail_at(&f.alice, &other.id, 22).unwrap()["wait_reason"]
            .as_str()
            .unwrap()
            .contains("利用枠")
    );
    assert!(f.work.claim("WinE", 22).unwrap().is_none());
    let child = f.work.claim("WinC", 22).unwrap().unwrap();
    assert_eq!(child.job.conversation_id, root.id);
    f.report(&child, "child-started", ReportOutcome::Started);
    let discovered = f
        .work
        .conversation_services_for_attempt("WinC", &child.attempt_id, child.generation, 22)
        .unwrap();
    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].service_id, "server-1");
    assert!(matches!(
        f.work.conversation_services_for_attempt(
            "WinC",
            &child.attempt_id,
            child.generation + 1,
            22,
        ),
        Err(WorkError::Forbidden)
    ));
    assert!(matches!(
        f.work
            .conversation_services_for_attempt("WinE", &child.attempt_id, child.generation, 22,),
        Err(WorkError::Forbidden)
    ));
    f.report(
        &child,
        "child-finished",
        ReportOutcome::Finished {
            success: true,
            result: json!({"reachable":true}),
            resources_released: true,
        },
    );
    assert_eq!(
        f.work.job_detail_at(&f.alice, &root.id, 23).unwrap()["wait_reason"],
        "実行端末の受付を待っています。",
        "this conversation can reuse its own retained service capacity"
    );
    let resumed = f.work.claim("WinE", 23).unwrap().unwrap();
    assert_eq!(resumed.job.id, root.id);
    assert_eq!(resumed.retained_services.len(), 1);
    assert_eq!(resumed.retained_services[0].service_id, "server-1");
    assert_eq!(resumed.retained_services[0].attempt_id, first.attempt_id);
    assert!(matches!(
        f.work.request_conversation_service_stop(
            "WinE",
            &resumed.attempt_id,
            resumed.generation + 1,
            "server-1",
            24,
        ),
        Err(WorkError::Forbidden)
    ));
    f.report(&resumed, "resumed", ReportOutcome::Started);
    assert!(matches!(
        f.work.request_conversation_service_stop(
            "WinC",
            &resumed.attempt_id,
            resumed.generation,
            "server-1",
            24,
        ),
        Err(WorkError::Forbidden)
    ));
    assert!(
        f.work
            .request_conversation_service_stop(
                "WinE",
                &resumed.attempt_id,
                resumed.generation,
                "server-1",
                24,
            )
            .unwrap()
            .stop_requested
    );
    assert!(matches!(
        f.work.request_local_service_stop("WinC", "server-1", 24),
        Err(WorkError::Forbidden)
    ));
    let stop = Report {
        event_id: "server-stopped".into(),
        attempt_id: first.attempt_id.clone(),
        generation: first.generation,
        outcome: ReportOutcome::ServiceStopped {
            service_id: "server-1".into(),
        },
    };
    f.work.report("WinE", &stop, 25).unwrap();
    f.work.report("WinE", &stop, 26).unwrap();
    assert!(f.work.runner_services("WinE", 26).unwrap().is_empty());
    f.report(
        &resumed,
        "finished",
        ReportOutcome::Finished {
            success: true,
            result: json!({"done":true}),
            resources_released: true,
        },
    );
    assert_eq!(f.work.claim("WinE", 27).unwrap().unwrap().job.id, other.id);
}

#[test]
fn cancellation_requests_stop_but_keeps_capacity_until_runner_confirms() {
    let mut f = Fixture::new();
    let root = f.submit("solver", "cancel-host", 0);
    let assignment = f.work.claim("WinE", 11).unwrap().unwrap();
    f.report(&assignment, "started", ReportOutcome::Started);
    f.report(
        &assignment,
        "retain",
        ReportOutcome::RetainService {
            service_id: "cancel-server".into(),
            expires_at_ms: Some(1000),
        },
    );
    let other = f.submit("solver", "next", 0);
    f.work.cancel(&f.alice, &root.id, 21).unwrap();
    assert!(f.work.runner_services("WinE", 22).unwrap()[0].stop_requested);
    let redelivery = f.work.claim("WinE", 22).unwrap().unwrap();
    assert_eq!(redelivery.attempt_id, assignment.attempt_id);
    assert!(redelivery.stop_requested);
    f.work
        .report(
            "WinE",
            &Report {
                event_id: "stopped".into(),
                attempt_id: assignment.attempt_id.clone(),
                generation: assignment.generation,
                outcome: ReportOutcome::ServiceStopped {
                    service_id: "cancel-server".into(),
                },
            },
            23,
        )
        .unwrap();
    assert_eq!(
        f.work.claim("WinE", 23).unwrap().unwrap().attempt_id,
        assignment.attempt_id
    );
    f.report(
        &assignment,
        "cancel-finished",
        ReportOutcome::Finished {
            success: false,
            result: json!({"cancelled":true}),
            resources_released: true,
        },
    );
    assert_eq!(f.work.claim("WinE", 24).unwrap().unwrap().job.id, other.id);
}

#[test]
fn successful_turn_can_leave_a_finite_service_for_the_next_turn() {
    let mut f = Fixture::new();
    let root = f.submit("solver", "preview", 0);
    let assignment = f.work.claim("WinE", 11).unwrap().unwrap();
    f.report(&assignment, "started", ReportOutcome::Started);
    f.report(
        &assignment,
        "retained",
        ReportOutcome::RetainService {
            service_id: "preview-server".into(),
            expires_at_ms: Some(1000),
        },
    );
    f.report(
        &assignment,
        "finished",
        ReportOutcome::Finished {
            success: true,
            result: json!({"preview":"ready"}),
            resources_released: true,
        },
    );
    let service = &f.work.runner_services("WinE", 21).unwrap()[0];
    assert_eq!(service.conversation_id, root.id);
    assert!(!service.stop_requested);
    assert_eq!(
        store::resource_occupancy(&f.work.db, "wine-slot").unwrap(),
        1
    );
    let other = f.submit("solver", "blocked-by-preview", 0);
    assert!(f.work.claim("WinE", 22).unwrap().is_none());
    assert!(f.work.runner_services("WinE", 1000).unwrap()[0].stop_requested);
    f.work
        .report(
            "WinE",
            &Report {
                event_id: "preview-stopped".into(),
                attempt_id: assignment.attempt_id.clone(),
                generation: assignment.generation,
                outcome: ReportOutcome::ServiceStopped {
                    service_id: "preview-server".into(),
                },
            },
            1001,
        )
        .unwrap();
    assert_eq!(
        f.work.claim("WinE", 1002).unwrap().unwrap().job.id,
        other.id
    );
}

#[test]
fn revoked_execution_authority_requests_service_stop_without_releasing_capacity() {
    let mut f = Fixture::new();
    let job = f.submit("solver", "revoke-host", 0);
    let assignment = f.work.claim("WinE", 11).unwrap().unwrap();
    f.report(&assignment, "started", ReportOutcome::Started);
    f.report(
        &assignment,
        "retained",
        ReportOutcome::RetainService {
            service_id: "revoked-server".into(),
            expires_at_ms: Some(1000),
        },
    );
    identity::set_project_role(&f.work.db, &f.admin, "alpha", &f.alice.user_id, None, 21).unwrap();
    assert!(f.work.runner_services("WinE", 22).unwrap()[0].stop_requested);
    assert_eq!(
        store::resource_occupancy(&f.work.db, "wine-slot").unwrap(),
        1
    );
    f.work
        .report(
            "WinE",
            &Report {
                event_id: "stop-after-revoke".into(),
                attempt_id: assignment.attempt_id.clone(),
                generation: assignment.generation,
                outcome: ReportOutcome::ServiceStopped {
                    service_id: "revoked-server".into(),
                },
            },
            23,
        )
        .unwrap();
    assert!(f.work.runner_services("WinE", 24).unwrap().is_empty());
    assert_eq!(
        f.work
            .db
            .query_row("SELECT state FROM shared_jobs WHERE id=?1", [job.id], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "running"
    );
}

#[test]
fn conversation_stop_requests_every_owned_lease_atomically_and_waits_for_ack() {
    let mut f = Fixture::new();
    let root = f.submit("solver", "all-stop", 1);
    let first = f.work.claim("WinE", 11).unwrap().unwrap();
    f.report(&first, "first-started", ReportOutcome::Started);
    f.report(
        &first,
        "first-retained",
        ReportOutcome::RetainService {
            service_id: "all-stop-first".into(),
            expires_at_ms: Some(1000),
        },
    );
    f.report(
        &first,
        "first-yield",
        ReportOutcome::YieldToChild {
            checkpoint: json!({"version":1}),
            child_environment_id: "parent".into(),
            child_title: "Check".into(),
            child_input: json!({"version":1,"prompt":"Check"}),
            resources_released: true,
        },
    );
    let child = f.work.claim("WinC", 22).unwrap().unwrap();
    f.report(&child, "child-started", ReportOutcome::Started);
    f.report(
        &child,
        "child-retained",
        ReportOutcome::RetainService {
            service_id: "all-stop-child".into(),
            expires_at_ms: Some(1000),
        },
    );
    f.report(
        &child,
        "child-finished",
        ReportOutcome::Finished {
            success: true,
            result: json!({"checked":true}),
            resources_released: true,
        },
    );
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "alpha",
        &f.bob.user_id,
        Some(ProjectRole::Contributor),
        23,
    )
    .unwrap();
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "alpha",
        &f.admin.user_id,
        Some(ProjectRole::Manager),
        23,
    )
    .unwrap();
    // Model a completed handover: two retained leases in one conversation now
    // have different assignees. A contributor cannot stop only their subset.
    f.work
        .db
        .execute(
            "UPDATE shared_jobs SET assignee_id=?2 WHERE id=?1",
            rusqlite::params![child.job.id, f.bob.user_id],
        )
        .unwrap();
    assert!(matches!(
        f.work
            .request_conversation_services_stop(&f.bob, "alpha", &root.id, "foreign-stop", 24),
        Err(WorkError::Forbidden)
    ));
    assert!(matches!(
        f.work
            .request_conversation_services_stop(&f.alice, "alpha", &root.id, "partial-stop", 24),
        Err(WorkError::Forbidden)
    ));
    assert!(
        f.work
            .project_services(&f.alice, "alpha", Some(&root.id), 24)
            .unwrap()
            .iter()
            .all(|service| !service.stop_requested)
    );
    let receipt = f
        .work
        .request_conversation_services_stop(&f.admin, "alpha", &root.id, "all-stop-1", 25)
        .unwrap();
    assert_eq!(receipt["accepted"], true);
    assert_eq!(receipt["conversation_id"], root.id);
    assert_eq!(
        f.work
            .request_conversation_services_stop(&f.admin, "alpha", &root.id, "all-stop-1", 26)
            .unwrap(),
        receipt
    );
    assert!(
        f.work
            .project_services(&f.alice, "alpha", Some(&root.id), 26)
            .unwrap()
            .iter()
            .all(|service| service.stop_requested)
    );
    assert_eq!(
        store::resource_occupancy(&f.work.db, "wine-slot").unwrap(),
        1
    );
    assert_eq!(
        store::resource_occupancy(&f.work.db, "parent-slot").unwrap(),
        1
    );
    for (assignment, service_id, event) in [
        (&first, "all-stop-first", "first-stopped"),
        (&child, "all-stop-child", "child-stopped"),
    ] {
        f.work
            .report(
                &assignment.runner_id,
                &Report {
                    event_id: event.into(),
                    attempt_id: assignment.attempt_id.clone(),
                    generation: assignment.generation,
                    outcome: ReportOutcome::ServiceStopped {
                        service_id: service_id.into(),
                    },
                },
                27,
            )
            .unwrap();
    }
    assert!(f.work.runner_services("WinE", 28).unwrap().is_empty());
    assert!(f.work.runner_services("WinC", 28).unwrap().is_empty());
    assert_eq!(
        store::resource_occupancy(&f.work.db, "wine-slot").unwrap(),
        0
    );
    assert_eq!(
        store::resource_occupancy(&f.work.db, "parent-slot").unwrap(),
        0
    );
}

#[test]
fn conversation_stop_retry_does_not_stop_later_service_or_reuse_another_scope() {
    let mut f = Fixture::new();
    let root = f.submit("solver", "first-preview", 1);
    let first = f.work.claim("WinE", 11).unwrap().unwrap();
    f.report(&first, "first-started", ReportOutcome::Started);
    f.report(
        &first,
        "first-retained",
        ReportOutcome::RetainService {
            service_id: "first-preview-service".into(),
            expires_at_ms: Some(1000),
        },
    );
    f.report(
        &first,
        "first-yield",
        ReportOutcome::YieldToChild {
            checkpoint: json!({"version":1}),
            child_environment_id: "parent".into(),
            child_title: "Check".into(),
            child_input: json!({"version":1,"prompt":"Check"}),
            resources_released: true,
        },
    );
    let child = f.work.claim("WinC", 20).unwrap().unwrap();
    f.report(&child, "child-started", ReportOutcome::Started);
    f.report(
        &child,
        "child-finished",
        ReportOutcome::Finished {
            success: true,
            result: json!({"checked":true}),
            resources_released: true,
        },
    );
    f.work
        .request_conversation_services_stop(&f.alice, "alpha", &root.id, "stop-once", 21)
        .unwrap();
    let db_path = f.dir.path().join("shared.sqlite3");
    f.work = SharedWork::open(&db_path).unwrap();
    f.work
        .report(
            "WinE",
            &Report {
                event_id: "first-stopped".into(),
                attempt_id: first.attempt_id,
                generation: first.generation,
                outcome: ReportOutcome::ServiceStopped {
                    service_id: "first-preview-service".into(),
                },
            },
            22,
        )
        .unwrap();
    let second = f.work.claim("WinE", 24).unwrap().unwrap();
    assert_eq!(second.job.conversation_id, root.id);
    f.report(&second, "second-started", ReportOutcome::Started);
    f.report(
        &second,
        "second-retained",
        ReportOutcome::RetainService {
            service_id: "second-preview-service".into(),
            expires_at_ms: Some(1000),
        },
    );
    let other = f.submit("parent", "other-conversation", 0);
    assert!(matches!(
        f.work
            .request_conversation_services_stop(&f.alice, "alpha", &other.id, "stop-once", 25),
        Err(WorkError::Conflict)
    ));
    f.work
        .request_conversation_services_stop(&f.alice, "alpha", &root.id, "stop-once", 26)
        .unwrap();
    assert!(!f.work.runner_services("WinE", 26).unwrap()[0].stop_requested);
    assert_eq!(
        store::resource_occupancy(&f.work.db, "wine-slot").unwrap(),
        1
    );
}

#[test]
fn unbounded_preview_and_another_queued_conversation_survive_seventy_two_hours() {
    let mut f = Fixture::new();
    let root = f.submit("solver", "long-preview", 0);
    let attempt = f.work.claim("WinE", 11).unwrap().unwrap();
    f.report(&attempt, "start", ReportOutcome::Started);
    f.report(
        &attempt,
        "retain",
        ReportOutcome::RetainService {
            service_id: "preview-unbounded".into(),
            expires_at_ms: None,
        },
    );
    f.report(&attempt, "done", finished());
    let queued = f.submit("solver", "later-conversation", 0);
    let later = 73 * 60 * 60 * 1000;
    f.reopen();
    f.work.expire_start_deadlines(later).unwrap();
    let service = f.work.runner_services("WinE", later).unwrap().remove(0);
    assert_eq!(service.expires_at_ms, None);
    assert!(!service.stop_requested);
    assert!(f.work.claim("WinE", later).unwrap().is_none());
    assert_eq!(
        f.work.job(&f.alice, &queued.id).unwrap().state,
        JobState::Queued
    );
    assert_eq!(
        f.work.job(&f.alice, &root.id).unwrap().state,
        JobState::Succeeded
    );
    f.work
        .request_service_stop(&f.alice, "preview-unbounded", later + 1)
        .unwrap();
    assert!(f.work.claim("WinE", later + 2).unwrap().is_none());
    f.work
        .report(
            "WinE",
            &Report {
                event_id: "exact-stop".into(),
                attempt_id: attempt.attempt_id,
                generation: attempt.generation,
                outcome: ReportOutcome::ServiceStopped {
                    service_id: "preview-unbounded".into(),
                },
            },
            later + 3,
        )
        .unwrap();
    assert_eq!(
        f.work.claim("WinE", later + 4).unwrap().unwrap().job.id,
        queued.id
    );
}

#[test]
fn explicit_legacy_preview_deadline_survives_forward_migration_and_requires_stop_ack() {
    let mut f = Fixture::new();
    f.submit("solver", "finite-preview", 0);
    let attempt = f.work.claim("WinE", 11).unwrap().unwrap();
    f.report(&attempt, "start", ReportOutcome::Started);
    f.report(
        &attempt,
        "retain",
        ReportOutcome::RetainService {
            service_id: "finite-preview".into(),
            expires_at_ms: Some(1000),
        },
    );
    f.report(&attempt, "done", finished());
    f.work.db.execute_batch("PRAGMA user_version=25").unwrap();
    f.reopen();
    let service = f.work.runner_services("WinE", 1000).unwrap().remove(0);
    assert_eq!(service.expires_at_ms, Some(1000));
    assert!(service.stop_requested);
    f.submit("solver", "blocked-until-stop", 0);
    assert!(f.work.claim("WinE", 1001).unwrap().is_none());
}
