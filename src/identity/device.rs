//! A registered device selects an actor through a reviewed, durable association.
//! The caller keeps the device network locked and verifies its certificate first.
use super::*;

#[derive(Debug, Serialize)]
pub struct DevicePrincipal {
    pub device_id: String,
    pub user_id: String,
    pub display_name: String,
}

pub fn list(conn: &Connection) -> Result<Vec<DevicePrincipal>> {
    Ok(conn.prepare("SELECT b.device_id,u.id,u.display_name FROM identity_device_principals b JOIN identity_users u ON u.id=b.user_id ORDER BY b.device_id LIMIT 1152")?
        .query_map([], |r| Ok(DevicePrincipal {device_id:r.get(0)?,user_id:r.get(1)?,display_name:r.get(2)?}))?
        .collect::<std::result::Result<_,_>>()?)
}

/// Called only by the authenticated registry command owner before it stops or
/// revokes a device. Existing approvals capture this principal generation; a
/// registry write failure may conservatively invalidate them, never preserve a
/// stale human authorization after a successful device revocation.
pub(crate) fn invalidate_approvals(conn: &Connection, device: &str) -> Result<()> {
    write(conn, || {
        if !identifier_ok(device) {
            return Err(IdentityError::Invalid("device"));
        }
        conn.execute(
            "UPDATE identity_users SET authorization_generation=authorization_generation+1
             WHERE id IN (SELECT user_id FROM identity_device_principals WHERE device_id=?1)",
            [device],
        )?;
        Ok(())
    })
}

/// Complete the already-reviewed network approval. Replaying approval never
/// replaces an administrator's later explicit association with an existing actor.
pub(crate) fn enroll(
    conn: &Connection,
    device: &str,
    display_name: &str,
    now: u64,
) -> Result<Principal> {
    write(conn, || {
        if !identifier_ok(device) {
            return Err(IdentityError::Invalid("device"));
        }
        if let Some(user) = conn
            .query_row(
                "SELECT user_id FROM identity_device_principals WHERE device_id=?1",
                [device],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            return active_principal(conn, &user);
        }
        let principal = dedicated(conn, device, display_name, now)?;
        conn.execute("INSERT INTO identity_device_principals(device_id,user_id,updated_at_ms) VALUES(?1,?2,?3)",params![device,principal.user_id,timestamp(now)?])?;
        conn.execute(
            "DELETE FROM identity_sessions WHERE audience='shared_work' AND device_id=?1",
            [device],
        )?;
        conn.execute(
            "DELETE FROM identity_refresh_sessions WHERE device_id=?1",
            [device],
        )?;
        Ok(principal)
    })
}

fn dedicated_username(device: &str) -> String {
    format!("device-{}", &hex(&Sha256::digest(device.as_bytes()))[..40])
}

/// The registered PC name owns its dedicated actor's display name. An explicit
/// association with a person or another PC never renames that associated actor.
pub(crate) fn rename_dedicated(
    conn: &Connection,
    device: &str,
    display_name: &str,
    now: u64,
) -> Result<()> {
    if !identifier_ok(device) {
        return Err(IdentityError::Invalid("device"));
    }
    let display_name = display_name_checked(display_name)?;
    write(conn, || {
        conn.execute("UPDATE identity_users SET display_name=?2,updated_at_ms=?3 WHERE credential_kind='device' AND username=?1 AND display_name<>?2",
            params![dedicated_username(device),display_name,timestamp(now)?])?;
        Ok(())
    })
}

fn dedicated(conn: &Connection, device: &str, display_name: &str, now: u64) -> Result<Principal> {
    // The internal username is opaque and is never an enrollment matching key.
    let display_name = display_name_checked(display_name)?;
    let username = dedicated_username(device);
    if let Some(id) = conn
        .query_row(
            "SELECT id FROM identity_users WHERE username=?1 AND credential_kind='device'",
            [&username],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        return active_principal(conn, &id);
    }
    let id = ulid::Ulid::new().to_string();
    if conn.query_row("SELECT COUNT(*) FROM identity_users", [], |r| {
        r.get::<_, i64>(0)
    })? >= MAX_USERS
    {
        return Err(IdentityError::Limit);
    }
    conn.execute("INSERT INTO identity_users(id,username,display_name,administrator,password_algorithm,password_iterations,password_salt,password_hash,created_at_ms,updated_at_ms,credential_kind)
        VALUES(?1,?2,?3,0,'pbkdf2_sha256',?4,?5,?6,?7,?7,'device')",
        params![id,username,display_name,PASSWORD_ITERATIONS,random_bytes::<16>()?.as_slice(),random_bytes::<32>()?.as_slice(),timestamp(now)?])?;
    active_principal(conn, &id)
}

