use super::*;

const PASSWORD: &str = "correct horse battery staple";
const OTHER_PASSWORD: &str = "different horse battery staple";
const NOW: u64 = 1_000_000;

#[test]
fn browser_sessions_survive_elapsed_time_but_logout_and_revocation_still_apply() {
    let conn = database();
    let owner = web_access::local_owner(&conn, NOW).unwrap();
    let session = web_access::issue(&conn, &owner.user_id, None, NOW).unwrap();
    let later = NOW + 500 * 24 * 60 * 60 * 1000;
    assert_eq!(
        authenticate(
            &conn,
            &session.token,
            SessionAudience::WebAdmin,
            None,
            later
        )
        .unwrap(),
        owner
    );
    assert!(
        authenticate(
            &conn,
            &session.token,
            SessionAudience::SharedWork,
            None,
            later
        )
        .is_err()
    );
    logout(&conn, &session.token, SessionAudience::WebAdmin, None).unwrap();
    assert!(
        authenticate(
            &conn,
            &session.token,
            SessionAudience::WebAdmin,
            None,
            later
        )
        .is_err()
    );
    let session = web_access::issue(&conn, &owner.user_id, None, later).unwrap();
    conn.execute(
        "UPDATE identity_users SET credential_generation=credential_generation+1 WHERE id=?1",
        [&owner.user_id],
    )
    .unwrap();
    assert!(
        authenticate(
            &conn,
            &session.token,
            SessionAudience::WebAdmin,
            None,
            later
        )
        .is_err()
    );
}

#[test]
fn browser_session_limit_keeps_the_most_recent_logins() {
    let conn = database();
    let owner = web_access::local_owner(&conn, NOW).unwrap();
    let sessions: Vec<_> = (0..18)
        .map(|i| web_access::issue(&conn, &owner.user_id, None, NOW + i).unwrap())
        .collect();
    for (index, session) in sessions.iter().enumerate() {
        assert_eq!(
            authenticate(
                &conn,
                &session.token,
                SessionAudience::WebAdmin,
                None,
                NOW + SESSION_TTL_MS + 1
            )
            .is_ok(),
            index >= 2
        );
    }
}

fn database() -> Connection {
    let conn = crate::shared_work::SharedWork::open(std::path::Path::new(":memory:"))
        .unwrap()
        .db;
    conn.execute_batch(
        "PRAGMA foreign_keys=ON;
         INSERT INTO shared_projects(id,label) VALUES('alpha','Alpha'),('beta','Beta');",
    )
    .unwrap();
    conn
}

fn administrator(conn: &Connection) -> Principal {
    bootstrap_admin(conn, "admin", "管理者", PASSWORD, NOW).unwrap()
}

fn member(conn: &Connection, admin: &Principal, name: &str) -> Principal {
    create_user(conn, admin, name, name, PASSWORD, false, NOW).unwrap()
}

#[test]
fn bootstrap_is_once_and_credentials_are_salted_without_plaintext() {
    let conn = database();
    assert!(!has_users(&conn).unwrap());
    let admin = administrator(&conn);
    assert!(has_users(&conn).unwrap());
    assert!(matches!(
        bootstrap_admin(&conn, "other", "Other", PASSWORD, NOW),
        Err(IdentityError::Conflict)
    ));
    let user = member(&conn, &admin, "Alice");
    assert!(matches!(
        create_user(&conn, &admin, " alice ", "Duplicate", PASSWORD, false, NOW),
        Err(IdentityError::Conflict)
    ));
    let credential = |id: &str| {
        conn.query_row(
            "SELECT password_salt,password_hash FROM identity_users WHERE id=?1",
            [id],
            |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?)),
        )
        .unwrap()
    };
    let admin_password = credential(&admin.user_id);
    let user_password = credential(&user.user_id);
    assert_ne!(admin_password.0, user_password.0);
    assert_ne!(admin_password.1, user_password.1);
    assert!(
        verify_password(
            PASSWORD,
            PASSWORD_ITERATIONS,
            &user_password.0,
            &user_password.1
        )
        .unwrap()
    );
    let session = login(
        &conn,
        "ALICE",
        PASSWORD,
        SessionAudience::SharedWork,
        Some("WinA"),
        NOW,
    )
    .unwrap();
    assert_eq!(session.principal, user);
    let stored_token: String = conn
        .query_row("SELECT token_hash FROM identity_sessions", [], |r| r.get(0))
        .unwrap();
    assert_ne!(session.token, stored_token);
}

