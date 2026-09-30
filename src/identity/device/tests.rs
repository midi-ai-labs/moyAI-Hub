use super::*;
use crate::shared_work::{ManagementCommand, ManagementRequest, SharedWork};
use std::path::Path;

fn fixture() -> (SharedWork, Principal) {
    let work = SharedWork::open(Path::new(":memory:")).unwrap();
    let admin = bootstrap_admin(&work.db, "admin", "Admin", "existing-password-1234", 1).unwrap();
    (work, admin)
}

fn project(work: &mut SharedWork, admin: &Principal, controllers: &[&str]) {
    let command = ManagementCommand {
        request_id: ulid::Ulid::new().to_string(),
        expected_revision: work.management_snapshot(admin).unwrap()["revision"]
            .as_str()
            .unwrap()
            .into(),
        request: ManagementRequest::SaveProject {
            overview: None,
            id: "project".into(),
            label: "Project".into(),
            archived: false,
            members: None,
            controller_device_ids: controllers.iter().map(|s| (*s).into()).collect(),
            runner_device_ids: vec![],
        },
    };
    work.manage(admin, &command, 2).unwrap();
}

#[test]
fn equal_windows_names_remain_separate_actors_and_replayed_approval_is_stable() {
    let (work, admin) = fixture();
    let a = enroll(&work.db, "WinA-key", "DOMAIN\\same-user", 2).unwrap();
    let b = enroll(&work.db, "WinB-key", "DOMAIN\\same-user", 2).unwrap();
    assert_ne!(a.user_id, b.user_id);
    assert!(!a.administrator && !b.administrator);
    assert_eq!(
        enroll(&work.db, "WinA-key", "Renamed PC", 3)
            .unwrap()
            .user_id,
        a.user_id
    );
    bind(&work.db, &admin, "WinB-key", Some(&a.user_id), "WinB", 4).unwrap();
    assert_eq!(
        enroll(&work.db, "WinB-key", "WinB", 5).unwrap().user_id,
        a.user_id
    );
    assert_eq!(list(&work.db).unwrap().len(), 2);
    bind(&work.db, &admin, "WinB-key", None, "WinB", 6).unwrap();
    assert_eq!(
        issue_session(&work.db, "WinB-key", 7)
            .unwrap()
            .principal
            .user_id,
        b.user_id
    );
}

#[test]
fn session_requires_reviewed_binding_and_is_bound_to_device_audience_and_current_association() {
    let (work, admin) = fixture();
    assert!(matches!(
        issue_session(&work.db, "WinA", 2),
        Err(IdentityError::DeviceUnbound)
    ));
    let a = enroll(&work.db, "WinA", "WinA", 2).unwrap();
    enroll(&work.db, "WinB", "WinB", 2).unwrap();
    let session = issue_session(&work.db, "WinA", 3).unwrap();
    let other = issue_session(&work.db, "WinB", 3).unwrap();
    assert_eq!(
        authenticate(
            &work.db,
            &session.token,
            SessionAudience::SharedWork,
            Some("WinA"),
            4
        )
        .unwrap(),
        a
    );
    assert!(
        authenticate(
            &work.db,
            &session.token,
            SessionAudience::SharedWork,
            Some("WinB"),
            4
        )
        .is_err()
    );
    assert!(authenticate(&work.db, &session.token, SessionAudience::WebAdmin, None, 4).is_err());
    bind(&work.db, &admin, "WinA", Some(&admin.user_id), "WinA", 5).unwrap();
    assert!(
        authenticate(
            &work.db,
            &session.token,
            SessionAudience::SharedWork,
            Some("WinA"),
            6
        )
        .is_err()
    );
    assert!(
        authenticate(
            &work.db,
            &other.token,
            SessionAudience::SharedWork,
            Some("WinB"),
            6
        )
        .is_ok()
    );
    assert_eq!(
        issue_session(&work.db, "WinA", 7)
            .unwrap()
            .principal
            .user_id,
        admin.user_id
    );
    assert!(active_principal(&work.db, &a.user_id).is_ok());
}

