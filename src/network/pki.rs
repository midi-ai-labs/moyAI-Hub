//! Hub-only CA key owner. PKCS#10 signatures prove possession before issuance.
use super::{NetworkError, Result};
use rcgen::{
    BasicConstraints, CertificateParams, CertificateSigningRequestParams, DistinguishedName,
    DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose, SanType,
};
use sha2::{Digest, Sha256};
use std::net::IpAddr;
use time::{Duration, OffsetDateTime};
use tokio_rustls::rustls::pki_types::{CertificateDer, pem::PemObject};

pub fn fingerprint(pem: &str) -> Result<String> {
    let der =
        CertificateDer::from_pem_slice(pem.as_bytes()).map_err(|_| NetworkError::InvalidCsr)?;
    Ok(format!("{:x}", Sha256::digest(der.as_ref())))
}

pub fn create_ca(hub_id: &str) -> Result<(String, String)> {
    let key = KeyPair::generate().map_err(|_| NetworkError::Unavailable)?;
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::CommonName, format!("moyAI Hub {hub_id}"));
    params.not_before = OffsetDateTime::now_utc() - Duration::minutes(5);
    params.not_after = OffsetDateTime::now_utc() + Duration::days(3650);
    let cert = params
        .self_signed(&key)
        .map_err(|_| NetworkError::Unavailable)?;
    Ok((cert.pem(), key.serialize_pem()))
}

fn issuer(ca: &str, key: &str) -> Result<Issuer<'static, KeyPair>> {
    let key = KeyPair::from_pem(key).map_err(|_| NetworkError::Unavailable)?;
    Issuer::from_ca_cert_pem(ca, key).map_err(|_| NetworkError::Unavailable)
}

fn leaf_params(id: &str, ip: IpAddr, now_ms: u64) -> Result<CertificateParams> {
    if ip.is_unspecified() || ip.is_multicast() {
        return Err(NetworkError::InvalidCsr);
    }
    let now = OffsetDateTime::from_unix_timestamp((now_ms / 1000) as i64)
        .map_err(|_| NetworkError::InvalidRequest)?;
    let mut params =
        CertificateParams::new(vec![ip.to_string()]).map_err(|_| NetworkError::InvalidCsr)?;
    params.distinguished_name = DistinguishedName::new();
    params.distinguished_name.push(DnType::CommonName, id);
    params.not_before = now - Duration::minutes(5);
    params.not_after = now + Duration::days(30);
    params.serial_number = Some(rcgen::SerialNumber::from_slice(
        &ulid::Ulid::new().to_bytes(),
    ));
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![
        ExtendedKeyUsagePurpose::ClientAuth,
        ExtendedKeyUsagePurpose::ServerAuth,
    ];
    Ok(params)
}

pub fn sign_device(
    ca: &str,
    key: &str,
    id: &str,
    csr: &str,
    ip: IpAddr,
    now: u64,
) -> Result<String> {
    if csr.len() > 16 * 1024 {
        return Err(NetworkError::InvalidCsr);
    }
    let mut request =
        CertificateSigningRequestParams::from_pem(csr).map_err(|_| NetworkError::InvalidCsr)?;
    if request.params.subject_alt_names != vec![SanType::IpAddress(ip)] {
        return Err(NetworkError::InvalidCsr);
    }
    // Never inherit requested CA privileges, identity, usage, lifetime or extensions.
    request.params = leaf_params(id, ip, now)?;
    request
        .signed_by(&issuer(ca, key)?)
        .map(|cert| cert.pem())
        .map_err(|_| NetworkError::InvalidCsr)
}

pub fn server_certificate(
    ca: &str,
    key: &str,
    hub_id: &str,
    ip: IpAddr,
    now: u64,
) -> Result<(String, String)> {
    let server_key = KeyPair::generate().map_err(|_| NetworkError::Unavailable)?;
    let mut params = leaf_params(hub_id, ip, now)?;
    // Clients require both the actual IP and this Hub-only role. Device CSRs
    // cannot acquire it because sign_device accepts exactly one observed IP SAN.
    params.subject_alt_names.push(SanType::DnsName(
        "hub.moyai.invalid"
            .try_into()
            .map_err(|_| NetworkError::Unavailable)?,
    ));
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let cert = params
        .signed_by(&server_key, &issuer(ca, key)?)
        .map_err(|_| NetworkError::Unavailable)?;
    Ok((cert.pem(), server_key.serialize_pem()))
}
