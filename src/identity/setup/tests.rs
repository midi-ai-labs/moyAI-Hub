use super::*;
const PASSWORD: &str = "chosen by the person only";

fn database() -> (Connection, Principal) {
    let work = crate::shared_work::SharedWork::open(std::path::Path::new(":memory:")).unwrap();
    let admin = bootstrap_admin(&work.db, "admin", "Admin", PASSWORD, 1).unwrap();
    (work.db, admin)
}

#[test]
fn setup_consumption_is_atomic_once_and_remembered_credentials_are_bound_to_the_device() {
    let (db, admin) = database();
    let issued = create_user_with_setup_code(&db, &admin, "alice", "Alice", 10).unwrap();
    assert_eq!(issued.code.len(), 64);
    let stored: String = db
        .query_row("SELECT code_hash FROM identity_setup_codes", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_ne!(stored, issued.code);
    assert!(
        list_users(&db, &admin)
            .unwrap()
            .iter()
            .find(|u| u.username == "alice")
            .unwrap()
            .setup_pending
    );
    // A rejected new password does not consume the otherwise valid code.
    assert!(complete_password_setup(&db, "alice", &issued.code, "short", "WinA", 11).is_err());
    let session =
        complete_password_setup(&db, "alice", &issued.code, PASSWORD, "WinA", 12).unwrap();
    assert!(!session.session.principal.administrator);
    assert!(complete_password_setup(&db, "alice", &issued.code, PASSWORD, "WinA", 13).is_err());
    assert!(refresh_session(&db, &session.refresh_token, "WinA", 14).is_ok());
    assert!(refresh_session(&db, &session.refresh_token, "WinB", 14).is_err());
    assert!(reissue_setup_code(&db, &admin, &issued.user_id, 15).is_err());
    // If the completion response was lost, ordinary login with the chosen password works.
    assert!(login_remembered(&db, "alice", PASSWORD, "WinA", 15).is_ok());
}

#[test]
fn only_admin_can_issue_and_expiry_reissue_revoke_disable_and_password_reset_invalidate_codes() {
    let (db, admin) = database();
    let user = create_user(&db, &admin, "ordinary", "Ordinary", PASSWORD, false, 2).unwrap();
    assert!(create_user_with_setup_code(&db, &user, "no", "No", 3).is_err());
    assert!(reissue_setup_code(&db, &admin, &user.user_id, 3).is_err());
    let first = create_user_with_setup_code(&db, &admin, "alice", "Alice", 10).unwrap();
    assert!(
        complete_password_setup(
            &db,
            "alice",
            &first.code,
            PASSWORD,
            "WinA",
            first.expires_at_ms
        )
        .is_err()
    );
    let second = reissue_setup_code(&db, &admin, &first.user_id, 20).unwrap();
    assert!(complete_password_setup(&db, "alice", &first.code, PASSWORD, "WinA", 21).is_err());
    assert!(complete_password_setup(&db, "ordinary", &second.code, PASSWORD, "WinA", 21).is_err());
    revoke_setup_code(&db, &admin, &first.user_id, 22).unwrap();
    assert!(complete_password_setup(&db, "alice", &second.code, PASSWORD, "WinA", 23).is_err());
    let third = reissue_setup_code(&db, &admin, &first.user_id, 24).unwrap();
    update_user(&db, &admin, &first.user_id, "Alice", false, true, 25).unwrap();
    assert!(complete_password_setup(&db, "alice", &third.code, PASSWORD, "WinA", 26).is_err());
    update_user(&db, &admin, &first.user_id, "Alice", false, false, 27).unwrap();
    assert!(complete_password_setup(&db, "alice", &third.code, PASSWORD, "WinA", 28).is_err());
    let fourth = reissue_setup_code(&db, &admin, &first.user_id, 29).unwrap();
    set_password(&db, &admin, &first.user_id, PASSWORD, 30).unwrap();
    assert!(complete_password_setup(&db, "alice", &fourth.code, PASSWORD, "WinA", 31).is_err());
}

#[test]
fn released_database_upgrade_preserves_shared_access_and_retires_old_web_access() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("old.db");
    let db = Connection::open(&path).unwrap();
    db.execute_batch(include_str!("../../../migrations/V1__shared_work.sql"))
        .unwrap();
    migrate(&db).unwrap();
    let admin = bootstrap_admin(&db, "admin", "Admin", PASSWORD, 1).unwrap();
    // Seed the released V2 shape; current login requires newer identity columns.
    let web_token = "ab".repeat(32);
    let shared_token = "cd".repeat(32);
    db.execute("INSERT INTO identity_sessions(token_hash,user_id,audience,device_id,credential_generation,expires_at_ms) VALUES(?1,?2,'web_admin',NULL,1,1000)",params![token_hash(&web_token),admin.user_id]).unwrap();
    db.execute("INSERT INTO identity_sessions(token_hash,user_id,audience,device_id,credential_generation,expires_at_ms) VALUES(?1,?2,'shared_work','WinA',1,1000)",params![token_hash(&shared_token),admin.user_id]).unwrap();
    db.pragma_update(None, "user_version", 2).unwrap();
    drop(db);
    let work = crate::shared_work::SharedWork::open(&path).unwrap();
    assert!(matches!(
        authenticate(&work.db, &web_token, SessionAudience::WebAdmin, None, 3),
        Err(IdentityError::Unauthorized)
    ));
    assert_eq!(
        authenticate(
            &work.db,
            &shared_token,
            SessionAudience::SharedWork,
            Some("WinA"),
            3
        )
        .unwrap(),
        admin
    );
    assert!(
        list_users(&work.db, &admin)
            .unwrap()
            .iter()
            .all(|u| !u.setup_pending)
    );
    assert!(login_remembered(&work.db, "admin", PASSWORD, "WinA", 4).is_ok());
}

#[test]
fn an_unclaimed_account_cannot_replace_the_last_usable_administrator() {
    let (db, admin) = database();
    let setup = create_user_with_setup_code(&db, &admin, "pending", "Pending", 10).unwrap();
    assert!(update_user(&db, &admin, &setup.user_id, "Pending", true, false, 11).is_err());
    assert!(update_user(&db, &admin, &admin.user_id, "Admin", false, false, 12).is_err());
    complete_password_setup(&db, "pending", &setup.code, PASSWORD, "WinA", 13).unwrap();
    update_user(&db, &admin, &setup.user_id, "Pending", true, false, 14).unwrap();
    update_user(&db, &admin, &admin.user_id, "Admin", false, false, 15).unwrap();
    assert!(
        login(
            &db,
            "pending",
            PASSWORD,
            SessionAudience::WebAdmin,
            None,
            16
        )
        .is_ok()
    );
}
