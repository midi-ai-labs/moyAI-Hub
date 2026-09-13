//! Human identity and project membership in the shared-work database.
//!
//! Callers own the Connection and its transaction. Authenticate an incoming
//! session and authorize its target in the same transaction as a job mutation.
//! Device certificates are verified by the transport, never by a claimed name.
use ring::{
    pbkdf2,
    rand::{SecureRandom, SystemRandom},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::num::NonZeroU32;

pub const MIGRATION_SQL: &str = include_str!("../migrations/V2__shared_identity.sql");
const PASSWORD_ITERATIONS: u32 = 600_000;
const SESSION_TTL_MS: u64 = 8 * 60 * 60 * 1000;
const LOGIN_COOLDOWN_MS: u64 = 30_000;
const MAX_USERS: i64 = 4096;
const MAX_USER_SESSIONS: i64 = 32;
const MAX_SESSIONS: i64 = 16_384;

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("invalid_identity_request: {0}")]
    Invalid(&'static str),
    #[error("identity_conflict")]
    Conflict,
    #[error("identity_limit_reached")]
    Limit,
    #[error("identity_unavailable")]
    Unavailable,
    #[error("identity_storage")]
    Storage(#[from] rusqlite::Error),
}
pub type Result<T> = std::result::Result<T, IdentityError>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Principal {
    pub user_id: String,
    pub display_name: String,
    pub administrator: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionAudience {
    WebAdmin,
    SharedWork,
}
impl SessionAudience {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::WebAdmin => "web_admin",
            Self::SharedWork => "shared_work",
        }
    }
    fn valid_device(self, device_id: Option<&str>) -> bool {
        match (self, device_id) {
            (Self::WebAdmin, None) => true,
            (Self::SharedWork, Some(id)) => identifier_ok(id),
            _ => false,
        }
    }
}

// Deliberately no Debug implementation: the token is returned only at login.
#[derive(Serialize)]
pub struct LoginSession {
    pub token: String,
    pub expires_at_ms: u64,
    pub principal: Principal,
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionStatus {
    pub principal: Principal,
    pub expires_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ProjectRole {
    Reader,
    Contributor,
    Manager,
}
impl ProjectRole {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Reader => "reader",
            Self::Contributor => "contributor",
            Self::Manager => "manager",
        }
    }
    fn parse(value: &str) -> Result<Self> {
        match value {
            "reader" => Ok(Self::Reader),
            "contributor" => Ok(Self::Contributor),
            "manager" => Ok(Self::Manager),
            _ => Err(IdentityError::Unavailable),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct UserRecord {
    pub user_id: String,
    pub username: String,
    pub display_name: String,
    pub administrator: bool,
    pub disabled: bool,
}

/// Run once inside the database owner's forward migration transaction.
pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(MIGRATION_SQL)?;
    Ok(())
}

pub fn has_users(conn: &Connection) -> Result<bool> {
    Ok(
        conn.query_row("SELECT EXISTS(SELECT 1 FROM identity_users)", [], |r| {
            r.get(0)
        })?,
    )
}

/// Trusted local bootstrap only. Never expose this as an anonymous LAN API.
pub fn bootstrap_admin(
    conn: &Connection,
    username: &str,
    display_name: &str,
    password: &str,
    now: u64,
) -> Result<Principal> {
    write(conn, || {
        if has_users(conn)? {
            return Err(IdentityError::Conflict);
        }
        insert_user(conn, username, display_name, password, true, now)
    })
}

pub fn create_user(
    conn: &Connection,
    actor: &Principal,
    username: &str,
    display_name: &str,
    password: &str,
    administrator: bool,
    now: u64,
) -> Result<Principal> {
    write(conn, || {
        require_admin(conn, actor)?;
        insert_user(conn, username, display_name, password, administrator, now)
    })
}

fn insert_user(
    conn: &Connection,
    username: &str,
    display_name: &str,
    password: &str,
    administrator: bool,
    now: u64,
) -> Result<Principal> {
    let username = username_key(username)?;
    let display_name = display_name_checked(display_name)?;
    let now = timestamp(now)?;
    if conn.query_row("SELECT COUNT(*) FROM identity_users", [], |r| {
        r.get::<_, i64>(0)
    })? >= MAX_USERS
    {
        return Err(IdentityError::Limit);
    }
    if conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM identity_users WHERE username=?1)",
        [&username],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(IdentityError::Conflict);
    }
    let (salt, hash) = password_record(password)?;
    let principal = Principal {
        user_id: ulid::Ulid::new().to_string(),
        display_name: display_name.to_owned(),
        administrator,
    };
    conn.execute(
        "INSERT INTO identity_users
         (id,username,display_name,administrator,password_algorithm,password_iterations,
          password_salt,password_hash,created_at_ms,updated_at_ms)
         VALUES(?1,?2,?3,?4,'pbkdf2_sha256',?5,?6,?7,?8,?8)",
        params![
            principal.user_id,
            username,
            display_name,
            administrator,
            PASSWORD_ITERATIONS,
            salt.as_slice(),
            hash.as_slice(),
            now
        ],
    )?;
    Ok(principal)
}

