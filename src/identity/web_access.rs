//! Passwordless browser sessions. Only the trusted native Host may select the
//! local owner; remote grants require an authenticated administrator device.
use super::*;

pub(crate) fn local_owner(conn: &Connection, now: u64) -> Result<Principal> {
    write(conn, || {
        if let Some(id) = conn
            .query_row(
                "SELECT user_id FROM identity_local_admin WHERE singleton=1",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            // Possession of this profile's native Host credential is the local
            // recovery authority; an accidental role edit must not require a PW.
            conn.execute("UPDATE identity_users SET administrator=1,disabled=0,credential_generation=credential_generation+1,updated_at_ms=?2 WHERE id=?1 AND credential_kind='local_admin' AND (administrator=0 OR disabled<>0)",params![id,timestamp(now)?])?;
            let principal = active_principal(conn, &id)?;
            require_admin(conn, &principal)?;
            return Ok(principal);
        }
        let id = ulid::Ulid::new().to_string();
        let username = format!("hub-owner-{}", hex(&random_bytes::<16>()?));
        conn.execute("INSERT INTO identity_users(id,username,display_name,administrator,password_algorithm,password_iterations,password_salt,password_hash,created_at_ms,updated_at_ms,credential_kind)
            VALUES(?1,?2,'Hub PC 管理',1,'pbkdf2_sha256',?3,?4,?5,?6,?6,'local_admin')",
            params![id,username,PASSWORD_ITERATIONS,random_bytes::<16>()?.as_slice(),random_bytes::<32>()?.as_slice(),timestamp(now)?])?;
        conn.execute(
            "INSERT INTO identity_local_admin(singleton,user_id) VALUES(1,?1)",
            [&id],
        )?;
        active_principal(conn, &id)
    })
}

#[derive(Clone)]
pub(crate) struct DeviceProof {
    pub device_id: String,
    pub certificate_sha256: String,
    pub binding_generation: i64,
}

pub(crate) fn device_proof(
    conn: &Connection,
    user: &str,
    device: &str,
    certificate: &str,
) -> Result<DeviceProof> {
    let generation = conn
        .query_row(
            "SELECT generation FROM identity_device_principals WHERE device_id=?1 AND user_id=?2",
            params![device, user],
            |r| r.get(0),
        )
        .optional()?
        .ok_or(IdentityError::DeviceUnbound)?;
    Ok(DeviceProof {
        device_id: device.into(),
        certificate_sha256: certificate.into(),
        binding_generation: generation,
    })
}

pub(crate) fn verify_device(
    conn: &Connection,
    network: &crate::network::DeviceNetwork,
    user: &str,
    proof: &DeviceProof,
    now: u64,
) -> Result<()> {
    network
        .admission_revision(&proof.device_id, now)
        .map_err(|_| IdentityError::Unauthorized)?;
    let current = network
        .authenticate(&proof.certificate_sha256, now)
        .map_err(|_| IdentityError::Unauthorized)?;
    let valid: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM identity_device_principals WHERE device_id=?1 AND user_id=?2 AND generation=?3)",params![proof.device_id,user,proof.binding_generation],|r|r.get(0))?;
    if current != proof.device_id || !valid {
        return Err(IdentityError::Unauthorized);
    }
    Ok(())
}

pub(crate) fn issue(
    conn: &Connection,
    user: &str,
    proof: Option<&DeviceProof>,
    now: u64,
) -> Result<LoginSession> {
    write(conn, || {
        let principal = active_principal(conn, user)?;
        require_admin(conn, &principal)?;
        let now = timestamp(now)?;
        conn.execute(
            "DELETE FROM identity_sessions WHERE expires_at_ms<=?1",
            [now],
        )?;
        // Reopening a browser must remain possible without another credential.
        conn.execute("DELETE FROM identity_sessions WHERE user_id=?1 AND audience='web_admin' AND token_hash NOT IN (SELECT token_hash FROM identity_sessions WHERE user_id=?1 AND audience='web_admin' ORDER BY rowid DESC LIMIT 15)",[user])?;
        if conn.query_row("SELECT COUNT(*) FROM identity_sessions", [], |r| {
            r.get::<_, i64>(0)
        })? >= MAX_SESSIONS
        {
            return Err(IdentityError::Limit);
        }
        let generation: i64 = conn.query_row(
            "SELECT credential_generation FROM identity_users WHERE id=?1",
            [user],
            |r| r.get(0),
        )?;
        let token = hex(&random_bytes::<32>()?);
        // Browser administration ends by explicit logout or authority/device
        // revocation. Shared-work credentials retain their own deadlines.
        let expires_at_ms = i64::MAX as u64;
        conn.execute("INSERT INTO identity_sessions(token_hash,user_id,audience,credential_generation,expires_at_ms) VALUES(?1,?2,'web_admin',?3,?4)",params![token_hash(&token),user,generation,timestamp(expires_at_ms)?])?;
        if let Some(proof) = proof {
            conn.execute("INSERT INTO identity_web_session_devices(token_hash,device_id,certificate_sha256,binding_generation) VALUES(?1,?2,?3,?4)",params![token_hash(&token),proof.device_id,proof.certificate_sha256,proof.binding_generation])?;
        }
        Ok(LoginSession {
            token,
            expires_at_ms,
            principal,
        })
    })
}

pub(crate) fn authenticate(
    conn: &Connection,
    network: &crate::network::DeviceNetwork,
    token: &str,
    now: u64,
) -> Result<Principal> {
    let principal = super::authenticate(conn, token, SessionAudience::WebAdmin, None, now)?;
    if let Some(proof) = session_device(conn, token)? {
        verify_device(conn, network, &principal.user_id, &proof, now)?;
    }
    Ok(principal)
}

pub(crate) fn session_device(conn: &Connection, token: &str) -> Result<Option<DeviceProof>> {
    Ok(conn.query_row("SELECT device_id,certificate_sha256,binding_generation FROM identity_web_session_devices WHERE token_hash=?1",[token_hash(token)],|r|Ok(DeviceProof{device_id:r.get(0)?,certificate_sha256:r.get(1)?,binding_generation:r.get(2)?})).optional()?)
}