#[test]
fn sessions_require_the_exact_audience_device_and_current_user() {
    let conn = database();
    let admin = administrator(&conn);
    let alice = member(&conn, &admin, "alice");
    assert!(matches!(
        login(
            &conn,
            "alice",
            PASSWORD,
            SessionAudience::WebAdmin,
            None,
            NOW
        ),
        Err(IdentityError::Forbidden)
    ));
    assert!(matches!(
        login(
            &conn,
            "alice",
            PASSWORD,
            SessionAudience::SharedWork,
            None,
            NOW
        ),
        Err(IdentityError::Invalid(_))
    ));
    let shared = login(
        &conn,
        "alice",
        PASSWORD,
        SessionAudience::SharedWork,
        Some("WinA"),
        NOW,
    )
    .unwrap();
    assert_eq!(
        authenticate(
            &conn,
            &shared.token,
            SessionAudience::SharedWork,
            Some("WinA"),
            NOW
        )
        .unwrap(),
        alice
    );
    for (audience, device) in [
        (SessionAudience::SharedWork, Some("WinB")),
        (SessionAudience::WebAdmin, None),
        (SessionAudience::SharedWork, None),
    ] {
        assert!(matches!(
            authenticate(&conn, &shared.token, audience, device, NOW),
            Err(IdentityError::Unauthorized)
        ));
    }
    let web = login(
        &conn,
        "admin",
        PASSWORD,
        SessionAudience::WebAdmin,
        None,
        NOW,
    )
    .unwrap();
    assert!(matches!(
        authenticate(
            &conn,
            &web.token,
            SessionAudience::SharedWork,
            Some("WinA"),
            NOW
        ),
        Err(IdentityError::Unauthorized)
    ));
    assert!(matches!(
        authenticate(
            &conn,
            &shared.token,
            SessionAudience::SharedWork,
            Some("WinA"),
            shared.expires_at_ms
        ),
        Err(IdentityError::Unauthorized)
    ));
    logout(
        &conn,
        &shared.token,
        SessionAudience::SharedWork,
        Some("WinB"),
    )
    .unwrap();
    assert!(
        authenticate(
            &conn,
            &shared.token,
            SessionAudience::SharedWork,
            Some("WinA"),
            NOW
        )
        .is_ok()
    );
    logout(
        &conn,
        &shared.token,
        SessionAudience::SharedWork,
        Some("WinA"),
    )
    .unwrap();
    logout(
        &conn,
        &shared.token,
        SessionAudience::SharedWork,
        Some("WinA"),
    )
    .unwrap();
    assert!(matches!(
        authenticate(
            &conn,
            &shared.token,
            SessionAudience::SharedWork,
            Some("WinA"),
            NOW
        ),
        Err(IdentityError::Unauthorized)
    ));
    assert!(authenticate(&conn, &web.token, SessionAudience::WebAdmin, None, NOW).is_ok());
}

#[test]
fn project_roles_are_current_and_administration_does_not_imply_membership() {
    let conn = database();
    let admin = administrator(&conn);
    let alice = member(&conn, &admin, "alice");
    let bob = member(&conn, &admin, "bob");
    assert!(matches!(
        require_project_role(&conn, &admin.user_id, "alpha", ProjectRole::Reader),
        Err(IdentityError::Forbidden)
    ));
    set_project_role(
        &conn,
        &admin,
        "alpha",
        &alice.user_id,
        Some(ProjectRole::Manager),
        NOW,
    )
    .unwrap();
    set_project_role(
        &conn,
        &admin,
        "alpha",
        &bob.user_id,
        Some(ProjectRole::Reader),
        NOW,
    )
    .unwrap();
    assert_eq!(
        require_project_role(&conn, &alice.user_id, "alpha", ProjectRole::Contributor).unwrap(),
        ProjectRole::Manager
    );
    assert!(matches!(
        require_project_role(&conn, &alice.user_id, "beta", ProjectRole::Reader),
        Err(IdentityError::Forbidden)
    ));
    assert!(matches!(
        require_project_role(&conn, &bob.user_id, "alpha", ProjectRole::Contributor),
        Err(IdentityError::Forbidden)
    ));
    assert!(matches!(
        set_project_role(
            &conn,
            &alice,
            "alpha",
            &bob.user_id,
            Some(ProjectRole::Manager),
            NOW
        ),
        Err(IdentityError::Forbidden)
    ));
    let session = login(
        &conn,
        "alice",
        PASSWORD,
        SessionAudience::SharedWork,
        Some("WinA"),
        NOW,
    )
    .unwrap();
    set_project_role(
        &conn,
        &admin,
        "alpha",
        &alice.user_id,
        Some(ProjectRole::Reader),
        NOW + 1,
    )
    .unwrap();
    assert!(
        authenticate(
            &conn,
            &session.token,
            SessionAudience::SharedWork,
            Some("WinA"),
            NOW + 2
        )
        .is_ok()
    );
    assert!(matches!(
        require_project_role(&conn, &alice.user_id, "alpha", ProjectRole::Contributor),
        Err(IdentityError::Forbidden)
    ));
    set_project_role(&conn, &admin, "alpha", &alice.user_id, None, NOW + 3).unwrap();
    assert!(matches!(
        require_project_role(&conn, &alice.user_id, "alpha", ProjectRole::Reader),
        Err(IdentityError::Forbidden)
    ));
}