#[test]
fn controller_selection_is_canonical_and_existing_reader_role_is_not_broadened() {
    let (mut work, admin) = fixture();
    let a = enroll(&work.db, "WinA", "WinA", 2).unwrap();
    let b = enroll(&work.db, "WinB", "WinB", 2).unwrap();
    project(&mut work, &admin, &["WinA"]);
    let before = work
        .db
        .query_row(
            "SELECT authorization_generation FROM identity_users WHERE id=?1",
            [&a.user_id],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    project(&mut work, &admin, &["WinA"]);
    assert_eq!(
        work.db
            .query_row(
                "SELECT authorization_generation FROM identity_users WHERE id=?1",
                [&a.user_id],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        before
    );
    assert_eq!(
        require_project_role(&work.db, &a.user_id, "project", ProjectRole::Reader).unwrap(),
        ProjectRole::Contributor
    );
    assert!(require_project_role(&work.db, &b.user_id, "project", ProjectRole::Reader).is_err());
    assert_eq!(
        work.db
            .query_row(
                "SELECT COUNT(*) FROM identity_project_memberships",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    set_project_role(
        &work.db,
        &admin,
        "project",
        &a.user_id,
        Some(ProjectRole::Reader),
        3,
    )
    .unwrap();
    project(&mut work, &admin, &["WinA", "WinB"]);
    assert_eq!(
        require_project_role(&work.db, &a.user_id, "project", ProjectRole::Reader).unwrap(),
        ProjectRole::Reader
    );
    assert_eq!(
        require_project_role(&work.db, &b.user_id, "project", ProjectRole::Reader).unwrap(),
        ProjectRole::Contributor
    );
    project(&mut work, &admin, &["WinA"]);
    assert!(require_project_role(&work.db, &b.user_id, "project", ProjectRole::Reader).is_err());
}

#[test]
fn device_only_project_edit_preserves_group_grants_and_legacy_projects_gain_nothing() {
    let (mut work, admin) = fixture();
    let a = enroll(&work.db, "WinA", "WinA", 2).unwrap();
    work.db
        .execute(
            "INSERT INTO shared_projects(id,label) VALUES('legacy','Legacy')",
            [],
        )
        .unwrap();
    assert!(require_project_role(&work.db, &a.user_id, "legacy", ProjectRole::Reader).is_err());
    project(&mut work, &admin, &["WinA"]);
    work.db
        .execute(
            "INSERT INTO identity_groups(id,label) VALUES('team','Team')",
            [],
        )
        .unwrap();
    work.db
        .execute(
            "INSERT INTO identity_group_memberships(group_id,user_id) VALUES('team',?1)",
            [&a.user_id],
        )
        .unwrap();
    work.db.execute("INSERT INTO identity_group_project_roles(group_id,project_id,role) VALUES('team','project','manager')",[]).unwrap();
    project(&mut work, &admin, &["WinA"]);
    assert_eq!(
        require_project_role(&work.db, &a.user_id, "project", ProjectRole::Reader).unwrap(),
        ProjectRole::Manager
    );
    assert_eq!(
        work.db
            .query_row(
                "SELECT COUNT(*) FROM identity_group_project_roles WHERE group_id='team'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn migration_preserves_v13_people_history_grants_and_does_not_guess_device_associations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.sqlite");
    let db = Connection::open(&path).unwrap();
    for sql in [
        include_str!("../../../migrations/V1__shared_work.sql"),
        include_str!("../../../migrations/V2__shared_identity.sql"),
        include_str!("../../../migrations/V3__shared_attempt_observation.sql"),
        include_str!("../../../migrations/V4__shared_approvals.sql"),
        include_str!("../../../migrations/V5__shared_management.sql"),
        include_str!("../../../migrations/V6__shared_data.sql"),
        include_str!("../../../migrations/V7__shared_runner_participation.sql"),
        include_str!("../../../migrations/V8__shared_data_retention.sql"),
        include_str!("../../../migrations/V9__shared_managed_files.sql"),
        include_str!("../../../migrations/V10__shared_start_deadlines.sql"),
        include_str!("../../../migrations/V11__remembered_human_sessions.sql"),
        include_str!("../../../migrations/V12__project_devices.sql"),
        include_str!("../../../migrations/V13__initial_password_setup.sql"),
    ] {
        db.execute_batch(sql).unwrap();
    }
    let admin = bootstrap_admin(&db, "admin", "Admin", "old-password-12345", 1).unwrap();
    // Seed the released session shape directly: current login requires the new
    // credential-kind column and must not be used against this historical DB.
    let old_web_token = "ab".repeat(32);
    let old_shared_token = "cd".repeat(32);
    db.execute("INSERT INTO identity_sessions(token_hash,user_id,audience,device_id,credential_generation,expires_at_ms) VALUES(?1,?2,'web_admin',NULL,1,1000)",params![token_hash(&old_web_token),admin.user_id]).unwrap();
    db.execute("INSERT INTO identity_sessions(token_hash,user_id,audience,device_id,credential_generation,expires_at_ms) VALUES(?1,?2,'shared_work','WinA',1,1000)",params![token_hash(&old_shared_token),admin.user_id]).unwrap();
    db.execute_batch("INSERT INTO shared_projects(id,label) VALUES('project','Project');
        INSERT INTO shared_resources(id,capacity) VALUES('resource',1);
        INSERT INTO shared_environments(id,label,resource_id,runner_id) VALUES('env','Environment','resource','WinB');").unwrap();
    set_project_role(
        &db,
        &admin,
        "project",
        &admin.user_id,
        Some(ProjectRole::Reader),
        2,
    )
    .unwrap();
    db.execute("INSERT INTO shared_jobs(id,root_id,project_id,environment_id,requestor_id,assignee_id,request_id,request_json,title,input_json,state,descendant_budget,revision,created_at_ms,updated_at_ms)
        VALUES('old-job','old-job','project','env',?1,?1,'old-request','{}','Old result','{}','succeeded',0,1,1,2)",[&admin.user_id]).unwrap();
    db.pragma_update(None, "user_version", 13).unwrap();
    drop(db);
    let work = SharedWork::open(&path).unwrap();
    assert!(matches!(
        super::super::authenticate(&work.db, &old_web_token, SessionAudience::WebAdmin, None, 3),
        Err(IdentityError::Unauthorized)
    ));
    assert_eq!(
        super::super::authenticate(
            &work.db,
            &old_shared_token,
            SessionAudience::SharedWork,
            Some("WinA"),
            3
        )
        .unwrap(),
        admin
    );
    assert_eq!(list(&work.db).unwrap().len(), 0);
    assert_eq!(active_principal(&work.db, &admin.user_id).unwrap(), admin);
    assert_eq!(
        require_project_role(&work.db, &admin.user_id, "project", ProjectRole::Reader).unwrap(),
        ProjectRole::Reader
    );
    assert_eq!(
        work.db
            .query_row(
                "SELECT requestor_id FROM shared_jobs WHERE id='old-job'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        admin.user_id
    );
    assert!(matches!(
        issue_session(&work.db, "WinA", 3),
        Err(IdentityError::DeviceUnbound)
    ));
    let owner = web_access::local_owner(&work.db, 4).unwrap();
    assert_ne!(owner.user_id, admin.user_id);
    drop(work);
    let reopened = SharedWork::open(&path).unwrap();
    assert_eq!(web_access::local_owner(&reopened.db, 5).unwrap(), owner);
    assert_eq!(
        reopened
            .db
            .query_row(
                "SELECT requestor_id FROM shared_jobs WHERE id='old-job'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        admin.user_id
    );
}

fn registered_device(
    network: &mut crate::network::DeviceNetwork,
    label: &str,
    now: u64,
) -> (String, String) {
    let key = rcgen::KeyPair::generate().unwrap();
    let csr = rcgen::CertificateParams::new(vec!["127.0.0.1".into()])
        .unwrap()
        .serialize_request(&key)
        .unwrap()
        .pem()
        .unwrap();
    let invite = network
        .invite(&network.revision(), label.into(), Default::default(), now)
        .unwrap();
    let receipt = network
        .enroll(
            invite["code"].as_str().unwrap(),
            &csr,
            "127.0.0.1".parse().unwrap(),
            now,
        )
        .unwrap();
    (receipt.device_id, receipt.certificate_sha256)
}

#[test]
fn remote_browser_sessions_recheck_registered_device_binding_admission_and_role() {
    let (work, admin) = fixture();
    let dir = tempfile::tempdir().unwrap();
    let now = crate::service::now_ms();
    let mut network =
        crate::network::DeviceNetwork::open(&dir.path().join("network.json"), "browser-proof-test")
            .unwrap();
    let (device, certificate) = registered_device(&mut network, "WinA", now);
    let ordinary = enroll(&work.db, &device, "WinA", now).unwrap();
    let ordinary_proof =
        web_access::device_proof(&work.db, &ordinary.user_id, &device, &certificate).unwrap();
    assert!(matches!(
        web_access::issue(&work.db, &ordinary.user_id, Some(&ordinary_proof), now),
        Err(IdentityError::Forbidden)
    ));
    bind(&work.db, &admin, &device, Some(&admin.user_id), "WinA", now).unwrap();
    let proof = web_access::device_proof(&work.db, &admin.user_id, &device, &certificate).unwrap();
    let browser = web_access::issue(&work.db, &admin.user_id, Some(&proof), now).unwrap();
    assert_eq!(
        web_access::authenticate(&work.db, &network, &browser.token, now).unwrap(),
        admin
    );
    bind(
        &work.db,
        &admin,
        &device,
        Some(&ordinary.user_id),
        "WinA",
        now + 1,
    )
    .unwrap();
    assert!(web_access::authenticate(&work.db, &network, &browser.token, now + 1).is_err());
    bind(
        &work.db,
        &admin,
        &device,
        Some(&admin.user_id),
        "WinA",
        now + 2,
    )
    .unwrap();
    assert!(web_access::authenticate(&work.db, &network, &browser.token, now + 2).is_err());
    let current =
        web_access::device_proof(&work.db, &admin.user_id, &device, &certificate).unwrap();
    let active = web_access::issue(&work.db, &admin.user_id, Some(&current), now + 3).unwrap();
    network
        .set_admission(&network.revision(), &device, true)
        .unwrap();
    assert!(web_access::authenticate(&work.db, &network, &active.token, now + 4).is_err());
    network
        .set_admission(&network.revision(), &device, false)
        .unwrap();
    assert!(web_access::authenticate(&work.db, &network, &active.token, now + 5).is_ok());
    network
        .set_device(
            &network.revision(),
            &device,
            "WinA".into(),
            Default::default(),
            true,
        )
        .unwrap();
    assert!(web_access::authenticate(&work.db, &network, &active.token, now + 6).is_err());
    let local = web_access::issue(&work.db, &admin.user_id, None, now + 7).unwrap();
    assert!(web_access::authenticate(&work.db, &network, &local.token, now + 8).is_ok());
    assert!(
        web_access::authenticate(&work.db, &network, &local.token, local.expires_at_ms).is_err()
    );
    let local = web_access::issue(&work.db, &admin.user_id, None, now + 9).unwrap();
    logout(&work.db, &local.token, SessionAudience::WebAdmin, None).unwrap();
    assert!(web_access::authenticate(&work.db, &network, &local.token, now + 10).is_err());
}

fn rename_device(
    work: &SharedWork,
    network: &mut crate::network::DeviceNetwork,
    device: &str,
    label: &str,
) -> std::result::Result<serde_json::Value, String> {
    let revision = network.revision();
    crate::admin::dispatch_network_locked(
        work,
        network,
        "hub_network_device",
        &serde_json::json!({"expectedRevision":revision,"deviceId":device,"label":label,"groups":[],"revoked":false}),
    )
}

fn actor_generations(work: &SharedWork, user: &str, device: &str) -> (i64, i64, i64) {
    work.db.query_row("SELECT u.credential_generation,u.authorization_generation,b.generation FROM identity_users u JOIN identity_device_principals b ON b.user_id=u.id WHERE u.id=?1 AND b.device_id=?2",params![user,device],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap()
}

#[test]
fn device_rename_preserves_actor_authority_and_explicit_associations() {
    let (mut work, admin) = fixture();
    let dir = tempfile::tempdir().unwrap();
    let now = crate::service::now_ms();
    let mut network =
        crate::network::DeviceNetwork::open(&dir.path().join("network.json"), "rename-test")
            .unwrap();
    let (a, _) = registered_device(&mut network, "Original A", now);
    let (b, _) = registered_device(&mut network, "Original B", now);
    let actor_a = enroll(&work.db, &a, "Original A", now).unwrap();
    let actor_b = enroll(&work.db, &b, "Original B", now).unwrap();
    project(&mut work, &admin, &[&a]);
    work.create_project(&admin, "history", "Existing project")
        .unwrap();
    set_project_role(
        &work.db,
        &admin,
        "history",
        &actor_a.user_id,
        Some(ProjectRole::Contributor),
        now,
    )
    .unwrap();
    work.register_environment(
        &admin,
        &crate::shared_work::Environment {
            id: "env".into(),
            label: "Environment".into(),
            resource_id: "slot".into(),
            runner_id: b.clone(),
            capacity: 1,
            project_ids: vec!["history".into()],
        },
    )
    .unwrap();
    let job = work
        .submit(
            &actor_a,
            &crate::shared_work::SubmitJob {
                request_id: "before-rename".into(),
                project_id: "history".into(),
                project_participation: None,
                conversation_id: None,
                revises_job_id: None,
                expected_revised_revision: None,
                environment_id: "env".into(),
                origin_session_ref: None,
                origin_turn_ref: None,
                origin_turn_epoch: None,
                title: "Existing job".into(),
                input: serde_json::json!({"prompt":"Test"}),
                descendant_budget: 0,
                start_before_ms: None,
            },
            now,
        )
        .unwrap();
    let session = issue_session(&work.db, &a, now).unwrap();
    let generations = actor_generations(&work, &actor_a.user_id, &a);
    rename_device(&work, &mut network, &a, "  WinA  ").unwrap();
    assert_eq!(network.device_label(&a), Some("WinA"));
    let refreshed = super::super::authenticate(
        &work.db,
        &session.token,
        SessionAudience::SharedWork,
        Some(&a),
        now + 1,
    )
    .unwrap();
    assert_eq!(refreshed.user_id, actor_a.user_id);
    assert_eq!(refreshed.display_name, "WinA");
    assert_eq!(actor_generations(&work, &actor_a.user_id, &a), generations);
    let status = work.status(&refreshed, "history", None, 50, None).unwrap();
    assert_eq!(status.jobs[0].id, job.id);
    assert_eq!(status.jobs[0].requestor.user_id, actor_a.user_id);
    assert_eq!(status.jobs[0].requestor.display_name, "WinA");
    assert_eq!(
        active_principal(&work.db, &actor_b.user_id).unwrap(),
        actor_b
    );

    // An explicit association can select another PC's dedicated actor. Its
    // display name still belongs to that other PC, not the edited device.
    bind(
        &work.db,
        &admin,
        &a,
        Some(&actor_b.user_id),
        "Ignored",
        now + 2,
    )
    .unwrap();
    let linked = issue_session(&work.db, &a, now + 3).unwrap();
    let generations = actor_generations(&work, &actor_b.user_id, &a);
    rename_device(&work, &mut network, &a, "WinA renamed again").unwrap();
    assert_eq!(
        active_principal(&work.db, &actor_b.user_id).unwrap(),
        actor_b
    );
    assert_eq!(actor_generations(&work, &actor_b.user_id, &a), generations);
    assert_eq!(
        super::super::authenticate(
            &work.db,
            &linked.token,
            SessionAudience::SharedWork,
            Some(&a),
            now + 4
        )
        .unwrap(),
        actor_b
    );
    assert_eq!(
        active_principal(&work.db, &actor_a.user_id)
            .unwrap()
            .display_name,
        "WinA renamed again"
    );

    bind(
        &work.db,
        &admin,
        &a,
        Some(&admin.user_id),
        "Ignored",
        now + 5,
    )
    .unwrap();
    let linked = issue_session(&work.db, &a, now + 6).unwrap();
    let generations = actor_generations(&work, &admin.user_id, &a);
    rename_device(&work, &mut network, &a, "WinA final name").unwrap();
    assert_eq!(active_principal(&work.db, &admin.user_id).unwrap(), admin);
    assert_eq!(actor_generations(&work, &admin.user_id, &a), generations);
    assert_eq!(
        super::super::authenticate(
            &work.db,
            &linked.token,
            SessionAudience::SharedWork,
            Some(&a),
            now + 7
        )
        .unwrap(),
        admin
    );
}

#[test]
fn device_rename_reports_durable_network_save_and_can_retry_display_update() {
    let (work, admin) = fixture();
    let dir = tempfile::tempdir().unwrap();
    let now = crate::service::now_ms();
    let mut network =
        crate::network::DeviceNetwork::open(&dir.path().join("network.json"), "rename-retry-test")
            .unwrap();
    let (device, _) = registered_device(&mut network, "Original", now);
    let actor = enroll(&work.db, &device, "Original", now).unwrap();
    work.db.execute_batch("CREATE TRIGGER fail_test_name_write BEFORE UPDATE OF display_name ON identity_users WHEN OLD.credential_kind='device' BEGIN SELECT RAISE(ABORT,'fixture storage failure'); END;").unwrap();
    let error = rename_device(&work, &mut network, &device, "WinA").unwrap_err();
    assert!(error.contains("PCの設定は保存済み"));
    assert_eq!(network.device_label(&device), Some("WinA"));
    assert_eq!(
        active_principal(&work.db, &actor.user_id)
            .unwrap()
            .display_name,
        "Original"
    );
    work.db
        .execute_batch("DROP TRIGGER fail_test_name_write;")
        .unwrap();
    rename_device(&work, &mut network, &device, "WinA").unwrap();
    assert_eq!(
        active_principal(&work.db, &actor.user_id)
            .unwrap()
            .display_name,
        "WinA"
    );
    let revision = work.management_snapshot(&admin).unwrap()["revision"].clone();
    rename_device(&work, &mut network, &device, "WinA").unwrap();
    assert_eq!(
        work.management_snapshot(&admin).unwrap()["revision"],
        revision
    );
}
