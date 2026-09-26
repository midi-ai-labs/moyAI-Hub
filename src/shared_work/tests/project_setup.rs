use super::*;

#[test]
fn setup_code_management_returns_secret_once_and_preserves_authority_and_revision() {
    let mut f = Fixture::new();
    let command = ManagementCommand {
        request_id: "initial-person".into(),
        expected_revision: f.work.management_snapshot(&f.admin).unwrap()["revision"]
            .as_str()
            .unwrap()
            .into(),
        request: ManagementRequest::CreateUserWithSetupCode {
            username: "new-person".into(),
            display_name: "New Person".into(),
        },
    };
    assert!(f.work.manage(&f.alice, &command, 100).is_err());
    let first = f.work.manage(&f.admin, &command, 100).unwrap();
    let user_id = first["issued_setup_code"]["user_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let code = first["issued_setup_code"]["code"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(code.len(), 64);
    assert!(
        first["users"]
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["user_id"] == user_id && u["setup_pending"] == true)
    );
    assert!(
        f.work
            .management_snapshot(&f.admin)
            .unwrap()
            .get("issued_setup_code")
            .is_none()
    );
    assert!(
        f.work
            .manage(&f.admin, &command, 101)
            .unwrap()
            .get("issued_setup_code")
            .is_none()
    );
    let mut reissue = ManagementCommand {
        request_id: "reissue-person".into(),
        expected_revision: command.expected_revision.clone(),
        request: ManagementRequest::ReissueSetupCode {
            user_id: user_id.clone(),
        },
    };
    assert!(f.work.manage(&f.admin, &reissue, 102).is_err());
    reissue.expected_revision = first["revision"].as_str().unwrap().into();
    assert!(f.work.manage(&f.alice, &reissue, 103).is_err());
    let second = f.work.manage(&f.admin, &reissue, 104).unwrap();
    let replacement = second["issued_setup_code"]["code"].as_str().unwrap();
    assert_ne!(code, replacement);
    assert!(
        identity::complete_password_setup(
            &f.work.db,
            "new-person",
            &code,
            "a sufficiently long password",
            "WinA",
            105
        )
        .is_err()
    );
    assert!(
        identity::complete_password_setup(
            &f.work.db,
            "new-person",
            replacement,
            "a sufficiently long password",
            "WinA",
            105
        )
        .is_ok()
    );
    f.reopen();
    assert!(
        f.work
            .manage(&f.admin, &command, 106)
            .unwrap()
            .get("issued_setup_code")
            .is_none()
    );
    assert!(
        f.work
            .manage(&f.admin, &reissue, 106)
            .unwrap()
            .get("issued_setup_code")
            .is_none()
    );
}

#[test]
fn onboarding_reason_distinguishes_person_from_device_without_disclosing_project_data() {
    let mut f = Fixture::new();
    save(&mut f, "onboarding", &["WinA"], &[]);
    let outsider = identity::create_user(
        &f.work.db,
        &f.admin,
        "outsider",
        "Outsider",
        "a sufficiently long password",
        false,
        45,
    )
    .unwrap();
    assert_eq!(
        super::super::project_setup::project_access(&f.work.db, &outsider.user_id, "WinA").unwrap(),
        "no_membership"
    );
    identity::set_project_role(
        &f.work.db,
        &f.admin,
        "onboarding",
        &outsider.user_id,
        Some(identity::ProjectRole::Reader),
        46,
    )
    .unwrap();
    assert_eq!(
        super::super::project_setup::project_access(&f.work.db, &outsider.user_id, "WinA").unwrap(),
        "ready"
    );
    assert_eq!(
        super::super::project_setup::project_access(&f.work.db, &outsider.user_id, "WinB").unwrap(),
        "device_not_allowed"
    );
}

