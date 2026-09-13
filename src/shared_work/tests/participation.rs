use super::*;

fn local(f: &Fixture, key: &str) -> LocalAdmission {
    let login = identity::login(
        &f.work.db,
        "alice",
        "test-password-12345",
        identity::SessionAudience::SharedWork,
        Some("desktop"),
        10,
    )
    .unwrap();
    LocalAdmission {
        request_id: key.into(),
        environment_id: "solver".into(),
        project_id: "alpha".into(),
        actor_device_id: "desktop".into(),
        principal_session: login.token,
        source: "local".into(),
        title: "Local user work".into(),
    }
}
#[test]
fn local_participant_and_shared_claim_reserve_one_capacity_before_execution() {
    let mut f = Fixture::new();
    let request = local(&f, "local-one");
    let local = f.work.admit_local("WinE", &request, 11).unwrap();
    assert_eq!(local.job.assignee_id, f.alice.user_id);
    assert_eq!(local.job.state, JobState::Running);
    let queued = f.submit("solver", "shared-waiting", 0);
    assert!(f.work.claim("WinE", 12).unwrap().is_none());
    f.reopen();
    let retried = f.work.admit_local("WinE", &request, 13).unwrap();
    assert_eq!(retried.attempt_id, local.attempt_id);
    assert_eq!(f.work.assignments("WinE").unwrap().len(), 1);
    f.report(&local, "local-finished", finished());
    let shared = f.work.claim("WinE", 21).unwrap().unwrap();
    assert_eq!(shared.job.id, queued.id);
    let second = local_request_after(&f, "local-two");
    let before: i64 = f
        .work
        .db
        .query_row("SELECT COUNT(*) FROM shared_jobs", [], |row| row.get(0))
        .unwrap();
    assert!(matches!(
        f.work.admit_local("WinE", &second, 22),
        Err(WorkError::Capacity)
    ));
    let after: i64 = f
        .work
        .db
        .query_row("SELECT COUNT(*) FROM shared_jobs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        before, after,
        "Failed capacity reservation must not leave a queued local job"
    );
}
fn local_request_after(f: &Fixture, key: &str) -> LocalAdmission {
    local(f, key)
}

#[test]
fn local_participant_does_not_accept_claimed_user_or_wrong_device_session() {
    let mut f = Fixture::new();
    let mut request = local(&f, "local-auth");
    request.actor_device_id = "other-device".into();
    assert!(f.work.admit_local("WinE", &request, 11).is_err());
    request.actor_device_id = "desktop".into();
    request.project_id = "beta".into();
    assert!(f.work.admit_local("WinE", &request, 12).is_err());
    assert!(f.work.assignments("WinE").unwrap().is_empty());
}

#[test]
fn provision_catalog_is_stable_and_exact_generation_fences_old_receipts() {
    let mut f = Fixture::new();
    let templates = vec![RunnerTemplate {
        id: "approved".into(),
        label: "Approved".into(),
    }];
    f.work.publish_templates("WinE", &templates, 10).unwrap();
    let before = f.work.provision_catalog(&f.admin).unwrap();
    f.work.publish_templates("WinE", &templates, 11).unwrap();
    assert_eq!(before, f.work.provision_catalog(&f.admin).unwrap());
    f.work
        .request_provision(
            &f.admin,
            &ProvisionRequest {
                environment_id: "solver".into(),
                template_id: "approved".into(),
                generation: 0,
            },
            12,
        )
        .unwrap();
    assert_eq!(f.work.pending_provision("WinE").unwrap()[0].generation, 1);
    let failed = ProvisionReceipt {
        environment_id: "solver".into(),
        template_id: "approved".into(),
        generation: 1,
        success: false,
        error: Some("Operator directory was not available".into()),
    };
    f.work.provision_receipt("WinE", &failed, 13).unwrap();
    f.work.provision_receipt("WinE", &failed, 14).unwrap();
    f.work
        .request_provision(
            &f.admin,
            &ProvisionRequest {
                environment_id: "solver".into(),
                template_id: "approved".into(),
                generation: 1,
            },
            15,
        )
        .unwrap();
    assert_eq!(f.work.pending_provision("WinE").unwrap()[0].generation, 2);
    assert!(f.work.provision_receipt("WinE", &failed, 16).is_err());
    f.work
        .provision_receipt(
            "WinE",
            &ProvisionReceipt {
                generation: 2,
                success: true,
                error: None,
                ..failed
            },
            17,
        )
        .unwrap();
    assert!(f.work.pending_provision("WinE").unwrap().is_empty());
}
