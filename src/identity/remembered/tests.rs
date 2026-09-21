use super::*;
const PASSWORD: &str = "correct horse battery staple";

#[test]
fn remembered_login_survives_restart_and_access_expiry_but_logout_revokes_family() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("work.sqlite3");
    let work = crate::shared_work::SharedWork::open(&path).unwrap();
    bootstrap_admin(&work.db, "admin", "Admin", PASSWORD, 10).unwrap();
    let login = login_remembered(&work.db, "admin", PASSWORD, "WinA", 20).unwrap();
    let user_id = login.session.principal.user_id.clone();
    let expiry = login.session.expires_at_ms;
    drop(work);
    let work = crate::shared_work::SharedWork::open(&path).unwrap();
    assert!(
        authenticate(
            &work.db,
            &login.session.token,
            SessionAudience::SharedWork,
            Some("WinA"),
            expiry
        )
        .is_err()
    );
    let renewed = refresh_session(&work.db, &login.refresh_token, "WinA", expiry + 1).unwrap();
    assert_eq!(renewed.principal.user_id, user_id);
    assert!(
        authenticate(
            &work.db,
            &renewed.token,
            SessionAudience::SharedWork,
            Some("WinA"),
            expiry + 2
        )
        .is_ok()
    );
    // A response lost after issuance can repeat refresh without the password.
    let replay = refresh_session(&work.db, &login.refresh_token, "WinA", expiry + 3).unwrap();
    logout_remembered(&work.db, Some(&renewed.token), None, "WinA").unwrap();
    assert!(refresh_session(&work.db, &login.refresh_token, "WinA", expiry + 4).is_err());
    assert!(
        authenticate(
            &work.db,
            &replay.token,
            SessionAudience::SharedWork,
            Some("WinA"),
            expiry + 4
        )
        .is_err()
    );
}

#[test]
fn remembered_identity_needs_exact_device_and_current_human_credentials() {
    let work = crate::shared_work::SharedWork::open(std::path::Path::new(":memory:")).unwrap();
    let admin = bootstrap_admin(&work.db, "admin", "Admin", PASSWORD, 1).unwrap();
    let alice = create_user(&work.db, &admin, "alice", "Alice", PASSWORD, false, 1).unwrap();
    let login = login_remembered(&work.db, "alice", PASSWORD, "WinA", 2).unwrap();
    assert!(refresh_session(&work.db, &login.refresh_token, "WinB", 3).is_err());
    assert!(
        authenticate(
            &work.db,
            &login.refresh_token,
            SessionAudience::WebAdmin,
            None,
            3
        )
        .is_err()
    );
    logout_remembered(&work.db, None, Some(&login.refresh_token), "WinB").unwrap();
    assert!(refresh_session(&work.db, &login.refresh_token, "WinA", 4).is_ok());
    update_user(&work.db, &admin, &alice.user_id, "Alice", false, true, 5).unwrap();
    update_user(&work.db, &admin, &alice.user_id, "Alice", false, false, 6).unwrap();
    assert!(refresh_session(&work.db, &login.refresh_token, "WinA", 7).is_err());
    let reset = login_remembered(&work.db, "alice", PASSWORD, "WinA", 8).unwrap();
    set_password(
        &work.db,
        &admin,
        &alice.user_id,
        "a different secure password",
        9,
    )
    .unwrap();
    assert!(refresh_session(&work.db, &reset.refresh_token, "WinA", 10).is_err());
    assert!(
        authenticate(
            &work.db,
            &reset.session.token,
            SessionAudience::SharedWork,
            Some("WinA"),
            10
        )
        .is_err()
    );
}

#[test]
fn remembered_grants_and_retries_are_bounded_and_store_only_hashes() {
    let work = crate::shared_work::SharedWork::open(std::path::Path::new(":memory:")).unwrap();
    bootstrap_admin(&work.db, "admin", "Admin", PASSWORD, 1).unwrap();
    let login = login_remembered(&work.db, "admin", PASSWORD, "WinA", 2).unwrap();
    for time in 3..20 {
        refresh_session(&work.db, &login.refresh_token, "WinA", time).unwrap();
    }
    let count: u64 = work
        .db
        .query_row(
            "SELECT COUNT(*) FROM identity_sessions WHERE refresh_hash IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 4);
    let stored: String = work
        .db
        .query_row(
            "SELECT token_hash FROM identity_refresh_sessions",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(stored, login.refresh_token);
    assert_eq!(stored, token_hash(&login.refresh_token));
    let replacement = login_remembered(&work.db, "admin", PASSWORD, "WinA", 20).unwrap();
    logout_remembered(&work.db, None, Some(&login.refresh_token), "WinA").unwrap();
    assert!(refresh_session(&work.db, &replacement.refresh_token, "WinA", 21).is_ok());
    assert!(refresh_session(&work.db, &login.refresh_token, "WinA", 21).is_err());
}

#[test]
fn remembered_forward_migration_preserves_old_access_and_does_not_invent_human_login() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pre-remembered.sqlite");
    let conn = Connection::open(&path).unwrap();
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
    ] {
        conn.execute_batch(sql).unwrap();
    }
    let principal = bootstrap_admin(&conn, "admin", "Admin", PASSWORD, 1).unwrap();
    // Seed the released V10 access-session format. Calling today's login owner
    // against a historical schema would not exercise a supported runtime state.
    let old = LoginSession {
        token: "ab".repeat(32),
        expires_at_ms: SESSION_TTL_MS + 2,
        principal,
    };
    conn.execute("INSERT INTO identity_sessions(token_hash,user_id,audience,device_id,credential_generation,expires_at_ms) VALUES(?1,?2,'shared_work','WinA',1,?3)",
        params![token_hash(&old.token),old.principal.user_id,old.expires_at_ms]).unwrap();
    conn.pragma_update(None, "user_version", 10).unwrap();
    drop(conn);
    let work = crate::shared_work::SharedWork::open(&path).unwrap();
    let conn = work.db;
    assert!(
        authenticate(
            &conn,
            &old.token,
            SessionAudience::SharedWork,
            Some("WinA"),
            3
        )
        .is_ok()
    );
    let count: u64 = conn
        .query_row("SELECT COUNT(*) FROM identity_refresh_sessions", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
    assert!(refresh_session(&conn, &old.token, "WinA", 4).is_err());
    logout(&conn, &old.token, SessionAudience::SharedWork, Some("WinA")).unwrap();
    assert!(
        authenticate(
            &conn,
            &old.token,
            SessionAudience::SharedWork,
            Some("WinA"),
            5
        )
        .is_err()
    );
}