#[test]
fn joining_pc_adds_only_reviewed_purposes_without_granting_people_or_replacing_other_pcs() {
    let mut f = Fixture::new();
    save(&mut f, "onboarding", &["WinA"], &[]);
    let before = f.work.management_snapshot(&f.admin).unwrap();
    let command = ManagementCommand {
        request_id: "join-pc".into(),
        expected_revision: before["revision"].as_str().unwrap().into(),
        request: ManagementRequest::AssignProjectDevice {
            project_id: "onboarding".into(),
            device_id: "WinB".into(),
            can_control: false,
            can_execute: true,
        },
    };
    assert!(f.work.manage(&f.alice, &command, 50).is_err());
    let after = f.work.manage(&f.admin, &command, 50).unwrap();
    assert_eq!(before["project_memberships"], after["project_memberships"]);
    assert!(
        f.work
            .require_project_controller("onboarding", "WinA")
            .is_ok()
    );
    assert!(
        f.work
            .require_project_controller("onboarding", "WinB")
            .is_err()
    );
    assert_eq!(
        f.work.device_projects("WinB").unwrap()["projects"][0]["preparation_state"],
        "waiting_local_folder"
    );
    f.work.manage(&f.admin, &command, 51).unwrap();
    let mut changed = command.clone();
    changed.request = ManagementRequest::AssignProjectDevice {
        project_id: "onboarding".into(),
        device_id: "WinC".into(),
        can_control: true,
        can_execute: true,
    };
    assert!(f.work.manage(&f.admin, &changed, 52).is_err());
    f.reopen();
    assert!(
        f.work
            .require_project_controller("onboarding", "WinA")
            .is_ok()
    );
}

fn save(
    f: &mut Fixture,
    project: &str,
    controllers: &[&str],
    runners: &[&str],
) -> ManagementCommand {
    let command = ManagementCommand {
        request_id: ulid::Ulid::new().to_string(),
        expected_revision: f.work.management_snapshot(&f.admin).unwrap()["revision"]
            .as_str()
            .unwrap()
            .into(),
        request: ManagementRequest::SaveProject {
            id: project.into(),
            label: "解析業務".into(),
            archived: false,
            members: Some(vec![ProjectMember {
                user_id: f.alice.user_id.clone(),
                role: ProjectRole::Contributor,
            }]),
            controller_device_ids: controllers.iter().map(|v| v.to_string()).collect(),
            runner_device_ids: runners.iter().map(|v| v.to_string()).collect(),
        },
    };
    f.work.manage(&f.admin, &command, 40).unwrap();
    command
}

#[test]
fn leaving_and_rejoining_changes_participation_and_execution_workspace() {
    let mut f = Fixture::new();
    save(&mut f, "new-a", &["WinA"], &["WinB"]);
    let first = f.work.pending_provision("WinB").unwrap().remove(0);
    f.work
        .provision_receipt(
            "WinB",
            &ProvisionReceipt {
                environment_id: first.environment_id.clone(),
                template_id: first.template_id.clone(),
                generation: first.generation,
                success: true,
                error: None,
            },
            41,
        )
        .unwrap();
    let submit = SubmitJob {
        request_id: "before-leave".into(),
        project_id: "new-a".into(),
        project_participation: Some(1),
        conversation_id: None,
        revises_job_id: None,
        expected_revised_revision: None,
        environment_id: String::new(),
        origin_session_ref: None,
        origin_turn_ref: None,
        origin_turn_epoch: None,
        title: "before leave".into(),
        input: json!({"prompt":"work"}),
        descendant_budget: 0,
        start_before_ms: None,
    };
    let initial = f
        .work
        .submit_for_device(&f.alice, &submit, 42, Some("WinA"))
        .unwrap();
    assert_eq!(initial.environment_id, first.environment_id);
    f.work.leave_project("new-a", "WinA", 1, 43).unwrap();
    assert_eq!(
        f.work.job(&f.alice, &initial.id).unwrap().state,
        JobState::Cancelled
    );
    assert!(matches!(
        f.work
            .submit_for_device(&f.alice, &submit, 44, Some("WinA")),
        Err(WorkError::Forbidden)
    ));
    save(&mut f, "new-a", &["WinA"], &["WinB"]);
    let win_a = f.work.device_projects("WinA").unwrap();
    assert_eq!(win_a["projects"][0]["participation_generation"], 2);
    assert!(matches!(
        f.work.submit_for_device(
            &f.alice,
            &SubmitJob {
                request_id: "stale-participation".into(),
                ..submit.clone()
            },
            45,
            Some("WinA")
        ),
        Err(WorkError::Conflict)
    ));
    assert!(matches!(
        f.work.leave_project("new-a", "WinA", 1, 46),
        Err(WorkError::Conflict)
    ));
    let current = f
        .work
        .submit_for_device(
            &f.alice,
            &SubmitJob {
                request_id: "after-rejoin".into(),
                project_participation: Some(2),
                ..submit
            },
            47,
            Some("WinA"),
        )
        .unwrap();
    assert_eq!(current.environment_id, first.environment_id);

    f.work.leave_project("new-a", "WinB", 1, 48).unwrap();
    assert_eq!(
        f.work.job(&f.alice, &current.id).unwrap().state,
        JobState::Cancelled
    );
    save(&mut f, "new-a", &["WinA"], &["WinB"]);
    let new = f.work.pending_provision("WinB").unwrap().remove(0);
    assert_ne!(new.environment_id, first.environment_id);
    assert_eq!(
        f.work.device_projects("WinB").unwrap()["projects"][0]["participation_generation"],
        2
    );
    f.work
        .provision_receipt(
            "WinB",
            &ProvisionReceipt {
                environment_id: first.environment_id,
                template_id: first.template_id,
                generation: first.generation,
                success: true,
                error: None,
            },
            49,
        )
        .unwrap();
    let retired = f
        .work
        .runner_environments("WinB")
        .unwrap()
        .into_iter()
        .find(|row| row.environment.id == initial.environment_id)
        .unwrap();
    assert!(!retired.enabled);
    assert!(retired.environment.project_ids.is_empty());
}

