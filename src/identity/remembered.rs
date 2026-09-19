//! Revocable remembered human login, bound to a separately authenticated device.
use super::*;

// No Debug: neither credential belongs in diagnostics or public projections.
#[derive(Serialize)]
pub struct RememberedSession {
    #[serde(flatten)]
    pub session: LoginSession,
    pub refresh_token: String,
}

pub fn login_remembered(
    conn: &Connection,
    username: &str,
    password: &str,
    device_id: &str,
    now: u64,
) -> Result<RememberedSession> {
    // Keep failed-password accounting committed independently of issuing a grant.
    let session = login(
        conn,
        username,
        password,
        SessionAudience::SharedWork,
        Some(device_id),
        now,
    )?;
    let issued = write(conn, || {
        conn.execute(
            "DELETE FROM identity_refresh_sessions WHERE user_id=?1 AND device_id=?2",
            params![session.principal.user_id, device_id],
        )?;
        let user_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM identity_refresh_sessions WHERE user_id=?1",
            [&session.principal.user_id],
            |r| r.get(0),
        )?;
        let total: i64 =
            conn.query_row("SELECT COUNT(*) FROM identity_refresh_sessions", [], |r| {
                r.get(0)
            })?;
        if user_count >= MAX_USER_SESSIONS || total >= MAX_SESSIONS {
            return Err(IdentityError::Limit);
        }
        let refresh_token = hex(&random_bytes::<32>()?);
        let hash = token_hash(&refresh_token);
        conn.execute("INSERT INTO identity_refresh_sessions(token_hash,user_id,device_id,credential_generation,created_at_ms,last_used_at_ms)
            SELECT ?1,id,?2,credential_generation,?3,?3 FROM identity_users WHERE id=?4 AND disabled=0",
            params![hash,device_id,timestamp(now)?,session.principal.user_id])?;
        conn.execute(
            "UPDATE identity_sessions SET refresh_hash=?2 WHERE token_hash=?1",
            params![token_hash(&session.token), hash],
        )?;
        Ok(refresh_token)
    });
    match issued {
        Ok(refresh_token) => Ok(RememberedSession {
            session,
            refresh_token,
        }),
        Err(error) => {
            let _ = conn.execute(
                "DELETE FROM identity_sessions WHERE token_hash=?1",
                [token_hash(&session.token)],
            );
            Err(error)
        }
    }
}

/// Caller holds current device-certificate authority through this transaction.
/// Reusing the same sender-constrained refresh credential is safe after a lost
/// response: it issues another bounded access session, never another identity.
pub fn refresh_session(
    conn: &Connection,
    refresh_token: &str,
    device_id: &str,
    now: u64,
) -> Result<LoginSession> {
    if !token_ok(refresh_token) || !identifier_ok(device_id) {
        return Err(IdentityError::Unauthorized);
    }
    write(conn, || {
        let hash = token_hash(refresh_token);
        let (principal, generation): (Principal, i64) = conn.query_row(
            "SELECT u.id,u.display_name,u.administrator,u.credential_generation
             FROM identity_refresh_sessions r JOIN identity_users u ON u.id=r.user_id
             WHERE r.token_hash=?1 AND r.device_id=?2 AND r.credential_generation=u.credential_generation AND u.disabled=0",
            params![hash,device_id], |r| Ok((principal_row(r)?,r.get(3)?)))
            .optional()?.ok_or(IdentityError::Unauthorized)?;
        let now = timestamp(now)?;
        conn.execute(
            "DELETE FROM identity_sessions WHERE expires_at_ms<=?1",
            [now],
        )?;
        // Parallel/retried renewals may coexist until their normal expiry. Bound
        // each grant without evicting another device or WebAdmin login.
        conn.execute("DELETE FROM identity_sessions WHERE refresh_hash=?1 AND token_hash NOT IN
            (SELECT token_hash FROM identity_sessions WHERE refresh_hash=?1 ORDER BY expires_at_ms DESC,token_hash LIMIT 3)", [&hash])?;
        let total: i64 =
            conn.query_row("SELECT COUNT(*) FROM identity_sessions", [], |r| r.get(0))?;
        if total >= MAX_SESSIONS {
            return Err(IdentityError::Limit);
        }
        let token = hex(&random_bytes::<32>()?);
        let expires_at = now
            .checked_add(SESSION_TTL_MS as i64)
            .ok_or(IdentityError::Invalid("time"))?;
        conn.execute("INSERT INTO identity_sessions(token_hash,user_id,audience,device_id,credential_generation,expires_at_ms,refresh_hash)
            VALUES(?1,?2,'shared_work',?3,?4,?5,?6)",
            params![token_hash(&token),principal.user_id,device_id,generation,expires_at,hash])?;
        conn.execute(
            "UPDATE identity_refresh_sessions SET last_used_at_ms=?2 WHERE token_hash=?1",
            params![hash, now],
        )?;
        Ok(LoginSession {
            token,
            expires_at_ms: expires_at as u64,
            principal,
        })
    })
}

/// Either credential identifies the same bounded login family. Expired access
/// may log out; a refresh credential can revoke it after offline local logout.
pub fn logout_remembered(
    conn: &Connection,
    token: Option<&str>,
    refresh_token: Option<&str>,
    device_id: &str,
) -> Result<()> {
    if !identifier_ok(device_id)
        || token.is_some_and(|v| !token_ok(v))
        || refresh_token.is_some_and(|v| !token_ok(v))
    {
        return Err(IdentityError::Unauthorized);
    }
    write(conn, || {
        if let Some(refresh) = refresh_token {
            conn.execute(
                "DELETE FROM identity_refresh_sessions WHERE token_hash=?1 AND device_id=?2",
                params![token_hash(refresh), device_id],
            )?;
        }
        if let Some(token) = token {
            let hash = token_hash(token);
            conn.execute("DELETE FROM identity_refresh_sessions WHERE device_id=?2 AND token_hash IN
                (SELECT refresh_hash FROM identity_sessions WHERE token_hash=?1 AND audience='shared_work' AND device_id=?2)", params![hash,device_id])?;
            conn.execute("DELETE FROM identity_sessions WHERE token_hash=?1 AND audience='shared_work' AND device_id=?2", params![hash,device_id])?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests;
