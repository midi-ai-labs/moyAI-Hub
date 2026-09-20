//! One-time initial password setup. Device approval remains a transport prerequisite.
use super::*;

const SETUP_TTL_MS: u64 = 24 * 60 * 60 * 1000;

// No Debug and no durable plaintext receipt: only the issuing response carries the code.
#[derive(Serialize)]
pub struct IssuedSetupCode {
    pub user_id: String,
    pub username: String,
    pub code: String,
    pub expires_at_ms: u64,
}

pub fn create_user_with_setup_code(
    conn: &Connection,
    actor: &Principal,
    username: &str,
    display_name: &str,
    now: u64,
) -> Result<IssuedSetupCode> {
    write(conn, || {
        require_admin(conn, actor)?;
        // Nobody receives a usable temporary password. Existing accounts are not changed.
        let secret = hex(&random_bytes::<32>()?);
        let person = insert_user(conn, username, display_name, &secret, false, now)?;
        conn.execute(
            "UPDATE identity_users SET setup_pending=1 WHERE id=?1",
            [&person.user_id],
        )?;
        reissue_setup_code(conn, actor, &person.user_id, now)
    })
}

pub fn reissue_setup_code(
    conn: &Connection,
    actor: &Principal,
    user_id: &str,
    now: u64,
) -> Result<IssuedSetupCode> {
    write(conn, || {
        require_admin(conn, actor)?;
        let (username, generation): (String, i64) = conn.query_row(
            "SELECT username,credential_generation FROM identity_users WHERE id=?1 AND setup_pending=1 AND disabled=0 AND administrator=0",
            [user_id], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?.ok_or(IdentityError::Conflict)?;
        let code = hex(&random_bytes::<32>()?);
        let expires_at_ms = now
            .checked_add(SETUP_TTL_MS)
            .ok_or(IdentityError::Invalid("time"))?;
        conn.execute("INSERT INTO identity_setup_codes(user_id,code_hash,credential_generation,issued_by,issued_at_ms,expires_at_ms,consumed_at_ms)
            VALUES(?1,?2,?3,?4,?5,?6,NULL) ON CONFLICT(user_id) DO UPDATE SET code_hash=excluded.code_hash,credential_generation=excluded.credential_generation,
            issued_by=excluded.issued_by,issued_at_ms=excluded.issued_at_ms,expires_at_ms=excluded.expires_at_ms,consumed_at_ms=NULL",
            params![user_id, token_hash(&code), generation, actor.user_id, timestamp(now)?, timestamp(expires_at_ms)?])?;
        Ok(IssuedSetupCode {
            user_id: user_id.into(),
            username,
            code,
            expires_at_ms,
        })
    })
}

pub fn revoke_setup_code(
    conn: &Connection,
    actor: &Principal,
    user_id: &str,
    now: u64,
) -> Result<()> {
    write(conn, || {
        require_admin(conn, actor)?;
        conn.execute("UPDATE identity_setup_codes SET consumed_at_ms=?2 WHERE user_id=?1 AND consumed_at_ms IS NULL", params![user_id, timestamp(now)?])?;
        Ok(())
    })
}

pub fn complete_password_setup(
    conn: &Connection,
    username: &str,
    code: &str,
    password: &str,
    device_id: &str,
    now: u64,
) -> Result<RememberedSession> {
    if !token_ok(code) || !identifier_ok(device_id) {
        return Err(IdentityError::Unauthorized);
    }
    let username = username_key(username).map_err(|_| IdentityError::Unauthorized)?;
    write(conn, || {
        let user_id: String = conn.query_row("SELECT u.id FROM identity_users u JOIN identity_setup_codes c ON c.user_id=u.id
            WHERE u.username=?1 AND c.code_hash=?2 AND u.setup_pending=1 AND u.disabled=0 AND u.administrator=0
            AND c.credential_generation=u.credential_generation AND c.consumed_at_ms IS NULL AND c.expires_at_ms>?3",
            params![username, token_hash(code), timestamp(now)?], |row| row.get(0))
            .optional()?.ok_or(IdentityError::Unauthorized)?;
        let (salt, hash) = password_record(password)?;
        conn.execute("UPDATE identity_users SET password_algorithm='pbkdf2_sha256',password_iterations=?2,password_salt=?3,password_hash=?4,
            credential_generation=credential_generation+1,setup_pending=0,failed_logins=0,blocked_until_ms=0,updated_at_ms=?5 WHERE id=?1",
            params![user_id, PASSWORD_ITERATIONS, salt.as_slice(), hash.as_slice(), timestamp(now)?])?;
        conn.execute(
            "UPDATE identity_setup_codes SET consumed_at_ms=?2 WHERE user_id=?1",
            params![user_id, timestamp(now)?],
        )?;
        conn.execute("DELETE FROM identity_sessions WHERE user_id=?1", [&user_id])?;
        conn.execute(
            "DELETE FROM identity_refresh_sessions WHERE user_id=?1",
            [&user_id],
        )?;
        login_remembered(conn, &username, password, device_id, now)
    })
}

#[cfg(test)]
mod tests;
