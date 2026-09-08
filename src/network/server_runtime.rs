//! One runtime owner for the Hub/Gateway server identities and their renewal operation.
use super::{
    NetworkError, Result, SharedNetwork,
    certificate::{CertificateIdentity, CertificateSnapshot, PreparedIdentity},
};
use crate::{gateway::GatewayTlsUpdate, gateway_process::GatewayControl};
use serde::Serialize;
use std::{net::IpAddr, sync::Arc};
use tokio::sync::Mutex;

#[derive(Clone, Serialize)]
pub struct NetworkDiagnostics {
    pub hub_id: String,
    pub metadata_endpoint: String,
    pub gateway_endpoint: Option<String>,
    pub gateway_ready: bool,
    pub certificate: CertificateSnapshot,
    pub gateway_certificate: Option<CertificateSnapshot>,
}

pub struct NetworkServerRuntime {
    network: SharedNetwork,
    identity: CertificateIdentity,
    gateway: GatewayControl,
    metadata_endpoint: String,
    public_ip: IpAddr,
    operation: Mutex<()>,
}
impl NetworkServerRuntime {
    pub fn new(
        network: SharedNetwork,
        identity: CertificateIdentity,
        gateway: GatewayControl,
        metadata_endpoint: String,
        public_ip: IpAddr,
    ) -> Arc<Self> {
        Arc::new(Self {
            network,
            identity,
            gateway,
            metadata_endpoint,
            public_ip,
            operation: Mutex::new(()),
        })
    }
    pub fn certificate(&self) -> Result<CertificateSnapshot> {
        self.identity.snapshot()
    }
    pub fn gateway_endpoint(&self) -> Option<&str> {
        self.gateway.network_base_url()
    }

    pub async fn renew(&self, force: bool, now: u64) -> Result<bool> {
        let _operation = self.operation.lock().await;
        if !force && !self.identity.due(now)? {
            return Ok(false);
        }
        let result = self.renew_locked(now).await;
        if result.is_err() {
            self.identity.failed();
        }
        result
    }
    async fn renew_locked(&self, now: u64) -> Result<bool> {
        let (certificate_pem, private_key_pem, ca) = {
            let network = self.network.lock().map_err(|_| NetworkError::Unavailable)?;
            let (cert, key) = network.server_certificate(self.public_ip, now)?;
            (cert, key, network.ca_certificate().to_owned())
        };
        // Validate the Hub side before changing either listener. The child verifies
        // its fixed CA/IP/role and acknowledges installation before the Hub switches.
        let prepared = PreparedIdentity::new(
            &certificate_pem,
            &private_key_pem,
            &ca,
            self.public_ip,
            true,
            now,
        )?;
        let receipt = self
            .gateway
            .install_certificate(&GatewayTlsUpdate {
                certificate_pem,
                private_key_pem,
            })
            .await
            .map_err(|_| NetworkError::Unavailable)?;
        if receipt.sha256 != prepared.snapshot().sha256
            || receipt.expires_at_ms != prepared.snapshot().expires_at_ms
        {
            return Err(NetworkError::Unavailable);
        }
        self.identity.install(prepared)?;
        Ok(true)
    }

    pub async fn diagnostics(&self) -> Result<NetworkDiagnostics> {
        let _operation = self.operation.lock().await;
        let hub_id = self
            .network
            .lock()
            .map_err(|_| NetworkError::Unavailable)?
            .hub_id()
            .to_owned();
        let certificate = self.identity.snapshot()?;
        let gateway_certificate = self.gateway.certificate().await.ok();
        let gateway_ready = gateway_certificate.is_some()
            && self
                .gateway
                .status()
                .await
                .ok()
                .and_then(|status| status.get("accepting").and_then(serde_json::Value::as_bool))
                .unwrap_or(false);
        Ok(NetworkDiagnostics {
            hub_id,
            metadata_endpoint: self.metadata_endpoint.clone(),
            gateway_endpoint: self.gateway.network_base_url().map(str::to_owned),
            gateway_ready,
            certificate,
            gateway_certificate,
        })
    }
}