pub fn require_admin(conn: &Connection, actor: &Principal) -> Result<Principal> {
    let current = active_principal(conn, &actor.user_id)?;
    if !current.administrator {
        return Err(IdentityError::Forbidden);
    }
    Ok(current)
}

pub fn active_principal(conn: &Connection, user_id: &str) -> Result<Principal> {
    conn.query_row(
        "SELECT id,display_name,administrator FROM identity_users WHERE id=?1 AND disabled=0",
        [user_id],
        principal_row,
    )
    .optional()?
    .ok_or(IdentityError::Unauthorized)
}

fn principal_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Principal> {
    Ok(Principal {
        user_id: row.get(0)?,
        display_name: row.get(1)?,
        administrator: row.get(2)?,
    })
}

pub fn list_users(conn: &Connection, actor: &Principal) -> Result<Vec<UserRecord>> {
    require_admin(conn, actor)?;
    let mut statement = conn.prepare(
        "SELECT id,username,display_name,administrator,disabled FROM identity_users ORDER BY username LIMIT 4096",
    )?;
    Ok(statement
        .query_map([], |r| {
            Ok(UserRecord {
                user_id: r.get(0)?,
                username: r.get(1)?,
                display_name: r.get(2)?,
                administrator: r.get(3)?,
                disabled: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn update_user(
    conn: &Connection,
    actor: &Principal,
    user_id: &str,
    display_name: &str,
    administrator: bool,
    disabled: bool,
    now: u64,
) -> Result<()> {
    write(conn, || {
        require_admin(conn, actor)?;
        let display_name = display_name_checked(display_name)?;
        let old: Option<(bool, bool)> = conn
            .query_row(
                "SELECT administrator,disabled FROM identity_users WHERE id=?1",
                [user_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((was_admin, was_disabled)) = old else {
            return Err(IdentityError::Invalid("user"));
        };
        if was_admin
            && !was_disabled
            && (!administrator || disabled)
            && conn.query_row(
                "SELECT COUNT(*) FROM identity_users WHERE administrator=1 AND disabled=0",
                [],
                |r| r.get::<_, i64>(0),
            )? <= 1
        {
            return Err(IdentityError::Conflict);
        }
        conn.execute(
            "UPDATE identity_users SET display_name=?2,administrator=?3,disabled=?4,
             credential_generation=credential_generation+?5,updated_at_ms=?6 WHERE id=?1",
            params![
                user_id,
                display_name,
                administrator,
                disabled,
                i64::from(was_admin != administrator || was_disabled != disabled),
                timestamp(now)?
            ],
        )?;
        if was_admin != administrator || was_disabled != disabled {
            conn.execute("DELETE FROM identity_sessions WHERE user_id=?1", [user_id])?;
        }
        Ok(())
    })
}

/// Administrative reset. A user-facing password change needs separate reauthentication.
pub fn set_password(
    conn: &Connection,
    actor: &Principal,
    user_id: &str,
    password: &str,
    now: u64,
) -> Result<()> {
    write(conn, || {
        require_admin(conn, actor)?;
        let (salt, hash) = password_record(password)?;
        if conn.execute(
            "UPDATE identity_users SET password_algorithm='pbkdf2_sha256',password_iterations=?2,
             password_salt=?3,password_hash=?4,credential_generation=credential_generation+1,
             failed_logins=0,blocked_until_ms=0,updated_at_ms=?5 WHERE id=?1",
            params![
                user_id,
                PASSWORD_ITERATIONS,
                salt.as_slice(),
                hash.as_slice(),
                timestamp(now)?
            ],
        )? == 0
        {
            return Err(IdentityError::Invalid("user"));
        }
        conn.execute("DELETE FROM identity_sessions WHERE user_id=?1", [user_id])?;
        Ok(())
    })
}

pub fn login(
    conn: &Connection,
    username: &str,
    password: &str,
    audience: SessionAudience,
    device_id: Option<&str>,
    now: u64,
) -> Result<LoginSession> {
    if !audience.valid_device(device_id) {
        return Err(IdentityError::Invalid("session_binding"));
    }
    if password.is_empty() || password.len() > 1024 {
        return Err(IdentityError::Unauthorized);
    }
    let now = timestamp(now)?;
    let username = username_key(username).map_err(|_| IdentityError::Unauthorized)?;
    type Credentials = (Principal, bool, u32, Vec<u8>, Vec<u8>, i64, i64, i64);
    let record: Option<Credentials> = conn
        .query_row(
            "SELECT id,display_name,administrator,disabled,password_iterations,password_salt,
         password_hash,credential_generation,failed_logins,blocked_until_ms
         FROM identity_users WHERE username=?1",
            [&username],
            |r| {
                Ok((
                    principal_row(r)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                ))
            },
        )
        .optional()?;
    let Some((principal, disabled, iterations, salt, hash, generation, failures, blocked_until)) =
        record
    else {
        // Unknown users incur the same password computation without creating rows.
        let _ = verify_password(password, PASSWORD_ITERATIONS, &[0; 16], &[0; 32]);
        return Err(IdentityError::Unauthorized);
    };
    if blocked_until > now {
        return Err(IdentityError::Unauthorized);
    }
    if !verify_password(password, iterations, &salt, &hash)? || disabled {
        let next_failures = if blocked_until > 0 {
            1
        } else {
            failures.saturating_add(1).min(5)
        };
        let blocked = if next_failures >= 5 {
            now.saturating_add(LOGIN_COOLDOWN_MS as i64)
        } else {
            0
        };
        conn.execute(
            "UPDATE identity_users SET failed_logins=?2,blocked_until_ms=?3 WHERE id=?1",
            params![principal.user_id, next_failures, blocked],
        )?;
        return Err(IdentityError::Unauthorized);
    }
    if audience == SessionAudience::WebAdmin && !principal.administrator {
        return Err(IdentityError::Forbidden);
    }
    write(conn, || {
        conn.execute(
            "DELETE FROM identity_sessions WHERE expires_at_ms<=?1",
            [now],
        )?;
        let per_user: i64 = conn.query_row(
            "SELECT COUNT(*) FROM identity_sessions WHERE user_id=?1",
            [&principal.user_id],
            |r| r.get(0),
        )?;
        let total: i64 =
            conn.query_row("SELECT COUNT(*) FROM identity_sessions", [], |r| r.get(0))?;
        if per_user >= MAX_USER_SESSIONS || total >= MAX_SESSIONS {
            return Err(IdentityError::Limit);
        }
        let token = hex(&random_bytes::<32>()?);
        let expires_at = now
            .checked_add(SESSION_TTL_MS as i64)
            .ok_or(IdentityError::Invalid("time"))?;
        conn.execute(
            "INSERT INTO identity_sessions(token_hash,user_id,audience,device_id,credential_generation,expires_at_ms)
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![token_hash(&token), principal.user_id, audience.as_str(), device_id, generation, expires_at],
        )?;
        conn.execute(
            "UPDATE identity_users SET failed_logins=0,blocked_until_ms=0 WHERE id=?1",
            [&principal.user_id],
        )?;
        Ok(LoginSession {
            token,
            expires_at_ms: expires_at as u64,
            principal,
        })
    })
}

pub fn authenticate(
    conn: &Connection,
    token: &str,
    audience: SessionAudience,
    device_id: Option<&str>,
    now: u64,
) -> Result<Principal> {
    Ok(session_status(conn, token, audience, device_id, now)?.principal)
}

pub fn session_status(
    conn: &Connection,
    token: &str,
    audience: SessionAudience,
    device_id: Option<&str>,
    now: u64,
) -> Result<SessionStatus> {
    if !audience.valid_device(device_id) || !token_ok(token) {
        return Err(IdentityError::Unauthorized);
    }
    let session = conn
        .query_row(
            "SELECT u.id,u.display_name,u.administrator,s.expires_at_ms FROM identity_sessions s
         JOIN identity_users u ON u.id=s.user_id
         WHERE s.token_hash=?1 AND s.audience=?2 AND s.device_id IS ?3
         AND s.expires_at_ms>?4 AND u.disabled=0
         AND s.credential_generation=u.credential_generation",
            params![
                token_hash(token),
                audience.as_str(),
                device_id,
                timestamp(now)?
            ],
            |row| {
                Ok(SessionStatus {
                    principal: principal_row(row)?,
                    expires_at_ms: row.get(3)?,
                })
            },
        )
        .optional()?
        .ok_or(IdentityError::Unauthorized)?;
    if audience == SessionAudience::WebAdmin && !session.principal.administrator {
        return Err(IdentityError::Forbidden);
    }
    Ok(session)
}

pub fn logout(
    conn: &Connection,
    token: &str,
    audience: SessionAudience,
    device_id: Option<&str>,
) -> Result<()> {
    if !audience.valid_device(device_id) || !token_ok(token) {
        return Err(IdentityError::Unauthorized);
    }
    conn.execute(
        "DELETE FROM identity_sessions WHERE token_hash=?1 AND audience=?2 AND device_id IS ?3",
        params![token_hash(token), audience.as_str(), device_id],
    )?;
    Ok(())
}

pub fn require_project_role(
    conn: &Connection,
    user_id: &str,
    project_id: &str,
    minimum: ProjectRole,
) -> Result<ProjectRole> {
    active_principal(conn, user_id)?;
    let role: Option<String> = conn
        .query_row(
            "SELECT role FROM identity_effective_project_memberships WHERE user_id=?1 AND project_id=?2",
            params![user_id, project_id],
            |r| r.get(0),
        )
        .optional()?;
    let role = ProjectRole::parse(&role.ok_or(IdentityError::Forbidden)?)?;
    if role < minimum {
        return Err(IdentityError::Forbidden);
    }
    Ok(role)
}

/// Administrative provisioning does not itself give the administrator project access.
pub fn set_project_role(
    conn: &Connection,
    actor: &Principal,
    project_id: &str,
    user_id: &str,
    role: Option<ProjectRole>,
    now: u64,
) -> Result<()> {
    write(conn, || {
        require_admin(conn, actor)?;
        if !conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM shared_projects WHERE id=?1)",
            [project_id],
            |r| r.get::<_, bool>(0),
        )? || !conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM identity_users WHERE id=?1)",
            [user_id],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(IdentityError::Invalid("membership_target"));
        }
        match role {
            Some(role) => {
                conn.execute(
                "INSERT INTO identity_project_memberships(project_id,user_id,role,updated_at_ms) VALUES(?1,?2,?3,?4)
                 ON CONFLICT(project_id,user_id) DO UPDATE SET role=excluded.role,updated_at_ms=excluded.updated_at_ms",
                params![project_id,user_id,role.as_str(),timestamp(now)?],
            )?;
            }
            None => {
                conn.execute(
                    "DELETE FROM identity_project_memberships WHERE project_id=?1 AND user_id=?2",
                    params![project_id, user_id],
                )?;
            }
        }
        Ok(())
    })
}

fn write<T>(conn: &Connection, operation: impl FnOnce() -> Result<T>) -> Result<T> {
    // A savepoint composes with the shared-work owner's surrounding transaction.
    conn.execute_batch("SAVEPOINT identity_mutation")?;
    match operation() {
        Ok(result) => {
            conn.execute_batch("RELEASE identity_mutation")?;
            Ok(result)
        }
        Err(error) => {
            conn.execute_batch("ROLLBACK TO identity_mutation; RELEASE identity_mutation")?;
            Err(error)
        }
    }
}

fn username_key(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-@".contains(&b))
    {
        return Err(IdentityError::Invalid("username"));
    }
    Ok(value.to_ascii_lowercase())
}

fn display_name_checked(value: &str) -> Result<&str> {
    let value = value.trim();
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(IdentityError::Invalid("display_name"));
    }
    Ok(value)
}

fn identifier_ok(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

fn timestamp(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| IdentityError::Invalid("time"))
}

fn password_record(password: &str) -> Result<([u8; 16], [u8; 32])> {
    if password.chars().count() < 12 || password.len() > 1024 {
        return Err(IdentityError::Invalid("password_length"));
    }
    let salt = random_bytes()?;
    let mut hash = [0; 32];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(PASSWORD_ITERATIONS).expect("positive work factor"),
        &salt,
        password.as_bytes(),
        &mut hash,
    );
    Ok((salt, hash))
}

fn verify_password(password: &str, iterations: u32, salt: &[u8], hash: &[u8]) -> Result<bool> {
    if !(PASSWORD_ITERATIONS..=2_000_000).contains(&iterations)
        || salt.len() != 16
        || hash.len() != 32
    {
        return Err(IdentityError::Unavailable);
    }
    Ok(pbkdf2::verify(
        pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(iterations).ok_or(IdentityError::Unavailable)?,
        salt,
        password.as_bytes(),
        hash,
    )
    .is_ok())
}

fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| IdentityError::Unavailable)?;
    Ok(bytes)
}

fn token_ok(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn token_hash(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(DIGITS[(byte >> 4) as usize] as char);
        text.push(DIGITS[(byte & 15) as usize] as char);
    }
    text
}

#[cfg(test)]
mod tests;
