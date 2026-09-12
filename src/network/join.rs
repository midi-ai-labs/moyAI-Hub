//! Pending, key-bound participation requests. These never authorize product APIs.
use super::*;
use rcgen::{CertificateSigningRequestParams, DnType, DnValue, PublicKeyData};

const JOIN_LIMIT: usize = 128;
const JOIN_IP_LIMIT: usize = 8;
const JOIN_MS: u64 = 86_400_000;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct JoinRequest {
    request_id: String,
    label: String,
    observed_ip: IpAddr,
    key_sha256: String,
    csr_pem: String,
    challenge: String,
    created_at_ms: u64,
    expires_at_ms: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct JoinApproval {
    request: JoinRequest,
    certificate_sha256: String,
}

#[derive(Serialize)]
pub struct JoinReceipt {
    pub hub_id: String,
    pub request_id: String,
    pub challenge: String,
    pub expires_at_ms: u64,
}

fn parsed(csr: &str, ip: IpAddr) -> Result<CertificateSigningRequestParams> {
    if csr.len() > 16 * 1024 || ip.is_unspecified() || ip.is_multicast() {
        return Err(NetworkError::InvalidCsr);
    }
    // rcgen verifies the PKCS#10 signature before returning the requested key.
    let parsed =
        CertificateSigningRequestParams::from_pem(csr).map_err(|_| NetworkError::InvalidCsr)?;
    if parsed.params.subject_alt_names != vec![rcgen::SanType::IpAddress(ip)] {
        return Err(NetworkError::InvalidCsr);
    }
    Ok(parsed)
}
fn key_hash(csr: &CertificateSigningRequestParams) -> String {
    format!(
        "{:x}",
        Sha256::digest(csr.public_key.subject_public_key_info())
    )
}
fn certificate_key_hash(certificate: &str) -> Result<String> {
    use tokio_rustls::rustls::pki_types::{CertificateDer, pem::PemObject};
    use x509_parser::prelude::FromDer;
    let der = CertificateDer::from_pem_slice(certificate.as_bytes())
        .map_err(|_| NetworkError::InvalidRequest)?;
    let (_, cert) = x509_parser::certificate::X509Certificate::from_der(&der)
        .map_err(|_| NetworkError::InvalidRequest)?;
    Ok(format!("{:x}", Sha256::digest(cert.public_key().raw)))
}
impl JoinRequest {
    fn receipt(&self, hub: &str) -> JoinReceipt {
        JoinReceipt {
            hub_id: hub.into(),
            request_id: self.request_id.clone(),
            challenge: self.challenge.clone(),
            expires_at_ms: self.expires_at_ms,
        }
    }
    fn validate(&self) -> Result<()> {
        if !id_ok(&self.request_id)
            || !id_ok(&self.challenge)
            || !text_ok(&self.label, 256)
            || self.expires_at_ms != self.created_at_ms.saturating_add(JOIN_MS)
            || self.key_sha256 != key_hash(&parsed(&self.csr_pem, self.observed_ip)?)
        {
            return Err(NetworkError::InvalidRequest);
        }
        Ok(())
    }
    fn prove(&self, hub: &str, csr: &str, ip: IpAddr) -> Result<()> {
        let proof = parsed(csr, ip)?;
        let expected = format!("moyAI join {hub} {} {}", self.request_id, self.challenge);
        if ip != self.observed_ip
            || !same_hash(&self.key_sha256, &key_hash(&proof))
            || proof.params.distinguished_name.get(&DnType::CommonName)
                != Some(&DnValue::Utf8String(expected))
        {
            return Err(NetworkError::Unauthorized);
        }
        Ok(())
    }
}

impl DeviceNetwork {
    pub(super) fn pending_label_conflicts(
        store: &Store,
        key: &str,
        request_id: Option<&str>,
        now: u64,
    ) -> bool {
        store.join_requests.values().any(|request| {
            request.expires_at_ms > now
                && Some(request.request_id.as_str()) != request_id
                && Self::label_key(&request.label) == key
        })
    }
    pub(super) fn reject_registered_key(&self, csr: &str, ip: IpAddr) -> Result<()> {
        let key = key_hash(&parsed(csr, ip)?);
        for device in self.store.devices.values() {
            if same_hash(&certificate_key_hash(&device.certificate_pem)?, &key) {
                return Err(NetworkError::JoinSuperseded);
            }
        }
        Ok(())
    }
    pub(super) fn validate_joins(store: &Store) -> Result<()> {
        if store.join_requests.len() > JOIN_LIMIT {
            return Err(NetworkError::Full);
        }
        let mut keys = BTreeSet::new();
        let mut ids = BTreeSet::new();
        for (id, request) in &store.join_requests {
            request.validate()?;
            if id != &request.request_id
                || !keys.insert(request.key_sha256.clone())
                || !ids.insert(request.request_id.clone())
            {
                return Err(NetworkError::InvalidRequest);
            }
        }
        for device in store.devices.values() {
            if device.admission_revision == 0 || device.admission_revision > store.revision {
                return Err(NetworkError::InvalidRequest);
            }
            if let Some(approval) = &device.join_approval {
                approval.request.validate()?;
                if !keys.insert(approval.request.key_sha256.clone())
                    || !ids.insert(approval.request.request_id.clone())
                    || approval.certificate_sha256.len() != 64
                {
                    return Err(NetworkError::InvalidRequest);
                }
            }
        }
        Ok(())
    }

    pub fn request_join(
        &mut self,
        label: String,
        csr: &str,
        ip: IpAddr,
        now: u64,
    ) -> Result<JoinReceipt> {
        if !text_ok(&label, 256) {
            return Err(NetworkError::InvalidRequest);
        }
        let key_sha256 = key_hash(&parsed(csr, ip)?);
        for device in self.store.devices.values() {
            if let Some(approval) = &device.join_approval {
                if same_hash(&approval.request.key_sha256, &key_sha256) {
                    if device.deleted_at_ms.is_some() {
                        return Err(NetworkError::Revoked);
                    }
                    if approval.request.csr_pem == csr && approval.request.observed_ip == ip {
                        return Ok(approval.request.receipt(self.hub_id()));
                    }
                    return Err(NetworkError::JoinSuperseded);
                }
            }
            // Losing local settings must not bypass a registered identity or its revocation.
            if same_hash(&certificate_key_hash(&device.certificate_pem)?, &key_sha256) {
                return Err(NetworkError::JoinSuperseded);
            }
        }
        if let Some(request) = self.store.join_requests.values().find(|request| {
            request.key_sha256 == key_sha256
                && request.csr_pem == csr
                && request.expires_at_ms > now
        }) {
            return Ok(request.receipt(self.hub_id()));
        }
        let request = JoinRequest {
            request_id: ulid::Ulid::new().to_string(),
            label,
            observed_ip: ip,
            key_sha256: key_sha256.clone(),
            csr_pem: csr.into(),
            challenge: secret(),
            created_at_ms: now,
            expires_at_ms: now.saturating_add(JOIN_MS),
        };
        self.edit(&self.revision(), |store| {
            store
                .join_requests
                .retain(|_, old| old.expires_at_ms > now && old.key_sha256 != key_sha256);
            let mut request = request.clone();
            request.label = Self::available_label(store, &request.label, None, None, now)?;
            if store.join_requests.len() >= JOIN_LIMIT
                || store
                    .join_requests
                    .values()
                    .filter(|old| old.observed_ip == ip)
                    .count()
                    >= JOIN_IP_LIMIT
            {
                return Err(NetworkError::Full);
            }
            store
                .join_requests
                .insert(request.request_id.clone(), request.clone());
            Ok(())
        })?;
        Ok(request.receipt(self.hub_id()))
    }

    pub fn join_status(
        &self,
        id: &str,
        proof: &str,
        ip: IpAddr,
        now: u64,
    ) -> Result<serde_json::Value> {
        if !id_ok(id) {
            return Err(NetworkError::Unauthorized);
        }
        if let Some(request) = self.store.join_requests.get(id) {
            request.prove(self.hub_id(), proof, ip)?;
            let status = if request.expires_at_ms > now {
                "pending"
            } else {
                "expired"
            };
            return Ok(
                serde_json::json!({"hub_id":self.hub_id(),"request_id":id,"status":status,"certificate":null}),
            );
        }
        let device = self
            .store
            .devices
            .values()
            .find(|device| {
                device
                    .join_approval
                    .as_ref()
                    .is_some_and(|approval| approval.request.request_id == id)
            })
            .ok_or(NetworkError::Unauthorized)?;
        let approval = device
            .join_approval
            .as_ref()
            .ok_or(NetworkError::Unauthorized)?;
        approval.request.prove(self.hub_id(), proof, ip)?;
        let status = if device.revoked {
            "revoked"
        } else if device.expires_at_ms <= now {
            "expired"
        } else if device.stopped {
            "stopped"
        } else {
            "approved"
        };
        let certificate = if matches!(status, "revoked" | "expired") {
            None
        } else {
            if device.certificate_sha256 != approval.certificate_sha256 {
                return Err(NetworkError::JoinSuperseded);
            }
            Some(self.receipt(&device.device_id)?)
        };
        Ok(
            serde_json::json!({"hub_id":self.hub_id(),"request_id":id,"status":status,"certificate":certificate}),
        )
    }

    pub fn approve_join(
        &mut self,
        expected: &str,
        id: &str,
        label: String,
        groups: BTreeSet<String>,
        now: u64,
    ) -> Result<()> {
        if expected != self.revision() {
            return Err(NetworkError::StaleRevision);
        }
        let request = self
            .store
            .join_requests
            .get(id)
            .filter(|request| request.expires_at_ms > now)
            .cloned()
            .ok_or(NetworkError::InvalidRequest)?;
        if !text_ok(&label, 256) || !Self::groups_valid(&self.store, &groups) {
            return Err(NetworkError::InvalidRequest);
        }
        let device_id = ulid::Ulid::new().to_string();
        let certificate = pki::sign_device(
            &self.store.ca_certificate_pem,
            &self.store.ca_private_key_pem,
            &device_id,
            &request.csr_pem,
            request.observed_ip,
            now,
        )?;
        let fingerprint = pki::fingerprint(&certificate)?;
        self.edit(expected, |store| {
            if Self::registered_count(store) >= DEVICE_LIMIT {
                return Err(NetworkError::Full);
            }
            let label = Self::available_label(store, &label, None, Some(id), now)?;
            store
                .join_requests
                .remove(id)
                .ok_or(NetworkError::InvalidRequest)?;
            store.devices.insert(
                device_id.clone(),
                Device {
                    device_id,
                    label,
                    groups,
                    revoked: false,
                    deleted_at_ms: None,
                    stopped: false,
                    admission_revision: store.revision.checked_add(1).ok_or(NetworkError::Full)?,
                    certificate_pem: certificate,
                    certificate_sha256: fingerprint.clone(),
                    expires_at_ms: now.saturating_add(CERT_MS),
                    certificate_ip: request.observed_ip,
                    renewal: None,
                    join_approval: Some(JoinApproval {
                        request,
                        certificate_sha256: fingerprint,
                    }),
                },
            );
            Ok(())
        })
    }

    pub(super) fn pending_joins(&self, now: u64) -> serde_json::Value {
        self.store.join_requests.values().map(|r| serde_json::json!({
            "request_id":r.request_id,"label":r.label,"observed_ip":r.observed_ip,"key_sha256":r.key_sha256,
            "created_at_ms":r.created_at_ms,"expires_at_ms":r.expires_at_ms,
            "status":if r.expires_at_ms>now {"pending"} else {"expired"}
            ,"name_conflict":Self::label_conflicts(&self.store, &r.label, None, Some(&r.request_id), now)
        })).collect()
    }
}
