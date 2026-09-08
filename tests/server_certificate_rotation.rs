use moyai_hub::{
    gateway::{Gateway, GatewayTlsUpdate},
    gateway_process::GatewayControl,
    network::{
        DeviceNetwork,
        certificate::{CertificateIdentity, PreparedIdentity},
        server_runtime::NetworkServerRuntime,
    },
    service::now_ms,
};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    net::IpAddr,
    sync::{Arc, Mutex},
};

async fn serve(router: axum::Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (base, task)
}

#[tokio::test]
async fn gateway_certificate_control_is_private_authenticated_and_preserves_old_identity_on_rejection()
 {
    let directory = tempfile::tempdir().unwrap();
    let mut network =
        DeviceNetwork::open(&directory.path().join("network.json"), "hub-rotation-test").unwrap();
    let ip: IpAddr = "127.0.0.1".parse().unwrap();
    let now = now_ms();
    let ca = network.ca_certificate().to_owned();
    let (cert, key) = network.server_certificate(ip, now).unwrap();
    let identity =
        CertificateIdentity::new(PreparedIdentity::new(&cert, &key, &ca, ip, false, now).unwrap());
    let original = identity.snapshot().unwrap().sha256;
    let token = "test-control-token-for-server-rotation";
    let gateway = Gateway::new("http://127.0.0.1:9".into(), token.into()).unwrap();
    gateway
        .configure_network_tls(identity.clone(), ca.clone(), ip)
        .unwrap();
    let (private, private_task) = serve(gateway.clone().router()).await;
    let (public, public_task) = serve(gateway.clone().network_router()).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let (next_cert, next_key) = network.server_certificate(ip, now).unwrap();
    let update = GatewayTlsUpdate {
        certificate_pem: next_cert,
        private_key_pem: next_key,
    };
    assert_eq!(
        client
            .get(format!("{private}/control/tls"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client
            .post(format!("{private}/control/tls"))
            .json(&update)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client
            .post(format!("{private}/control/tls"))
            .bearer_auth(token)
            .header("origin", "https://untrusted.invalid")
            .json(&update)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client
            .post(format!("{public}/control/tls"))
            .bearer_auth(token)
            .json(&update)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );

    let device_key = rcgen::KeyPair::generate().unwrap();
    let csr = rcgen::CertificateParams::new(vec![ip.to_string()])
        .unwrap()
        .serialize_request(&device_key)
        .unwrap()
        .pem()
        .unwrap();
    let invite = network
        .invite(
            &network.revision(),
            "same-PC-device".into(),
            BTreeSet::new(),
            now,
        )
        .unwrap();
    let device = network
        .enroll(invite["code"].as_str().unwrap(), &csr, ip, now)
        .unwrap();
    let (wrong_ip_cert, wrong_ip_key) = network
        .server_certificate("127.0.0.2".parse().unwrap(), now)
        .unwrap();
    for rejected in [
        GatewayTlsUpdate {
            certificate_pem: device.certificate_pem,
            private_key_pem: device_key.serialize_pem(),
        },
        GatewayTlsUpdate {
            certificate_pem: update.certificate_pem.clone(),
            private_key_pem: key,
        },
        GatewayTlsUpdate {
            certificate_pem: wrong_ip_cert,
            private_key_pem: wrong_ip_key,
        },
    ] {
        assert_eq!(
            client
                .post(format!("{private}/control/tls"))
                .bearer_auth(token)
                .json(&rejected)
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
        assert_eq!(identity.snapshot().unwrap().sha256, original);
    }
    let before = gateway.status();
    let response = client
        .post(format!("{private}/control/tls"))
        .bearer_auth(token)
        .json(&update)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let value: Value = response.json().await.unwrap();
    assert_ne!(value["sha256"].as_str().unwrap(), original);
    assert_eq!(
        value["sha256"].as_str().unwrap(),
        identity.snapshot().unwrap().sha256
    );
    assert_eq!(
        gateway.status(),
        before,
        "TLS renewal must not alter allocation/stream accounting"
    );
    assert!(!value.to_string().contains("PRIVATE KEY"));
    assert!(value.get("certificate_pem").is_none());
    private_task.abort();
    public_task.abort();
    let _ = private_task.await;
    let _ = public_task.await;
}

#[tokio::test]
async fn hub_renewal_waits_for_gateway_ack_and_keeps_its_valid_identity_when_gateway_is_unavailable()
 {
    let directory = tempfile::tempdir().unwrap();
    let network = Arc::new(Mutex::new(
        DeviceNetwork::open(&directory.path().join("network.json"), "hub-renewal-test").unwrap(),
    ));
    let now = now_ms();
    let ip: IpAddr = "127.0.0.1".parse().unwrap();
    let (cert, key, ca) = {
        let owner = network.lock().unwrap();
        let (cert, key) = owner.server_certificate(ip, now).unwrap();
        (cert, key, owner.ca_certificate().to_owned())
    };
    let hub_identity =
        CertificateIdentity::new(PreparedIdentity::new(&cert, &key, &ca, ip, true, now).unwrap());
    let gateway_identity =
        CertificateIdentity::new(PreparedIdentity::new(&cert, &key, &ca, ip, false, now).unwrap());
    let token = "test-control-token-for-parent-renewal";
    let gateway = Gateway::new("http://127.0.0.1:9".into(), token.into()).unwrap();
    gateway
        .configure_network_tls(gateway_identity.clone(), ca, ip)
        .unwrap();
    let (private, task) = serve(gateway.router()).await;
    let control = GatewayControl::for_loopback(private, token.into())
        .unwrap()
        .with_network_base_url("https://127.0.0.1:7443".into())
        .unwrap();
    let runtime = NetworkServerRuntime::new(
        network,
        hub_identity.clone(),
        control,
        "https://127.0.0.1:9471".into(),
        ip,
    );
    assert!(!runtime.renew(false, now).await.unwrap());
    let first = hub_identity.snapshot().unwrap().sha256;
    assert!(runtime.renew(true, now).await.unwrap());
    let updated = hub_identity.snapshot().unwrap().sha256;
    assert_ne!(updated, first);
    assert_eq!(gateway_identity.snapshot().unwrap().sha256, updated);
    let diagnostic = runtime.diagnostics().await.unwrap();
    assert!(diagnostic.gateway_ready);
    assert_eq!(
        diagnostic.certificate.sha256,
        diagnostic.gateway_certificate.unwrap().sha256
    );
    task.abort();
    let _ = task.await;
    assert!(runtime.renew(true, now).await.is_err());
    let retained = hub_identity.snapshot().unwrap();
    assert_eq!(retained.sha256, updated);
    assert_eq!(
        retained.last_error.as_deref(),
        Some("certificate_renewal_failed")
    );
}

#[tokio::test]
async fn gateway_rotation_changes_new_handshakes_without_interrupting_an_existing_tls_response() {
    use axum::{Router, extract::State, routing::get};
    use moyai_hub::network::tls::{self, NetworkListener};
    use sha2::{Digest, Sha256};
    use std::time::Duration;
    use tokio::sync::Notify;

    struct ResponseBarrier {
        entered: Notify,
        release: Notify,
    }
    async fn held_response(State(barrier): State<Arc<ResponseBarrier>>) -> &'static str {
        barrier.entered.notify_one();
        barrier.release.notified().await;
        "old TLS request completed"
    }
    let directory = tempfile::tempdir().unwrap();
    let mut network =
        DeviceNetwork::open(&directory.path().join("network.json"), "live-rotation").unwrap();
    let now = now_ms();
    let ip = "127.0.0.1".parse().unwrap();
    let ca = network.ca_certificate().to_owned();
    let (cert, key) = network.server_certificate(ip, now).unwrap();
    let identity =
        CertificateIdentity::new(PreparedIdentity::new(&cert, &key, &ca, ip, false, now).unwrap());
    let first_fingerprint = identity.snapshot().unwrap().sha256;
    let gateway = Gateway::new("http://127.0.0.1:9".into(), "rotation-control".into()).unwrap();
    gateway
        .configure_network_tls(identity.clone(), ca.clone(), ip)
        .unwrap();
    let (control_url, control_task) = serve(gateway.router()).await;

    let device_key = rcgen::KeyPair::generate().unwrap();
    let csr = rcgen::CertificateParams::new(vec![ip.to_string()])
        .unwrap()
        .serialize_request(&device_key)
        .unwrap()
        .pem()
        .unwrap();
    let invite = network
        .invite(&network.revision(), "receiver".into(), BTreeSet::new(), now)
        .unwrap();
    let device = network
        .enroll(invite["code"].as_str().unwrap(), &csr, ip, now)
        .unwrap();
    let client_pem = format!("{}\n{}", device.certificate_pem, device_key.serialize_pem());
    let new_client = || {
        reqwest::Client::builder()
            .no_proxy()
            .add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap())
            .identity(reqwest::Identity::from_pem(client_pem.as_bytes()).unwrap())
            .tls_info(true)
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap()
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("https://{}", listener.local_addr().unwrap());
    let listener = NetworkListener::new(listener, tls::acceptor(&cert, &key, &ca, false).unwrap())
        .with_identity(identity.clone());
    let barrier = Arc::new(ResponseBarrier {
        entered: Notify::new(),
        release: Notify::new(),
    });
    let router = Router::new()
        .route("/held", get(held_response))
        .route("/ready", get(|| async { "new TLS request completed" }))
        .with_state(barrier.clone());
    let tls_task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let old_client = new_client();
    let old_url = format!("{base}/held");
    let old_request = tokio::spawn(async move { old_client.get(old_url).send().await.unwrap() });
    tokio::time::timeout(Duration::from_secs(5), barrier.entered.notified())
        .await
        .unwrap();

    let (next_cert, next_key) = network.server_certificate(ip, now_ms()).unwrap();
    let receipt = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("{control_url}/control/tls"))
        .bearer_auth("rotation-control")
        .json(&GatewayTlsUpdate {
            certificate_pem: next_cert,
            private_key_pem: next_key,
        })
        .send()
        .await
        .unwrap();
    assert!(receipt.status().is_success());
    let new_response = new_client()
        .get(format!("{base}/ready"))
        .send()
        .await
        .unwrap();
    let peer = new_response
        .extensions()
        .get::<reqwest::tls::TlsInfo>()
        .unwrap()
        .peer_certificate()
        .unwrap();
    let fingerprint = format!("{:x}", Sha256::digest(peer));
    assert_ne!(fingerprint, first_fingerprint);
    assert_eq!(fingerprint, identity.snapshot().unwrap().sha256);
    assert_eq!(
        new_response.text().await.unwrap(),
        "new TLS request completed"
    );
    barrier.release.notify_one();
    let old_response = tokio::time::timeout(Duration::from_secs(5), old_request)
        .await
        .unwrap()
        .unwrap();
    let old_peer = old_response
        .extensions()
        .get::<reqwest::tls::TlsInfo>()
        .unwrap()
        .peer_certificate()
        .unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(old_peer)), first_fingerprint);
    assert_eq!(
        old_response.text().await.unwrap(),
        "old TLS request completed"
    );
    tls_task.abort();
    control_task.abort();
    let _ = tls_task.await;
    let _ = control_task.await;
}