#[test]
fn missing_local_folder_disables_execution_until_explicit_rebind() {
    missing_folder_requires_current_rebind("local-folder");
}

#[test]
fn missing_legacy_desktop_folder_disables_execution_until_explicit_rebind() {
    missing_folder_requires_current_rebind("desktop-default");
}

fn missing_folder_requires_current_rebind(template: &str) {
    let mut f = Fixture::new();
    save(&mut f, "new-a", &["WinA"], &["WinB"]);
    let mut initial = f.work.pending_provision("WinB").unwrap().remove(0);
    if template == "desktop-default" {
        // Saved projects from the former automatic-folder workflow keep their receipt type.
        f.work
            .db
            .execute(
                "UPDATE shared_provisioning SET template_id=?2 WHERE environment_id=?1",
                rusqlite::params![initial.environment_id, template],
            )
            .unwrap();
        initial.template_id = template.into();
    }
    let ready = ProvisionReceipt {
        environment_id: initial.environment_id.clone(),
        template_id: initial.template_id.clone(),
        generation: initial.generation,
        success: true,
        error: None,
    };
    f.work.provision_receipt("WinB", &ready, 41).unwrap();
    f.work
        .provision_receipt(
            "WinB",
            &ProvisionReceipt {
                success: false,
                error: Some("folder missing".into()),
                ..ready.clone()
            },
            42,
        )
        .unwrap();
    let pending = f.work.pending_provision("WinB").unwrap().remove(0);
    assert_eq!(pending.state.as_deref(), Some("failed"));
    assert_eq!(pending.template_id, template);
    assert_eq!(pending.generation, initial.generation + 1);
    assert!(!f.work.runner_environments("WinB").unwrap()[0].enabled);
    f.reopen();
    // Repeating an unacknowledged failure must neither bump again nor restore authority.
    f.work
        .provision_receipt(
            "WinB",
            &ProvisionReceipt {
                success: false,
                error: Some("folder missing".into()),
                ..ready.clone()
            },
            43,
        )
        .unwrap();
    assert_eq!(
        f.work.pending_provision("WinB").unwrap()[0].generation,
        pending.generation
    );
    assert!(f.work.provision_receipt("WinA", &ready, 43).is_err());
    assert!(matches!(
        f.work.provision_receipt("WinB", &ready, 43),
        Err(WorkError::Conflict)
    ));
    let rebound = ProvisionReceipt {
        generation: pending.generation,
        ..ready
    };
    f.work.provision_receipt("WinB", &rebound, 44).unwrap();
    assert!(f.work.runner_environments("WinB").unwrap()[0].enabled);
    f.work
        .db
        .execute(
            "UPDATE shared_environments SET enabled=0 WHERE id=?1",
            [&initial.environment_id],
        )
        .unwrap();
    f.work.provision_receipt("WinB", &rebound, 45).unwrap();
    assert!(!f.work.runner_environments("WinB").unwrap()[0].enabled);
    save(&mut f, "new-a", &["WinA"], &["WinB"]);
    assert!(!f.work.runner_environments("WinB").unwrap()[0].enabled);
    save(&mut f, "new-a", &["WinA"], &[]);
    assert!(
        f.work
            .provision_receipt(
                "WinB",
                &ProvisionReceipt {
                    success: false,
                    error: Some("missing after removal".into()),
                    ..rebound
                },
                46
            )
            .is_err()
    );
}