#[test]
fn disabled_users_and_password_reset_invalidate_all_existing_sessions() {
    let conn = database();
    let admin = administrator(&conn);
    let alice = member(&conn, &admin, "alice");
    let first = login(
        &conn,
        "alice",
        PASSWORD,
        SessionAudience::SharedWork,
        Some("WinA"),
        NOW,
    )
    .unwrap();
    let second = login(
        &conn,
        "alice",
        PASSWORD,
        SessionAudience::SharedWork,
        Some("WinB"),
        NOW,
    )
    .unwrap();
    set_project_role(
        &conn,
        &admin,
        "alpha",
        &alice.user_id,
        Some(ProjectRole::Contributor),
        NOW,
    )
    .unwrap();
    update_user(&conn, &admin, &alice.user_id, "Alice", false, true, NOW + 1).unwrap();
    assert!(matches!(
        authenticate(
            &conn,
            &first.token,
            SessionAudience::SharedWork,
            Some("WinA"),
            NOW + 2
        ),
        Err(IdentityError::Unauthorized)
    ));
    assert!(matches!(
        require_project_role(&conn, &alice.user_id, "alpha", ProjectRole::Reader),
        Err(IdentityError::Unauthorized)
    ));
    update_user(
        &conn,
        &admin,
        &alice.user_id,
        "Alice",
        false,
        false,
        NOW + 3,
    )
    .unwrap();
    assert!(matches!(
        authenticate(
            &conn,
            &second.token,
            SessionAudience::SharedWork,
            Some("WinB"),
            NOW + 4
        ),
        Err(IdentityError::Unauthorized)
    ));
    let current = login(
        &conn,
        "alice",
        PASSWORD,
        SessionAudience::SharedWork,
        Some("WinA"),
        NOW + 4,
    )
    .unwrap();
    set_password(&conn, &admin, &alice.user_id, OTHER_PASSWORD, NOW + 5).unwrap();
    assert!(matches!(
        authenticate(
            &conn,
            &current.token,
            SessionAudience::SharedWork,
            Some("WinA"),
            NOW + 6
        ),
        Err(IdentityError::Unauthorized)
    ));
    assert!(matches!(
        login(
            &conn,
            "alice",
            PASSWORD,
            SessionAudience::SharedWork,
            Some("WinA"),
            NOW + 6
        ),
        Err(IdentityError::Unauthorized)
    ));
    assert!(
        login(
            &conn,
            "alice",
            OTHER_PASSWORD,
            SessionAudience::SharedWork,
            Some("WinA"),
            NOW + 6
        )
        .is_ok()
    );
}

