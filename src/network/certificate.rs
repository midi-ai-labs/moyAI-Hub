//! A single TLS identity owner swaps only the acceptor used by new handshakes.
use super::{NetworkError, Result, pki, tls};
use rustls::pki_types::{CertificateDer, pem::PemObject};
use serde::{Deserialize, Serialize};
use std::{
    net::IpAddr,
    sync::{Arc, RwLock},
};
use tokio_rustls::{TlsAcceptor, rustls};

const RENEW_BEFORE_MS: u64 = 7 * 86_400_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CertificateSnapshot {
    pub sha256: String,
    pub expires_at_ms: String,
    pub renew_after_ms: String,
    pub last_success_ms: String,
    pub last_error: Option<String>,
}

pub struct PreparedIdentity {
    acceptor: TlsAcceptor,
    snapshot: CertificateSnapshot,
}

impl PreparedIdentity {
    pub fn new(
        cert: &str,
        key: &str,
        ca: &str,
        ip: IpAddr,
        enrollment: bool,
        now: u64,
    ) -> Result<Self> {
        if [cert.len(), key.len(), ca.len()]
            .into_iter()
            .any(|n| n > 16 * 1024)
        {
            return Err(NetworkError::Unavailable);
        }
        tls::verify_hub_server_chain(cert, ca, ip)?;
        let der = CertificateDer::from_pem_slice(cert.as_bytes())
            .map_err(|_| NetworkError::Unavailable)?;
        let (rest, parsed) = x509_parser::parse_x509_certificate(der.as_ref())
            .map_err(|_| NetworkError::Unavailable)?;
        if !rest.is_empty() {
            return Err(NetworkError::Unavailable);
        }
        let expiry = u64::try_from(parsed.validity().not_after.timestamp())
            .ok()
            .and_then(|n| n.checked_mul(1000))
            .filter(|expiry| *expiry > now)
            .ok_or(NetworkError::Unavailable)?;
        Ok(Self {
            acceptor: tls::acceptor(cert, key, ca, enrollment)?,
            snapshot: CertificateSnapshot {
                sha256: pki::fingerprint(cert)?,
                expires_at_ms: expiry.to_string(),
                renew_after_ms: expiry.saturating_sub(RENEW_BEFORE_MS).to_string(),
                last_success_ms: now.to_string(),
                last_error: None,
            },
        })
    }
    pub fn snapshot(&self) -> &CertificateSnapshot {
        &self.snapshot
    }
}

#[derive(Clone)]
pub struct CertificateIdentity(Arc<RwLock<PreparedIdentity>>);
impl CertificateIdentity {
    pub fn new(identity: PreparedIdentity) -> Self {
        Self(Arc::new(RwLock::new(identity)))
    }
    pub fn snapshot(&self) -> Result<CertificateSnapshot> {
        self.0
            .read()
            .map(|s| s.snapshot.clone())
            .map_err(|_| NetworkError::Unavailable)
    }
    pub fn due(&self, now: u64) -> Result<bool> {
        self.snapshot()?
            .renew_after_ms
            .parse::<u64>()
            .map(|deadline| now >= deadline)
            .map_err(|_| NetworkError::Unavailable)
    }
    pub fn install(&self, prepared: PreparedIdentity) -> Result<()> {
        *self.0.write().map_err(|_| NetworkError::Unavailable)? = prepared;
        Ok(())
    }
    pub fn failed(&self) {
        if let Ok(mut state) = self.0.write() {
            state.snapshot.last_error = Some("certificate_renewal_failed".into());
        }
    }
    pub(super) fn acceptor(&self) -> Result<TlsAcceptor> {
        self.0
            .read()
            .map(|s| s.acceptor.clone())
            .map_err(|_| NetworkError::Unavailable)
    }
}
