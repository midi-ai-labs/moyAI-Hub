//! Persistent device identity and directed authorization. No task bodies enter this module.
pub mod api;
pub mod pki;
pub mod tls;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    net::IpAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub type SharedNetwork = Arc<Mutex<DeviceNetwork>>;
pub type Result<T> = std::result::Result<T, NetworkError>;
const DEVICE_LIMIT: usize = 128;
// Each action has a separate bounded allowance. Routine observation cannot
// consume the capacity needed to cancel already-authorized work.
const GRANT_LIMIT: usize = 4096;
const RETENTION_MS: u64 = 86_400_000;
const CERT_MS: u64 = 30 * RETENTION_MS;
const PRESENCE_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy, thiserror::Error)]
pub enum NetworkError {
    #[error("invalid_request")]
    InvalidRequest,
    #[error("invalid_csr")]
    InvalidCsr,
    #[error("invalid_endpoint")]
    InvalidEndpoint,
    #[error("unauthorized")]
    Unauthorized,
    #[error("device_revoked")]
    Revoked,
    #[error("enrollment_denied")]
    EnrollmentDenied,
    #[error("policy_denied")]
    PolicyDenied,
    #[error("grant_denied")]
    GrantDenied,
    #[error("stale_revision")]
    StaleRevision,
    #[error("registry_full")]
    Full,
    #[error("unavailable")]
    Unavailable,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub id: String,
    pub label: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Subject {
    Device { id: String },
    Group { id: String },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Usage {
    Direct,
    Delegated,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Allow,
    Deny,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub from: Subject,
    pub to: Subject,
    pub usage: Usage,
    pub effect: Effect,
    pub max_depth: u8,
    pub profile_id: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Device {
    device_id: String,
    label: String,
    groups: BTreeSet<String>,
    revoked: bool,
    certificate_pem: String,
    certificate_sha256: String,
    expires_at_ms: u64,
    certificate_ip: IpAddr,
    renewal: Option<RenewalReplay>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RenewalReplay {
    previous_certificate_sha256: String,
    csr_sha256: String,
    expires_at_ms: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnrollmentReplay {
    device_id: String,
    csr_sha256: String,
    ip: IpAddr,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Invitation {
    hash: String,
    label: String,
    groups: BTreeSet<String>,
    expires_at_ms: u64,
    redeemed: Option<EnrollmentReplay>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Store {
    schema_version: u32,
    hub_id: String,
    revision: u64,
    ca_certificate_pem: String,
    ca_private_key_pem: String,
    devices: BTreeMap<String, Device>,
    groups: BTreeMap<String, Group>,
    rules: BTreeMap<String, Rule>,
    invitations: Vec<Invitation>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PublishMode {
    ReadTools,
    Agent,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Publication {
    pub profile_id: String,
    pub name: String,
    pub endpoint: String,
    pub mode: PublishMode,
    pub scope_id: String,
    pub enabled: bool,
}
#[derive(Default)]
struct Presence {
    last_seen_ms: u64,
    observed_ip: Option<IpAddr>,
    publications: BTreeMap<String, Publication>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Claims {
    pub hub_id: String,
    pub origin_device_id: String,
    pub actor_device_id: String,
    pub audience_device_id: String,
    pub profile_id: String,
    pub mode: PublishMode,
    pub scope_id: String,
    pub root_task_id: String,
    pub request_key: String,
    pub parent_job_id: Option<String>,
    pub depth: u8,
    pub device_path: Vec<String>,
}
struct Grant {
    id: String,
    token_hash: String,
    claims: Claims,
    actor_cert: String,
    audience_cert: String,
    expires_at_ms: u64,
    retained_until_ms: u64,
    policy_hash: String,
    parent_grant_id: Option<String>,
    action: Action,
}
#[derive(Serialize)]
pub struct GrantReceipt {
    pub grant_id: String,
    pub token: String,
    pub expires_at_ms: u64,
    pub claims: Claims,
    pub peer: DirectoryPeer,
}
#[derive(Serialize)]
pub struct Introspection {
    pub active: bool,
    pub grant_id: String,
    pub expires_at_ms: u64,
    pub claims: Claims,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantRequest {
    pub audience_device_id: String,
    pub profile_id: String,
    pub root_task_id: String,
    pub request_key: String,
    pub parent_grant_id: Option<String>,
    pub parent_job_id: Option<String>,
    #[serde(default)]
    pub action: Action,
}
#[derive(Clone, Copy, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    #[default]
    Execute,
    Observe,
    Cancel,
}
#[derive(Serialize)]
pub struct CertificateReceipt {
    pub hub_id: String,
    pub device_id: String,
    pub label: String,
    pub certificate_pem: String,
    pub ca_certificate_pem: String,
    pub certificate_sha256: String,
    pub expires_at_ms: u64,
}
#[derive(Clone, Serialize)]
pub struct DirectoryPeer {
    pub device_id: String,
    pub label: String,
    pub profile_id: String,
    pub name: String,
    pub endpoint: String,
    pub mode: PublishMode,
    pub scope_id: String,
    pub certificate_pem: String,
    pub certificate_sha256: String,
}

pub struct DeviceNetwork {
    path: PathBuf,
    store: Store,
    presence: BTreeMap<String, Presence>,
    grants: BTreeMap<String, Grant>,
    cancelled: BTreeMap<(String, String), u64>,
    job_intents: BTreeMap<(String, String), RootBudget>,
}
struct RootBudget {
    expires_at_ms: u64,
    intents: BTreeSet<(String, String, String, String)>,
}

fn text_ok(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
fn id_ok(value: &str) -> bool {
    text_ok(value, 128)
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}
fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn secret() -> String {
    format!("{}{}", ulid::Ulid::new(), ulid::Ulid::new())
}
fn same_hash(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .bytes()
            .zip(right.bytes())
            .fold(0u8, |n, (a, b)| n | (a ^ b))
            == 0
}

impl DeviceNetwork {
    pub fn open(path: &Path, hub_id: &str) -> Result<Self> {
        let store = if path.exists() {
            let mut bytes = Vec::new();
            std::fs::File::open(path)
                .map_err(|_| NetworkError::Unavailable)?
                .take(4 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| NetworkError::Unavailable)?;
            if bytes.len() > 4 * 1024 * 1024 {
                return Err(NetworkError::Full);
            }
            serde_json::from_slice::<Store>(&bytes).map_err(|_| NetworkError::InvalidRequest)?
        } else {
            let (ca, key) = pki::create_ca(hub_id)?;
            Store {
                schema_version: 1,
                hub_id: hub_id.into(),
                revision: 1,
                ca_certificate_pem: ca,
                ca_private_key_pem: key,
                devices: BTreeMap::new(),
                groups: BTreeMap::new(),
                rules: BTreeMap::new(),
                invitations: Vec::new(),
            }
        };
        if store.schema_version != 1
            || store.hub_id != hub_id
            || store.revision == 0
            || store.devices.len() > DEVICE_LIMIT
            || store.groups.len() > 64
            || store.rules.len() > 1024
            || store.invitations.len() > DEVICE_LIMIT
        {
            return Err(NetworkError::InvalidRequest);
        }
        // Parse the persisted key/certificate pair before exposing any listener.
        let (cert, key) = pki::server_certificate(
            &store.ca_certificate_pem,
            &store.ca_private_key_pem,
            hub_id,
            IpAddr::from([127, 0, 0, 1]),
            crate::service::now_ms(),
        )?;
        tls::acceptor(&cert, &key, &store.ca_certificate_pem, true)?;
        tls::verify_server_chain(
            &cert,
            &store.ca_certificate_pem,
            IpAddr::from([127, 0, 0, 1]),
        )?;
        let owner = Self {
            path: path.into(),
            store,
            presence: BTreeMap::new(),
            grants: BTreeMap::new(),
            cancelled: BTreeMap::new(),
            job_intents: BTreeMap::new(),
        };
        if !path.exists() {
            owner.save(&owner.store)?;
        }
        Ok(owner)
    }
    fn save(&self, store: &Store) -> Result<()> {
        let mut file =
            tempfile::NamedTempFile::new_in(self.path.parent().ok_or(NetworkError::Unavailable)?)
                .map_err(|_| NetworkError::Unavailable)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|_| NetworkError::Unavailable)?;
        }
        serde_json::to_writer_pretty(&mut file, store).map_err(|_| NetworkError::Unavailable)?;
        file.write_all(b"\n")
            .map_err(|_| NetworkError::Unavailable)?;
        file.as_file()
            .sync_all()
            .map_err(|_| NetworkError::Unavailable)?;
        file.persist(&self.path)
            .map_err(|_| NetworkError::Unavailable)?;
        Ok(())
    }
    fn edit(&mut self, expected: &str, edit: impl FnOnce(&mut Store) -> Result<()>) -> Result<()> {
        if expected != self.store.revision.to_string() {
            return Err(NetworkError::StaleRevision);
        }
        let mut next = self.store.clone();
        edit(&mut next)?;
        next.revision = next.revision.checked_add(1).ok_or(NetworkError::Full)?;
        self.save(&next)?;
        self.store = next;
        let retired: Vec<_> = self
            .grants
            .iter()
            .filter(|(_, g)| g.policy_hash != self.policy_hash(&g.claims))
            .map(|(id, _)| id.clone())
            .collect();
        for id in retired {
            if let Some(g) = self.grants.get_mut(&id) {
                g.expires_at_ms = 0;
            }
        }
        Ok(())
    }
    pub fn revision(&self) -> String {
        self.store.revision.to_string()
    }
    pub fn hub_id(&self) -> &str {
        &self.store.hub_id
    }
    pub fn ca_certificate(&self) -> &str {
        &self.store.ca_certificate_pem
    }
    pub fn server_certificate(&self, ip: IpAddr, now: u64) -> Result<(String, String)> {
        pki::server_certificate(
            &self.store.ca_certificate_pem,
            &self.store.ca_private_key_pem,
            &self.store.hub_id,
            ip,
            now,
        )
    }
    fn groups_valid(store: &Store, groups: &BTreeSet<String>) -> bool {
        groups.len() <= 64 && groups.iter().all(|id| store.groups.contains_key(id))
    }
    pub fn invite(
        &mut self,
        expected: &str,
        label: String,
        groups: BTreeSet<String>,
        now: u64,
    ) -> Result<serde_json::Value> {
        let code = secret();
        let expires_at_ms = now.saturating_add(600_000);
        self.edit(expected, |store| {
            store.invitations.retain(|i| i.expires_at_ms > now);
            if store.invitations.len() >= DEVICE_LIMIT || store.devices.len() >= DEVICE_LIMIT {
                return Err(NetworkError::Full);
            }
            if !text_ok(&label, 256) || !Self::groups_valid(store, &groups) {
                return Err(NetworkError::InvalidRequest);
            }
            store.invitations.push(Invitation {
                hash: hash(&code),
                label,
                groups,
                expires_at_ms,
                redeemed: None,
            });
            Ok(())
        })?;
        Ok(
            serde_json::json!({"code":code,"expires_at_ms":expires_at_ms,"revision":self.revision()}),
        )
    }
    pub fn set_group(&mut self, expected: &str, id: Option<String>, label: String) -> Result<()> {
        self.edit(expected, |store| {
            if !text_ok(&label, 256) {
                return Err(NetworkError::InvalidRequest);
            }
            let id = match id {
                Some(id) if store.groups.contains_key(&id) => id,
                Some(_) => return Err(NetworkError::InvalidRequest),
                None => ulid::Ulid::new().to_string(),
            };
            if !store.groups.contains_key(&id) && store.groups.len() >= 64 {
                return Err(NetworkError::Full);
            }
            store.groups.insert(id.clone(), Group { id, label });
            Ok(())
        })
    }
    pub fn set_device(
        &mut self,
        expected: &str,
        id: &str,
        label: String,
        groups: BTreeSet<String>,
        revoked: bool,
    ) -> Result<()> {
        self.edit(expected, |store| {
            if !text_ok(&label, 256) || !Self::groups_valid(store, &groups) {
                return Err(NetworkError::InvalidRequest);
            }
            let device = store
                .devices
                .get_mut(id)
                .ok_or(NetworkError::InvalidRequest)?;
            if device.revoked && !revoked {
                return Err(NetworkError::Revoked);
            }
            device.label = label;
            device.groups = groups;
            device.revoked = revoked;
            Ok(())
        })
    }
    pub fn set_rule(&mut self, expected: &str, mut rule: Rule) -> Result<()> {
        self.edit(expected, |store| {
            for subject in [&rule.from, &rule.to] {
                let exists = match subject {
                    Subject::Device { id } => store.devices.contains_key(id),
                    Subject::Group { id } => store.groups.contains_key(id),
                };
                if !exists {
                    return Err(NetworkError::InvalidRequest);
                }
            }
            if !(1..=3).contains(&rule.max_depth)
                || rule.profile_id.as_ref().is_some_and(|s| !id_ok(s))
            {
                return Err(NetworkError::InvalidRequest);
            }
            if rule.id.is_empty() {
                rule.id = ulid::Ulid::new().to_string();
            } else if !store.rules.contains_key(&rule.id) {
                return Err(NetworkError::InvalidRequest);
            }
            if !store.rules.contains_key(&rule.id) && store.rules.len() >= 1024 {
                return Err(NetworkError::Full);
            }
            store.rules.insert(rule.id.clone(), rule);
            Ok(())
        })
    }
    pub fn delete_rule(&mut self, expected: &str, id: &str) -> Result<()> {
        self.edit(expected, |store| {
            store.rules.remove(id).ok_or(NetworkError::InvalidRequest)?;
            Ok(())
        })
    }
    pub fn enroll(
        &mut self,
        code: &str,
        csr: &str,
        ip: IpAddr,
        now: u64,
    ) -> Result<CertificateReceipt> {
        if code.len() > 256 {
            return Err(NetworkError::EnrollmentDenied);
        }
        let index = self
            .store
            .invitations
            .iter()
            .position(|i| i.expires_at_ms > now && same_hash(&i.hash, &hash(code)))
            .ok_or(NetworkError::EnrollmentDenied)?;
        if let Some(replay) = &self.store.invitations[index].redeemed {
            if replay.ip != ip || !same_hash(&replay.csr_sha256, &hash(csr)) {
                return Err(NetworkError::EnrollmentDenied);
            }
            if self
                .active_device(&replay.device_id, now)?
                .renewal
                .is_some()
            {
                return Err(NetworkError::EnrollmentDenied);
            }
            return self.receipt(&replay.device_id);
        }
        if self.store.devices.len() >= DEVICE_LIMIT {
            return Err(NetworkError::Full);
        }
        let id = ulid::Ulid::new().to_string();
        let cert = pki::sign_device(
            &self.store.ca_certificate_pem,
            &self.store.ca_private_key_pem,
            &id,
            csr,
            ip,
            now,
        )?;
        let invitation = self.store.invitations[index].clone();
        let device = Device {
            device_id: id.clone(),
            label: invitation.label,
            groups: invitation.groups,
            revoked: false,
            certificate_sha256: pki::fingerprint(&cert)?,
            certificate_pem: cert,
            expires_at_ms: now.saturating_add(CERT_MS),
            certificate_ip: ip,
            renewal: None,
        };
        self.edit(&self.revision(), |store| {
            store.invitations[index].redeemed = Some(EnrollmentReplay {
                device_id: id.clone(),
                csr_sha256: hash(csr),
                ip,
            });
            store.devices.insert(id.clone(), device);
            Ok(())
        })?;
        self.seen(&id, ip, now)?;
        self.receipt(&id)
    }
    fn receipt(&self, id: &str) -> Result<CertificateReceipt> {
        let d = self
            .store
            .devices
            .get(id)
            .ok_or(NetworkError::Unauthorized)?;
        Ok(CertificateReceipt {
            hub_id: self.store.hub_id.clone(),
            device_id: id.into(),
            label: d.label.clone(),
            certificate_pem: d.certificate_pem.clone(),
            ca_certificate_pem: self.store.ca_certificate_pem.clone(),
            certificate_sha256: d.certificate_sha256.clone(),
            expires_at_ms: d.expires_at_ms,
        })
    }
    pub fn authenticate(&self, cert_hash: &str, now: u64) -> Result<String> {
        let device = self
            .store
            .devices
            .values()
            .find(|d| same_hash(&d.certificate_sha256, cert_hash))
            .ok_or(NetworkError::Unauthorized)?;
        if device.revoked || device.expires_at_ms <= now {
            return Err(NetworkError::Revoked);
        }
        Ok(device.device_id.clone())
    }
    pub fn current_certificate(&self, id: &str, fingerprint: &str, now: u64) -> bool {
        self.authenticate(fingerprint, now)
            .is_ok_and(|owner| owner == id)
    }
    pub fn renew(
        &mut self,
        id: &str,
        csr: &str,
        ip: IpAddr,
        now: u64,
    ) -> Result<CertificateReceipt> {
        let device = self
            .store
            .devices
            .get(id)
            .ok_or(NetworkError::Unauthorized)?;
        if device.revoked || device.expires_at_ms <= now {
            return Err(NetworkError::Revoked);
        }
        let cert = pki::sign_device(
            &self.store.ca_certificate_pem,
            &self.store.ca_private_key_pem,
            id,
            csr,
            ip,
            now,
        )?;
        let fingerprint = pki::fingerprint(&cert)?;
        self.edit(&self.revision(), |store| {
            let device = store
                .devices
                .get_mut(id)
                .ok_or(NetworkError::Unauthorized)?;
            device.renewal = Some(RenewalReplay {
                previous_certificate_sha256: device.certificate_sha256.clone(),
                csr_sha256: hash(csr),
                expires_at_ms: now.saturating_add(600_000),
            });
            device.certificate_pem = cert;
            device.certificate_sha256 = fingerprint;
            device.certificate_ip = ip;
            device.expires_at_ms = now.saturating_add(CERT_MS);
            Ok(())
        })?;
        self.presence.remove(id);
        self.seen(id, ip, now)?;
        self.receipt(id)
    }
    pub fn renew_authenticated(
        &mut self,
        cert_hash: &str,
        csr: &str,
        ip: IpAddr,
        now: u64,
    ) -> Result<CertificateReceipt> {
        if let Ok(id) = self.authenticate(cert_hash, now) {
            return self.renew(&id, csr, ip, now);
        }
        let device = self
            .store
            .devices
            .values()
            .find(|d| {
                !d.revoked
                    && d.certificate_ip == ip
                    && d.renewal.as_ref().is_some_and(|r| {
                        r.expires_at_ms > now
                            && same_hash(&r.previous_certificate_sha256, cert_hash)
                            && same_hash(&r.csr_sha256, &hash(csr))
                    })
            })
            .ok_or(NetworkError::Unauthorized)?;
        self.receipt(&device.device_id)
    }
    fn active_device(&self, id: &str, now: u64) -> Result<&Device> {
        self.store
            .devices
            .get(id)
            .filter(|d| !d.revoked && d.expires_at_ms > now)
            .ok_or(NetworkError::Revoked)
    }
    pub fn seen(&mut self, id: &str, ip: IpAddr, now: u64) -> Result<()> {
        self.active_device(id, now)?;
        let presence = self.presence.entry(id.into()).or_default();
        if presence.observed_ip.is_some_and(|old| old != ip) {
            presence.publications.clear();
        }
        presence.last_seen_ms = now;
        presence.observed_ip = Some(ip);
        Ok(())
    }
    pub fn publish(
        &mut self,
        id: &str,
        publication: Publication,
        ip: IpAddr,
        now: u64,
    ) -> Result<()> {
        let device = self.active_device(id, now)?;
        if !id_ok(&publication.profile_id)
            || !id_ok(&publication.scope_id)
            || !text_ok(&publication.name, 256)
        {
            return Err(NetworkError::InvalidRequest);
        }
        let url = reqwest::Url::parse(&publication.endpoint)
            .map_err(|_| NetworkError::InvalidEndpoint)?;
        if publication.endpoint.len() > 512
            || url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/mcp"
            || url.port_or_known_default() == Some(0)
            || url
                .host_str()
                .and_then(|s| s.trim_matches(['[', ']']).parse::<IpAddr>().ok())
                != Some(ip)
            || device.certificate_ip != ip
        {
            return Err(NetworkError::InvalidEndpoint);
        }
        self.seen(id, ip, now)?;
        let presence = self.presence.get_mut(id).ok_or(NetworkError::Unavailable)?;
        if !presence.publications.contains_key(&publication.profile_id)
            && presence.publications.len() >= 32
        {
            return Err(NetworkError::Full);
        }
        // Any OFF or scope/endpoint change retires all execute grants for the prior publication;
        // re-enabling the same fields does not resurrect old credentials.
        if presence.publications.get(&publication.profile_id) != Some(&publication) {
            for grant in self.grants.values_mut().filter(|g| {
                g.claims.audience_device_id == id && g.claims.profile_id == publication.profile_id
            }) {
                grant.expires_at_ms = 0;
            }
        }
        presence
            .publications
            .insert(publication.profile_id.clone(), publication);
        Ok(())
    }
    fn publication(&self, id: &str, profile: &str, now: u64) -> Result<&Publication> {
        self.active_device(id, now)?;
        self.presence
            .get(id)
            .filter(|p| now.saturating_sub(p.last_seen_ms) <= PRESENCE_MS)
            .and_then(|p| p.publications.get(profile))
            .filter(|p| p.enabled)
            .ok_or(NetworkError::PolicyDenied)
    }
    fn matches(&self, subject: &Subject, device: &Device) -> bool {
        match subject {
            Subject::Device { id } => id == &device.device_id,
            Subject::Group { id } => device.groups.contains(id),
        }
    }
    fn allowed(
        &self,
        from: &str,
        to: &str,
        profile: &str,
        usage: Usage,
        depth: u8,
        now: u64,
    ) -> bool {
        let (Ok(from), Ok(to)) = (self.active_device(from, now), self.active_device(to, now))
        else {
            return false;
        };
        let mut matching: Vec<_> = self
            .store
            .rules
            .values()
            .filter(|r| {
                r.usage == usage
                    && r.profile_id.as_ref().is_none_or(|id| id == profile)
                    && self.matches(&r.from, from)
                    && self.matches(&r.to, to)
            })
            .collect();
        // An explicit device-to-device exception takes precedence over group rules.
        if matching.iter().any(|r| {
            matches!(r.from, Subject::Device { .. }) && matches!(r.to, Subject::Device { .. })
        }) {
            matching.retain(|r| {
                matches!(r.from, Subject::Device { .. }) && matches!(r.to, Subject::Device { .. })
            });
        }
        !matching.iter().any(|r| r.effect == Effect::Deny)
            && matching
                .iter()
                .any(|r| r.effect == Effect::Allow && depth <= r.max_depth)
    }
    fn parent(&self, actor: &str, id: &str, now: u64) -> Result<&Grant> {
        let parent = self
            .grants
            .get(id)
            .filter(|p| p.retained_until_ms > now && p.claims.audience_device_id == actor)
            .ok_or(NetworkError::GrantDenied)?;
        self.validate_ancestry(parent, now)?;
        Ok(parent)
    }
    fn validate_ancestry(&self, grant: &Grant, now: u64) -> Result<()> {
        let mut current = grant;
        for _ in 0..4 {
            if current.action != Action::Execute
                || current.expires_at_ms == 0
                || current.retained_until_ms <= now
                || current.policy_hash != self.policy_hash(&current.claims)
            {
                return Err(NetworkError::GrantDenied);
            }
            self.check_chain(&current.claims, now)?;
            let p = self.publication(
                &current.claims.audience_device_id,
                &current.claims.profile_id,
                now,
            )?;
            if p.scope_id != current.claims.scope_id || p.mode != current.claims.mode {
                return Err(NetworkError::GrantDenied);
            }
            match current.parent_grant_id.as_ref() {
                Some(parent) => {
                    current = self.grants.get(parent).ok_or(NetworkError::GrantDenied)?
                }
                None => return Ok(()),
            }
        }
        Err(NetworkError::GrantDenied)
    }
    fn check_chain(&self, claims: &Claims, now: u64) -> Result<()> {
        if self
            .cancelled
            .get(&(claims.origin_device_id.clone(), claims.root_task_id.clone()))
            .is_some_and(|expires| *expires > now)
        {
            return Err(NetworkError::GrantDenied);
        }
        for device in &claims.device_path {
            self.active_device(device, now)?;
        }
        if !self.allowed(
            &claims.actor_device_id,
            &claims.audience_device_id,
            &claims.profile_id,
            Usage::Direct,
            1,
            now,
        ) || (claims.depth > 1
            && !self.allowed(
                &claims.origin_device_id,
                &claims.audience_device_id,
                &claims.profile_id,
                Usage::Delegated,
                claims.depth - 1,
                now,
            ))
        {
            return Err(NetworkError::PolicyDenied);
        }
        Ok(())
    }
    pub fn directory(
        &self,
        actor: &str,
        parent: Option<&str>,
        now: u64,
    ) -> Result<Vec<DirectoryPeer>> {
        self.active_device(actor, now)?;
        let parent = parent.map(|id| self.parent(actor, id, now)).transpose()?;
        let mut peers = Vec::new();
        for device in self.store.devices.values() {
            if device.device_id == actor {
                continue;
            }
            let Some(presence) = self.presence.get(&device.device_id) else {
                continue;
            };
            for p in presence.publications.values() {
                if self
                    .publication(&device.device_id, &p.profile_id, now)
                    .is_err()
                    || !self.allowed(
                        actor,
                        &device.device_id,
                        &p.profile_id,
                        Usage::Direct,
                        1,
                        now,
                    )
                {
                    continue;
                }
                if parent.is_some_and(|g| {
                    g.claims.depth >= 4
                        || g.claims.device_path.contains(&device.device_id)
                        || !self.allowed(
                            &g.claims.origin_device_id,
                            &device.device_id,
                            &p.profile_id,
                            Usage::Delegated,
                            g.claims.depth,
                            now,
                        )
                }) {
                    continue;
                }
                if peers.len() >= 128 {
                    return Err(NetworkError::Full);
                }
                peers.push(DirectoryPeer {
                    device_id: device.device_id.clone(),
                    label: device.label.clone(),
                    profile_id: p.profile_id.clone(),
                    name: p.name.clone(),
                    endpoint: p.endpoint.clone(),
                    mode: p.mode,
                    scope_id: p.scope_id.clone(),
                    certificate_pem: device.certificate_pem.clone(),
                    certificate_sha256: device.certificate_sha256.clone(),
                });
            }
        }
        Ok(peers)
    }
    pub fn grant(&mut self, actor: &str, request: GrantRequest, now: u64) -> Result<GrantReceipt> {
        self.grants.retain(|_, g| g.retained_until_ms > now);
        self.job_intents.retain(|_, root| root.expires_at_ms > now);
        self.cancelled.retain(|_, expires| *expires > now);
        if self
            .grants
            .values()
            .filter(|g| g.action == request.action)
            .count()
            >= GRANT_LIMIT
        {
            return Err(NetworkError::Full);
        }
        if !id_ok(&request.root_task_id)
            || !id_ok(&request.request_key)
            || !id_ok(&request.profile_id)
            || request.parent_job_id.as_ref().is_some_and(|id| !id_ok(id))
            || request.parent_job_id.is_some() != request.parent_grant_id.is_some()
        {
            return Err(NetworkError::InvalidRequest);
        }
        let actor_device = self.active_device(actor, now)?;
        let audience = self.active_device(&request.audience_device_id, now)?;
        let publication_metadata = self
            .presence
            .get(&request.audience_device_id)
            .and_then(|p| p.publications.get(&request.profile_id))
            .ok_or(NetworkError::GrantDenied)?;
        let peer = DirectoryPeer {
            device_id: audience.device_id.clone(),
            label: audience.label.clone(),
            profile_id: publication_metadata.profile_id.clone(),
            name: publication_metadata.name.clone(),
            endpoint: publication_metadata.endpoint.clone(),
            mode: publication_metadata.mode,
            scope_id: publication_metadata.scope_id.clone(),
            certificate_pem: audience.certificate_pem.clone(),
            certificate_sha256: audience.certificate_sha256.clone(),
        };
        if request.action != Action::Execute {
            let old = self
                .grants
                .values()
                .find(|g| {
                    g.action == Action::Execute
                        && g.claims.actor_device_id == actor
                        && g.claims.audience_device_id == request.audience_device_id
                        && g.claims.profile_id == request.profile_id
                        && g.claims.root_task_id == request.root_task_id
                        && g.claims.request_key == request.request_key
                        && g.claims.parent_job_id == request.parent_job_id
                        && g.retained_until_ms > now
                })
                .ok_or(NetworkError::GrantDenied)?;
            let token = secret();
            let id = ulid::Ulid::new().to_string();
            let claims = old.claims.clone();
            let expires_at_ms = now.saturating_add(60_000).min(old.retained_until_ms);
            let grant = Grant {
                id: id.clone(),
                token_hash: hash(&token),
                claims: claims.clone(),
                actor_cert: actor_device.certificate_sha256.clone(),
                audience_cert: audience.certificate_sha256.clone(),
                expires_at_ms,
                // Control tokens never become ancestry. Their accepted execute
                // record remains available for exact reauthorization for 24h.
                retained_until_ms: expires_at_ms,
                policy_hash: old.policy_hash.clone(),
                parent_grant_id: old.parent_grant_id.clone(),
                action: request.action,
            };
            self.grants.insert(id.clone(), grant);
            return Ok(GrantReceipt {
                grant_id: id,
                token,
                expires_at_ms,
                claims,
                peer,
            });
        }
        let publication =
            self.publication(&request.audience_device_id, &request.profile_id, now)?;
        let parent = request
            .parent_grant_id
            .as_deref()
            .map(|id| self.parent(actor, id, now))
            .transpose()?;
        let (origin, depth, mut path) = if let Some(parent) = parent {
            if request.root_task_id != parent.claims.root_task_id {
                return Err(NetworkError::GrantDenied);
            }
            (
                parent.claims.origin_device_id.clone(),
                parent.claims.depth + 1,
                parent.claims.device_path.clone(),
            )
        } else {
            (actor.to_string(), 1, vec![actor.to_string()])
        };
        if depth > 4 || path.contains(&request.audience_device_id) {
            return Err(NetworkError::GrantDenied);
        }
        path.push(request.audience_device_id.clone());
        let claims = Claims {
            hub_id: self.store.hub_id.clone(),
            origin_device_id: origin,
            actor_device_id: actor.into(),
            audience_device_id: request.audience_device_id,
            profile_id: request.profile_id,
            mode: publication.mode,
            scope_id: publication.scope_id.clone(),
            root_task_id: request.root_task_id,
            request_key: request.request_key,
            parent_job_id: request.parent_job_id,
            depth,
            device_path: path,
        };
        self.check_chain(&claims, now)?;
        let root = (claims.origin_device_id.clone(), claims.root_task_id.clone());
        let intent = (
            claims.actor_device_id.clone(),
            claims.audience_device_id.clone(),
            claims.profile_id.clone(),
            claims.request_key.clone(),
        );
        if self
            .job_intents
            .get(&root)
            .is_some_and(|root| root.intents.len() >= 32 && !root.intents.contains(&intent))
            || (!self.job_intents.contains_key(&root) && self.job_intents.len() >= 128)
        {
            return Err(NetworkError::Full);
        }
        let token = secret();
        let id = ulid::Ulid::new().to_string();
        let expires_at_ms = now.saturating_add(60_000);
        let grant = Grant {
            id: id.clone(),
            token_hash: hash(&token),
            claims: claims.clone(),
            actor_cert: actor_device.certificate_sha256.clone(),
            audience_cert: audience.certificate_sha256.clone(),
            expires_at_ms,
            retained_until_ms: now.saturating_add(RETENTION_MS),
            policy_hash: self.policy_hash(&claims),
            parent_grant_id: request.parent_grant_id,
            action: Action::Execute,
        };
        self.grants.insert(id.clone(), grant);
        self.job_intents
            .entry(root)
            .or_insert_with(|| RootBudget {
                expires_at_ms: now.saturating_add(RETENTION_MS),
                intents: BTreeSet::new(),
            })
            .intents
            .insert(intent);
        Ok(GrantReceipt {
            grant_id: id,
            token,
            expires_at_ms,
            claims,
            peer,
        })
    }
    pub fn introspect(
        &self,
        audience: &str,
        token: &str,
        actor_cert: &str,
        action: Action,
        now: u64,
    ) -> Result<Introspection> {
        if token.len() > 256 || actor_cert.len() != 64 {
            return Err(NetworkError::GrantDenied);
        }
        let token_hash = hash(token);
        let g = self
            .grants
            .values()
            .find(|g| same_hash(&g.token_hash, &token_hash) && g.retained_until_ms > now)
            .ok_or(NetworkError::GrantDenied)?;
        if audience != g.claims.audience_device_id
            || !same_hash(&g.actor_cert, actor_cert)
            || !self.current_certificate(&g.claims.actor_device_id, &g.actor_cert, now)
            || !self.current_certificate(audience, &g.audience_cert, now)
        {
            return Err(NetworkError::GrantDenied);
        }
        if action == Action::Execute {
            if g.expires_at_ms <= now {
                return Err(NetworkError::GrantDenied);
            }
            self.validate_ancestry(g, now)?;
        } else if action == Action::Cancel && g.action == Action::Observe {
            return Err(NetworkError::GrantDenied);
        }
        Ok(Introspection {
            active: true,
            grant_id: g.id.clone(),
            expires_at_ms: if action == Action::Execute {
                g.expires_at_ms
            } else {
                g.retained_until_ms
            },
            claims: g.claims.clone(),
        })
    }
    pub fn cancel_lineage(&mut self, origin: &str, task: &str, now: u64) -> Result<()> {
        self.active_device(origin, now)?;
        if !id_ok(task) {
            return Err(NetworkError::InvalidRequest);
        }
        self.cancelled.retain(|_, expires| *expires > now);
        if self.cancelled.len() >= GRANT_LIMIT
            && !self.cancelled.contains_key(&(origin.into(), task.into()))
        {
            return Err(NetworkError::Full);
        }
        self.cancelled.insert(
            (origin.into(), task.into()),
            now.saturating_add(RETENTION_MS),
        );
        Ok(())
    }
    pub fn reset_runtime(&mut self) {
        self.presence.clear();
        self.grants.clear();
        self.cancelled.clear();
        self.job_intents.clear();
    }
    fn policy_hash(&self, claims: &Claims) -> String {
        let rules: Vec<_> = self
            .store
            .rules
            .values()
            .filter(|r| {
                let from = if r.usage == Usage::Direct {
                    &claims.actor_device_id
                } else {
                    &claims.origin_device_id
                };
                let (Some(from), Some(to)) = (
                    self.store.devices.get(from),
                    self.store.devices.get(&claims.audience_device_id),
                ) else {
                    return false;
                };
                (r.usage == Usage::Direct || claims.depth > 1)
                    && self.matches(&r.from, from)
                    && self.matches(&r.to, to)
                    && r.profile_id
                        .as_ref()
                        .is_none_or(|id| id == &claims.profile_id)
            })
            .collect();
        let devices: Vec<_> = claims
            .device_path
            .iter()
            .filter_map(|id| self.store.devices.get(id))
            .map(|d| (&d.device_id, d.revoked, &d.certificate_sha256))
            .collect();
        let bytes = serde_json::to_vec(&(rules, devices)).expect("policy serialization");
        format!("{:x}", Sha256::digest(bytes))
    }
    pub fn self_projection(&self, id: &str, now: u64) -> Result<serde_json::Value> {
        let d = self.active_device(id, now)?;
        let cancelled: BTreeSet<_> = self
            .grants
            .values()
            .filter(|g| {
                g.claims.device_path.iter().any(|d| d == id)
                    && self
                        .cancelled
                        .get(&(
                            g.claims.origin_device_id.clone(),
                            g.claims.root_task_id.clone(),
                        ))
                        .is_some_and(|expires| *expires > now)
            })
            .map(|g| {
                (
                    g.claims.origin_device_id.clone(),
                    g.claims.root_task_id.clone(),
                )
            })
            .collect();
        Ok(
            serde_json::json!({"hub_id":self.hub_id(),"device_id":id,"label":d.label,"groups":d.groups,"revision":self.revision(),
            "certificate_sha256":d.certificate_sha256,"expires_at_ms":d.expires_at_ms,
            "cancelled_lineages":cancelled.into_iter().map(|(origin_device_id,root_task_id)|serde_json::json!({"origin_device_id":origin_device_id,"root_task_id":root_task_id})).collect::<Vec<_>>() }),
        )
    }
    pub fn snapshot(&self, now: u64) -> serde_json::Value {
        let devices:Vec<_>=self.store.devices.values().map(|d| {
            let p=self.presence.get(&d.device_id);
            serde_json::json!({"device_id":d.device_id,"label":d.label,"groups":d.groups,"revoked":d.revoked,
                "certificate_sha256":d.certificate_sha256,"expires_at_ms":d.expires_at_ms,
                "last_seen_ms":p.map(|p|p.last_seen_ms),"observed_ip":p.and_then(|p|p.observed_ip),
                "online":!d.revoked&&d.expires_at_ms>now&&p.is_some_and(|p|now.saturating_sub(p.last_seen_ms)<=PRESENCE_MS),
                "publications":p.map(|p|p.publications.values().cloned().collect::<Vec<_>>()).unwrap_or_default()})
        }).collect();
        serde_json::json!({"revision":self.revision(),"hub_id":self.hub_id(),"ca_certificate_pem":self.ca_certificate(),
            "ca_sha256":pki::fingerprint(self.ca_certificate()).ok(),"devices":devices,
            "groups":self.store.groups.values().collect::<Vec<_>>(),"rules":self.store.rules.values().collect::<Vec<_>>()})
    }
}
