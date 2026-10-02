use super::*;

#[test]
fn old_runner_claims_legacy_jobs_but_not_new_unbounded_jobs() {
    let mut f = Fixture::new();
    let origin = f
        .work
        .submit_for_device(
            &f.alice,
            &SubmitJob {
                request_id: "origin-capability".into(),
                project_id: "alpha".into(),
                project_participation: None,
                conversation_id: None,
                revises_job_id: None,
                expected_revised_revision: None,
                environment_id: "solver".into(),
                origin_session_ref: Some("local-session".into()),
                origin_turn_ref: None,
                origin_turn_epoch: None,
                title: "From local chat".into(),
                input: json!({"prompt":"test"}),
                descendant_budget: 0,
                start_before_ms: None,
            },
            10,
            Some("WinA"),
        )
        .unwrap();
    let plain = f.submit("solver", "plain", 0);
    f.work
        .db
        .execute(
            "UPDATE shared_jobs SET required_capability=NULL WHERE id=?1",
            [&plain.id],
        )
        .unwrap();
    let old = f
        .work
        .claim_for_capability("WinE", None, None, 11)
        .unwrap()
        .unwrap();
    assert_eq!(old.job.id, plain.id);
    assert!(old.required_runner_capabilities.is_empty());
    assert_eq!(
        f.work
            .assignments_for_capability("WinE", None)
            .unwrap()
            .len(),
        1
    );
    f.report(&old, "plain-start", ReportOutcome::Started);
    f.report(&old, "plain-done", finished());
    assert!(
        f.work
            .claim_for_capability("WinE", None, None, 22)
            .unwrap()
            .is_none()
    );
    let waiting = f.work.status(&f.alice, "alpha", None, 20, None).unwrap();
    assert_eq!(
        waiting
            .jobs
            .iter()
            .find(|item| item.id == origin.id)
            .unwrap()
            .wait_reason
            .as_deref()
            .unwrap(),
        "実行端末の受付を待っています。",
        "a required capability is not evidence of an incompatible Runner"
    );
    let new = f
        .work
        .claim_for_capability(
            "WinE",
            None,
            Some(store::MULTI_DEVICE_SESSION_CAPABILITY),
            23,
        )
        .unwrap()
        .unwrap();
    assert_eq!(new.job.id, origin.id);
    assert_eq!(
        new.required_runner_capabilities,
        [store::MULTI_DEVICE_SESSION_CAPABILITY]
    );
    assert!(
        f.work
            .claim_for_capability("WinE", None, None, 24)
            .unwrap()
            .is_none()
    );
    assert!(
        f.work
            .assignments_for_capability("WinE", None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.work
            .assignments_for_capability("WinE", Some(store::MULTI_DEVICE_SESSION_CAPABILITY))
            .unwrap()[0]
            .attempt_id,
        new.attempt_id
    );
    assert_eq!(
        f.work
            .claim_for_capability(
                "WinE",
                None,
                Some(store::MULTI_DEVICE_SESSION_CAPABILITY),
                25
            )
            .unwrap()
            .unwrap()
            .attempt_id,
        new.attempt_id
    );
}

#[test]
fn retaining_service_upgrades_plain_conversation_before_old_runner_redelivery() {
    let mut f = Fixture::new();
    let plain = f.submit("solver", "plain-preview", 0);
    f.work
        .db
        .execute(
            "UPDATE shared_jobs SET required_capability=NULL WHERE id=?1",
            [&plain.id],
        )
        .unwrap();
    let assignment = f
        .work
        .claim_for_capability(
            "WinE",
            None,
            Some(store::MULTI_DEVICE_SESSION_CAPABILITY),
            11,
        )
        .unwrap()
        .unwrap();
    assert_eq!(assignment.job.id, plain.id);
    assert!(assignment.required_runner_capabilities.is_empty());
    f.report(&assignment, "started", ReportOutcome::Started);
    f.report(
        &assignment,
        "retained",
        ReportOutcome::RetainService {
            service_id: "preview-capability".into(),
            expires_at_ms: Some(1000),
        },
    );
    assert_eq!(
        f.work
            .assignments_for_capability("WinE", None)
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        f.work
            .assignments_for_capability("WinE", Some(store::MULTI_DEVICE_SESSION_CAPABILITY))
            .unwrap()[0]
            .required_runner_capabilities,
        [store::MULTI_DEVICE_SESSION_CAPABILITY]
    );
    // A service still occupies the resource; a prior binary cannot rejoin the
    // same conversation even if the current attempt later yields.
    f.report(&assignment, "finished", finished());
    assert!(
        f.work
            .claim_for_capability("WinE", None, None, 22)
            .unwrap()
            .is_none()
    );
}

#[test]
fn current_runner_accepts_legacy_v1_but_v1_runner_cannot_start_new_lifetime_contract() {
    let mut f = Fixture::new();
    let legacy = f.submit("solver", "old-v1", 0);
    f.work
        .db
        .execute(
            "UPDATE shared_jobs SET required_capability='multi_device_session_v1' WHERE id=?1",
            [&legacy.id],
        )
        .unwrap();
    let attempt = f
        .work
        .claim_for_capability(
            "WinE",
            None,
            Some(store::MULTI_DEVICE_SESSION_CAPABILITY),
            11,
        )
        .unwrap()
        .unwrap();
    assert_eq!(attempt.job.id, legacy.id);
    f.report(&attempt, "start", ReportOutcome::Started);
    f.report(&attempt, "finish", finished());
    let current = f.submit("solver", "current", 0);
    assert!(
        f.work
            .claim_for_capability("WinE", None, Some("multi_device_session_v1"), 21)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        f.work.claim("WinE", 22).unwrap().unwrap().job.id,
        current.id
    );
}