#[test]
fn deleting_used_project_revokes_roles_and_stops_work_without_dropping_audit() {
    let mut f = Fixture::new();
    save(&mut f, "new-a", &["WinA"], &["WinB"]);
    let request = f.work.pending_provision("WinB").unwrap().remove(0);
    f.work
        .provision_receipt(
            "WinB",
            &ProvisionReceipt {
                environment_id: request.environment_id.clone(),
                template_id: request.template_id,
                generation: request.generation,
                success: true,
                error: None,
            },
            41,
        )
        .unwrap();
    let job = f
        .work
        .submit_for_device(
            &f.alice,
            &SubmitJob {
                request_id: "used-project".into(),
                project_id: "new-a".into(),
                project_participation: Some(1),
                conversation_id: None,
                revises_job_id: None,
                expected_revised_revision: None,
                environment_id: String::new(),
                origin_session_ref: None,
                origin_turn_ref: None,
                origin_turn_epoch: None,
                title: "ongoing".into(),
                input: json!({"prompt":"work"}),
                descendant_budget: 0,
                start_before_ms: None,
            },
            42,
            Some("WinA"),
        )
        .unwrap();
    let attempt = f.work.claim("WinB", 43).unwrap().unwrap();
    let revision = f.work.management_snapshot(&f.admin).unwrap()["revision"]
        .as_str()
        .unwrap()
        .to_owned();
    f.work
        .manage(
            &f.admin,
            &ManagementCommand {
                request_id: "delete-used-project".into(),
                expected_revision: revision,
                request: ManagementRequest::DeleteProject { id: "new-a".into() },
            },
            44,
        )
        .unwrap();
    assert_eq!(
        f.work
            .attempt("WinB", &attempt.attempt_id)
            .unwrap()
            .assignment
            .stop_requested,
        true
    );
    assert!(
        f.work.device_projects("WinA").unwrap()["projects"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["id"] != "new-a")
    );
    assert!(
        f.work.management_snapshot(&f.admin).unwrap()["projects"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["id"] != "new-a")
    );
    f.report(
        &attempt,
        "stopped-after-project-delete",
        ReportOutcome::Finished {
            success: false,
            result: json!({"stopped":true}),
            resources_released: true,
        },
    );
    let stored: String = f
        .work
        .db
        .query_row(
            "SELECT state FROM shared_jobs WHERE id=?1",
            [&job.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, "cancelled");
    assert!(f.work.job(&f.alice, &job.id).is_err());
    assert!(
        f.work
            .create_project(&f.admin, "new-a", "recreated")
            .is_err()
    );
}

#[test]
fn project_setup_waits_for_pc_and_reuses_one_physical_resource_across_projects() {
    let mut f = Fixture::new();
    let command = save(&mut f, "new-a", &["WinA", "WinZ"], &["WinZ"]);
    let pending = f.work.device_projects("WinZ").unwrap();
    let row = pending["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "new-a")
        .unwrap();
    assert_eq!(row["preparation_state"], "waiting_local_folder");
    assert!(row["environment_id"].is_string());
    f.work
        .publish_templates(
            "WinZ",
            &[RunnerTemplate {
                id: "desktop-default".into(),
                label: "PCの実行設定".into(),
            }],
            50,
        )
        .unwrap();
    let request = f.work.pending_provision("WinZ").unwrap().remove(0);
    assert_eq!(request.template_id, "local-folder");
    assert_eq!(request.state.as_deref(), Some("pending"));
    assert_eq!(
        f.work
            .status_for_device(&f.alice, "new-a", None, 10, None, Some("WinA"))
            .unwrap()
            .execution_devices[0]
            .preparation_state,
        "waiting_local_folder"
    );
    assert!(!f.work.runner_environments("WinZ").unwrap()[0].enabled);
    f.work
        .provision_receipt(
            "WinZ",
            &ProvisionReceipt {
                environment_id: request.environment_id.clone(),
                template_id: request.template_id.clone(),
                generation: request.generation,
                success: true,
                error: None,
            },
            60,
        )
        .unwrap();
    assert!(f.work.runner_environments("WinZ").unwrap()[0].enabled);
    assert_eq!(
        f.work
            .status_for_device(&f.alice, "new-a", None, 10, None, Some("WinA"))
            .unwrap()
            .execution_devices[0]
            .preparation_state,
        "ready"
    );
    f.work.manage(&f.admin, &command, 61).unwrap();
    assert_eq!(
        f.work.runner_environments("WinZ").unwrap().len(),
        1,
        "exact retry cannot create another environment"
    );
    save(&mut f, "new-b", &["WinB"], &["WinZ"]);
    let environments = f.work.runner_environments("WinZ").unwrap();
    assert_eq!(environments.len(), 2);
    assert_eq!(
        environments[0].environment.resource_id,
        environments[1].environment.resource_id
    );
    assert_eq!(environments[0].environment.capacity, 1);
    assert!(f.work.require_project_controller("new-a", "WinA").is_ok());
    assert!(f.work.require_project_controller("new-a", "WinZ").is_ok());
    assert!(f.work.require_project_controller("new-a", "WinB").is_err());
    assert!(
        f.work.require_project_controller("new-b", "WinZ").is_err(),
        "executor is not implicitly a controller"
    );
    assert!(
        f.work
            .require_project_controller("alpha", "any-registered-pc")
            .is_ok(),
        "legacy project authorization is preserved until saved"
    );
    f.reopen();
    assert_eq!(f.work.runner_environments("WinZ").unwrap().len(), 2);
    assert!(f.work.require_project_controller("new-a", "WinB").is_err());
}

#[test]
fn project_status_shows_assigned_executor_before_initial_setup() {
    let mut f = Fixture::new();
    save(&mut f, "new-a", &["WinA"], &["WinZ"]);
    let status = f
        .work
        .status_for_device(&f.alice, "new-a", None, 10, None, Some("WinA"))
        .unwrap();
    assert_eq!(status.environments.len(), 1);
    let json = serde_json::to_value(status).unwrap();
    assert_eq!(json["execution_devices"][0]["device_id"], "WinZ");
    assert!(json["execution_devices"][0]["environment_id"].is_string());
    assert_eq!(
        json["execution_devices"][0]["preparation_state"],
        "waiting_local_folder"
    );
    assert!(
        f.work
            .status_for_device(&f.bob, "new-a", None, 10, None, Some("WinA"))
            .is_err()
    );
    assert!(
        f.work
            .status_for_device(&f.alice, "new-a", None, 10, None, Some("WinB"))
            .is_err()
    );
}

#[test]
fn project_setup_revoke_keeps_history_binding_and_stale_ready_does_not_reenable_it() {
    let mut f = Fixture::new();
    f.work
        .publish_templates(
            "WinZ",
            &[RunnerTemplate {
                id: "desktop-default".into(),
                label: "PCの実行設定".into(),
            }],
            30,
        )
        .unwrap();
    save(&mut f, "new-a", &["WinA"], &["WinZ"]);
    let request = f.work.pending_provision("WinZ").unwrap().remove(0);
    save(&mut f, "new-a", &["WinB"], &[]);
    assert!(matches!(
        f.work.provision_receipt(
            "WinZ",
            &ProvisionReceipt {
                environment_id: request.environment_id,
                template_id: request.template_id,
                generation: request.generation,
                success: true,
                error: None,
            },
            60,
        ),
        Err(WorkError::Forbidden)
    ));
    let environments = f.work.runner_environments("WinZ").unwrap();
    assert_eq!(environments.len(), 1);
    assert!(!environments[0].enabled);
    assert!(environments[0].environment.project_ids.is_empty());
    assert!(f.work.require_project_controller("new-a", "WinA").is_err());
    assert!(f.work.require_project_controller("new-a", "WinB").is_ok());
}

#[test]
fn project_setup_invalid_people_roll_back_the_entire_project_edit() {
    let mut f = Fixture::new();
    let command = ManagementCommand {
        request_id: "invalid-project".into(),
        expected_revision: f.work.management_snapshot(&f.admin).unwrap()["revision"]
            .as_str()
            .unwrap()
            .into(),
        request: ManagementRequest::SaveProject {
            id: "invalid-new".into(),
            label: "新規".into(),
            archived: false,
            members: Some(vec![ProjectMember {
                user_id: "missing-user".into(),
                role: ProjectRole::Contributor,
            }]),
            controller_device_ids: vec!["WinA".into()],
            runner_device_ids: vec!["WinZ".into()],
        },
    };
    assert!(f.work.manage(&f.admin, &command, 40).is_err());
    assert!(
        !f.work.management_snapshot(&f.admin).unwrap()["projects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == "invalid-new")
    );
}

#[test]
fn project_setup_pc_limit_is_visible_without_breaking_existing_runner_catalog() {
    let mut f = Fixture::new();
    for n in 0..128 {
        f.work
            .register_environment(
                &f.admin,
                &Environment {
                    id: format!("old-{n}"),
                    label: format!("Old {n}"),
                    resource_id: "one-pc-resource".into(),
                    runner_id: "WinZ".into(),
                    capacity: 1,
                    project_ids: vec!["alpha".into()],
                },
            )
            .unwrap();
    }
    save(&mut f, "new-a", &["WinA"], &["WinZ"]);
    f.work
        .publish_templates(
            "WinZ",
            &[RunnerTemplate {
                id: "desktop-default".into(),
                label: "PCの実行設定".into(),
            }],
            50,
        )
        .unwrap();
    assert_eq!(
        f.work.runner_environments("WinZ").unwrap().len(),
        128,
        "publishing settings must not make the existing Runner catalog unreadable"
    );
    let projection = f.work.device_projects("WinZ").unwrap();
    let row = projection["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "new-a")
        .unwrap();
    assert_eq!(row["preparation_state"], "failed");
    assert!(row["error"].as_str().unwrap().contains("上限"));
    assert_eq!(row["environment_id"], Value::Null);
    assert!(f.work.pending_provision("WinZ").unwrap().is_empty());
}

#[test]
fn project_setup_pc_edit_keeps_unchanged_people_authorization_generation() {
    let mut f = Fixture::new();
    save(&mut f, "new-a", &["WinA"], &[]);
    let job = f.submit("solver", "approval-in-another-project", 0);
    let attempt = f.work.claim("WinE", 11).unwrap().unwrap();
    f.report(&attempt, "started", ReportOutcome::Started);
    request_approval(&mut f, &attempt, "existing-approval");
    f.work
        .decide_approval(
            &f.alice,
            &job.id,
            "existing-approval",
            ApprovalDecision::Approve,
            22,
        )
        .unwrap();
    let generation = || {
        f.work
            .db
            .query_row(
                "SELECT authorization_generation FROM identity_users WHERE id=?1",
                [&f.alice.user_id],
                |r| r.get::<_, u64>(0),
            )
            .unwrap()
    };
    let before = generation();
    save(&mut f, "new-a", &["WinB"], &[]);
    let after = f
        .work
        .db
        .query_row(
            "SELECT authorization_generation FROM identity_users WHERE id=?1",
            [&f.alice.user_id],
            |r| r.get::<_, u64>(0),
        )
        .unwrap();
    assert_eq!(
        after, before,
        "changing only PC purposes must not invalidate this person's approvals in unrelated projects"
    );
    let consumed = f
        .work
        .consume_approval(
            "WinE",
            "existing-approval",
            &attempt.attempt_id,
            attempt.generation,
            50,
        )
        .unwrap()
        .unwrap();
    let ApprovalConsumeResult::Answer(answer) = consumed else {
        panic!("unchanged project membership must retain the existing answer");
    };
    assert_eq!(answer.decision, ApprovalDecision::Approve);
}

#[test]
fn project_setup_archive_preserves_selected_pc_history_and_cancellation_but_blocks_new_work() {
    let mut f = Fixture::new();
    save(&mut f, "alpha", &["WinA"], &["WinE"]);
    let job = f.submit("solver", "keep-history", 0);
    let command = ManagementCommand {
        request_id: "archive".into(),
        expected_revision: f.work.management_snapshot(&f.admin).unwrap()["revision"]
            .as_str()
            .unwrap()
            .into(),
        request: ManagementRequest::SaveProject {
            id: "alpha".into(),
            label: "保管した仕事".into(),
            archived: true,
            members: Some(vec![ProjectMember {
                user_id: f.alice.user_id.clone(),
                role: ProjectRole::Contributor,
            }]),
            controller_device_ids: vec!["WinA".into()],
            runner_device_ids: vec!["WinE".into()],
        },
    };
    f.work.manage(&f.admin, &command, 40).unwrap();
    assert!(f.work.require_project_controller("alpha", "WinA").is_ok());
    assert!(
        f.work
            .controlled_projects("WinA")
            .unwrap()
            .iter()
            .any(|p| p == "alpha")
    );
    assert!(f.work.job(&f.alice, &job.id).is_ok());
    assert!(f.work.cancel(&f.alice, &job.id, 41).is_ok());
    assert!(
        f.work
            .submit(
                &f.alice,
                &SubmitJob {
                    request_id: "after-archive".into(),
                    project_id: "alpha".into(),
                    project_participation: None,
                    conversation_id: None,
                    revises_job_id: None,
                    expected_revised_revision: None,
                    environment_id: "solver".into(),
                    origin_session_ref: None,
                    origin_turn_ref: None,
                    origin_turn_epoch: None,
                    title: "new".into(),
                    input: json!({}),
                    descendant_budget: 0,
                    start_before_ms: None
                },
                42
            )
            .is_err()
    );
}

#[test]
fn project_setup_adopted_environment_cannot_be_retargeted_by_the_legacy_editor() {
    let mut f = Fixture::new();
    save(&mut f, "alpha", &["WinA"], &["WinE"]);
    let command = ManagementCommand {
        request_id: "retarget".into(),
        expected_revision: f.work.management_snapshot(&f.admin).unwrap()["revision"]
            .as_str()
            .unwrap()
            .into(),
        request: ManagementRequest::UpdateEnvironment {
            id: "solver".into(),
            label: "Repurposed".into(),
            resource_id: "wine-slot".into(),
            runner_id: "WinB".into(),
            enabled: true,
            project_ids: vec!["alpha".into(), "beta".into()],
        },
    };
    assert!(matches!(
        f.work.manage(&f.admin, &command, 41),
        Err(WorkError::Conflict)
    ));
    assert_eq!(
        f.work.runner_environments("WinE").unwrap()[0]
            .environment
            .id,
        "solver"
    );
    assert!(f.work.runner_environments("WinB").unwrap().is_empty());
}

#[test]
fn project_setup_legacy_environment_edits_cannot_override_managed_pc_purposes() {
    let mut f = Fixture::new();
    save(&mut f, "alpha", &["WinA"], &["WinE"]);
    for request in [
        ManagementRequest::CreateEnvironment {
            id: "extra".into(),
            label: "Extra".into(),
            resource_id: "wine-slot".into(),
            runner_id: "WinB".into(),
            capacity: 1,
            project_ids: vec!["alpha".into()],
            template_id: None,
        },
        ManagementRequest::UpdateEnvironment {
            id: "parent".into(),
            label: "Parent".into(),
            resource_id: "parent-slot".into(),
            runner_id: "WinC".into(),
            enabled: true,
            project_ids: vec!["alpha".into(), "beta".into()],
        },
        ManagementRequest::UpdateEnvironment {
            id: "solver".into(),
            label: "Solver".into(),
            resource_id: "wine-slot".into(),
            runner_id: "WinE".into(),
            enabled: true,
            project_ids: vec!["beta".into()],
        },
    ] {
        let command = ManagementCommand {
            request_id: ulid::Ulid::new().to_string(),
            expected_revision: f.work.management_snapshot(&f.admin).unwrap()["revision"]
                .as_str()
                .unwrap()
                .into(),
            request,
        };
        assert!(matches!(
            f.work.manage(&f.admin, &command, 41),
            Err(WorkError::Conflict)
        ));
    }
    assert!(matches!(
        f.work.register_environment(
            &f.admin,
            &Environment {
                id: "direct-extra".into(),
                label: "Direct".into(),
                resource_id: "wine-slot".into(),
                runner_id: "WinB".into(),
                capacity: 1,
                project_ids: vec!["alpha".into()]
            }
        ),
        Err(WorkError::Conflict)
    ));
    assert_eq!(
        f.work.runner_environments("WinE").unwrap()[0]
            .environment
            .project_ids,
        vec!["alpha", "beta"]
    );
    assert!(f.work.runner_environments("WinB").unwrap().is_empty());
}

#[test]
fn project_setup_uses_a_fresh_folder_binding_without_adopting_ambiguous_legacy_environments() {
    let mut f = Fixture::new();
    f.work
        .register_environment(
            &f.admin,
            &Environment {
                id: "extra".into(),
                label: "Extra".into(),
                resource_id: "wine-slot".into(),
                runner_id: "WinE".into(),
                capacity: 1,
                project_ids: vec!["alpha".into()],
            },
        )
        .unwrap();
    save(&mut f, "alpha", &["WinA"], &["WinE"]);
    let row = |f: &Fixture| {
        f.work.device_projects("WinE").unwrap()["projects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == "alpha")
            .unwrap()
            .clone()
    };
    let new_environment = row(&f)["environment_id"].as_str().unwrap().to_owned();
    assert_ne!(new_environment, "solver");
    assert_ne!(new_environment, "extra");
    assert_eq!(row(&f)["preparation_state"], "waiting_local_folder");
    let command = ManagementCommand {
        request_id: "remove-ambiguity".into(),
        expected_revision: f.work.management_snapshot(&f.admin).unwrap()["revision"]
            .as_str()
            .unwrap()
            .into(),
        request: ManagementRequest::DeleteEnvironment { id: "extra".into() },
    };
    f.work.manage(&f.admin, &command, 41).unwrap();
    save(&mut f, "alpha", &["WinA"], &["WinE"]);
    assert_eq!(row(&f)["environment_id"], new_environment);
    assert_eq!(row(&f)["preparation_state"], "waiting_local_folder");
    assert_eq!(row(&f)["error"], Value::Null);
}

#[test]
fn project_setup_v11_forward_preserves_work_people_and_registered_pc_compatibility() {
    super::completion::released_migration(11);
}

#[test]
fn project_setup_converts_only_the_saved_projects_effective_people() {
    let mut f = Fixture::new();
    f.work
        .db
        .execute(
            "INSERT INTO identity_groups(id,label) VALUES('team','Team')",
            [],
        )
        .unwrap();
    f.work
        .db
        .execute(
            "INSERT INTO identity_group_memberships VALUES('team',?1)",
            [&f.bob.user_id],
        )
        .unwrap();
    f.work.db.execute("INSERT INTO identity_group_project_roles VALUES('team','alpha','manager'),('team','beta','manager')",[]).unwrap();
    let command = ManagementCommand {
        request_id: "explicit-members".into(),
        expected_revision: f.work.management_snapshot(&f.admin).unwrap()["revision"]
            .as_str()
            .unwrap()
            .into(),
        request: ManagementRequest::SaveProject {
            id: "alpha".into(),
            label: "Alpha".into(),
            archived: false,
            members: Some(vec![
                ProjectMember {
                    user_id: f.alice.user_id.clone(),
                    role: ProjectRole::Contributor,
                },
                ProjectMember {
                    user_id: f.bob.user_id.clone(),
                    role: ProjectRole::Manager,
                },
            ]),
            controller_device_ids: vec!["WinA".into()],
            runner_device_ids: vec!["WinE".into(), "WinC".into()],
        },
    };
    f.work.manage(&f.admin, &command, 40).unwrap();
    assert!(
        identity::require_project_role(&f.work.db, &f.bob.user_id, "alpha", ProjectRole::Manager)
            .is_ok()
    );
    assert!(
        identity::require_project_role(&f.work.db, &f.bob.user_id, "beta", ProjectRole::Manager)
            .is_ok()
    );
    assert_eq!(
        f.work
            .db
            .query_row(
                "SELECT COUNT(*) FROM identity_group_project_roles WHERE project_id='alpha'",
                [],
                |r| r.get::<_, u32>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        f.work
            .db
            .query_row(
                "SELECT COUNT(*) FROM identity_group_project_roles WHERE project_id='beta'",
                [],
                |r| r.get::<_, u32>(0)
            )
            .unwrap(),
        1
    );
    assert_eq!(f.work.runner_environments("WinE").unwrap().len(), 1);
    assert!(f.work.require_project_controller("alpha", "WinB").is_err());
}