#[test]
fn old_or_claimed_admin_flags_do_not_authorize_mutations() {
    let conn = database();
    let admin = administrator(&conn);
    assert!(matches!(
        update_user(&conn, &admin, &admin.user_id, "Admin", false, false, NOW),
        Err(IdentityError::Conflict)
    ));
    let second = create_user(&conn, &admin, "second", "Second", PASSWORD, true, NOW).unwrap();
    let second_session = login(
        &conn,
        "second",
        PASSWORD,
        SessionAudience::WebAdmin,
        None,
        NOW,
    )
    .unwrap();
    update_user(
        &conn,
        &admin,
        &second.user_id,
        "Second",
        false,
        false,
        NOW + 1,
    )
    .unwrap();
    assert!(second.administrator);
    assert!(matches!(
        require_admin(&conn, &second),
        Err(IdentityError::Forbidden)
    ));
    assert!(matches!(
        create_user(&conn, &second, "rogue", "Rogue", PASSWORD, true, NOW + 2),
        Err(IdentityError::Forbidden)
    ));
    assert!(matches!(
        authenticate(
            &conn,
            &second_session.token,
            SessionAudience::WebAdmin,
            None,
            NOW + 2
        ),
        Err(IdentityError::Unauthorized)
    ));
}

#[test]
fn identity_writes_share_the_callers_transaction_and_failed_writes_do_not_escape() {
    let mut conn = database();
    let admin = administrator(&conn);
    let alice = member(&conn, &admin, "alice");
    let transaction = conn.transaction().unwrap();
    set_project_role(
        &transaction,
        &admin,
        "alpha",
        &alice.user_id,
        Some(ProjectRole::Contributor),
        NOW,
    )
    .unwrap();
    assert!(
        require_project_role(
            &transaction,
            &alice.user_id,
            "alpha",
            ProjectRole::Contributor
        )
        .is_ok()
    );
    transaction.rollback().unwrap();
    assert!(matches!(
        require_project_role(&conn, &alice.user_id, "alpha", ProjectRole::Reader),
        Err(IdentityError::Forbidden)
    ));
    assert!(matches!(
        set_project_role(
            &conn,
            &admin,
            "missing",
            &alice.user_id,
            Some(ProjectRole::Reader),
            NOW
        ),
        Err(IdentityError::Invalid(_))
    ));
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM identity_project_memberships",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn wrong_password_attempts_have_a_bounded_cooldown_and_unknown_users_create_no_state() {
    let conn = database();
    administrator(&conn);
    for _ in 0..5 {
        assert!(matches!(
            login(
                &conn,
                "admin",
                OTHER_PASSWORD,
                SessionAudience::WebAdmin,
                None,
                NOW
            ),
            Err(IdentityError::Unauthorized)
        ));
    }
    assert!(matches!(
        login(
            &conn,
            "admin",
            PASSWORD,
            SessionAudience::WebAdmin,
            None,
            NOW + 1
        ),
        Err(IdentityError::Unauthorized)
    ));
    assert!(
        login(
            &conn,
            "admin",
            PASSWORD,
            SessionAudience::WebAdmin,
            None,
            NOW + LOGIN_COOLDOWN_MS
        )
        .is_ok()
    );
    assert!(matches!(
        login(
            &conn,
            "unknown",
            PASSWORD,
            SessionAudience::WebAdmin,
            None,
            NOW
        ),
        Err(IdentityError::Unauthorized)
    ));
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM identity_users", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn restart_preserves_identity_sessions_and_project_access_without_plaintext_secrets() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("identity.sqlite3");
    let conn = crate::shared_work::SharedWork::open(&path).unwrap().db;
    conn.execute(
        "INSERT INTO shared_projects(id,label) VALUES('alpha','Alpha')",
        [],
    )
    .unwrap();
    let admin = administrator(&conn);
    let alice = member(&conn, &admin, "alice");
    set_project_role(
        &conn,
        &admin,
        "alpha",
        &alice.user_id,
        Some(ProjectRole::Contributor),
        NOW,
    )
    .unwrap();
    let session = login(
        &conn,
        "alice",
        PASSWORD,
        SessionAudience::SharedWork,
        Some("WinA"),
        NOW,
    )
    .unwrap();
    drop(conn);
    let bytes = std::fs::read(&path).unwrap();
    for secret in [PASSWORD.as_bytes(), session.token.as_bytes()] {
        assert!(!bytes.windows(secret.len()).any(|part| part == secret));
    }
    let conn = crate::shared_work::SharedWork::open(&path).unwrap().db;
    assert_eq!(
        authenticate(
            &conn,
            &session.token,
            SessionAudience::SharedWork,
            Some("WinA"),
            NOW + 1
        )
        .unwrap(),
        alice
    );
    assert_eq!(
        require_project_role(&conn, &alice.user_id, "alpha", ProjectRole::Contributor).unwrap(),
        ProjectRole::Contributor
    );
}