/// Explicit administrator mutation. None means a separate new actor, never a
/// guessed existing person. Repeated approval of an already-bound device is stable.
pub fn bind(
    conn: &Connection,
    actor: &Principal,
    device: &str,
    user_id: Option<&str>,
    display_name: &str,
    now: u64,
) -> Result<Principal> {
    write(conn, || {
        require_admin(conn, actor)?;
        if !identifier_ok(device) {
            return Err(IdentityError::Invalid("device"));
        }
        let existing: Option<String> = conn
            .query_row(
                "SELECT user_id FROM identity_device_principals WHERE device_id=?1",
                [device],
                |r| r.get(0),
            )
            .optional()?;
        let principal = match user_id {
            Some(id) => active_principal(conn, id)?,
            None => dedicated(conn, device, display_name, now)?,
        };
        if existing.as_deref() != Some(principal.user_id.as_str()) {
            conn.execute("INSERT INTO identity_device_principals(device_id,user_id,updated_at_ms) VALUES(?1,?2,?3)
                ON CONFLICT(device_id) DO UPDATE SET user_id=excluded.user_id,generation=generation+1,updated_at_ms=excluded.updated_at_ms",
                params![device,principal.user_id,timestamp(now)?])?;
            // Also remove pre-migration sessions for this exact PC. History and
            // another PC's sessions remain attached to their original actors.
            conn.execute(
                "DELETE FROM identity_sessions WHERE audience='shared_work' AND device_id=?1",
                [device],
            )?;
            conn.execute(
                "DELETE FROM identity_refresh_sessions WHERE device_id=?1",
                [device],
            )?;
        }
        Ok(principal)
    })
}

pub fn issue_session(conn: &Connection, device: &str, now: u64) -> Result<LoginSession> {
    if !identifier_ok(device) {
        return Err(IdentityError::Unauthorized);
    }
    write(conn, || {
        let binding: Option<(Principal,i64,i64)> = conn.query_row(
            "SELECT u.id,u.display_name,u.administrator,u.credential_generation,b.generation FROM identity_device_principals b
             JOIN identity_users u ON u.id=b.user_id WHERE b.device_id=?1 AND u.disabled=0",[device],
            |r|Ok((principal_row(r)?,r.get(3)?,r.get(4)?))).optional()?;
        let (principal, credential_generation, binding_generation) =
            binding.ok_or(IdentityError::DeviceUnbound)?;
        let now = timestamp(now)?;
        conn.execute(
            "DELETE FROM identity_sessions WHERE expires_at_ms<=?1",
            [now],
        )?;
        // Retries and concurrent windows are bounded without evicting other PCs.
        conn.execute("DELETE FROM identity_sessions WHERE audience='shared_work' AND device_id=?1 AND token_hash NOT IN
            (SELECT token_hash FROM identity_sessions WHERE audience='shared_work' AND device_id=?1 ORDER BY expires_at_ms DESC,token_hash LIMIT 3)",[device])?;
        if conn.query_row("SELECT COUNT(*) FROM identity_sessions", [], |r| {
            r.get::<_, i64>(0)
        })? >= MAX_SESSIONS
        {
            return Err(IdentityError::Limit);
        }
        let token = hex(&random_bytes::<32>()?);
        let expires = now
            .checked_add(SESSION_TTL_MS as i64)
            .ok_or(IdentityError::Invalid("time"))?;
        conn.execute("INSERT INTO identity_sessions(token_hash,user_id,audience,device_id,credential_generation,expires_at_ms,device_binding_generation)
            VALUES(?1,?2,'shared_work',?3,?4,?5,?6)",
            params![token_hash(&token),principal.user_id,device,credential_generation,expires,binding_generation])?;
        Ok(LoginSession {
            token,
            expires_at_ms: expires as u64,
            principal,
        })
    })
}

#[cfg(test)]
mod tests;
