//! Bounded TLS listener; peer identity is derived from the verified handshake, never HTTP headers.
use super::{NetworkError, Result};
use axum::extract::connect_info::{ConnectInfo, Connected};
use futures_util::{StreamExt, stream::FuturesUnordered};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    io,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::{OwnedSemaphorePermit, Semaphore},
    time::{Instant, Sleep},
};
use tokio_rustls::{TlsAcceptor, rustls, server::TlsStream};

pub fn acceptor(cert: &str, key: &str, ca: &str, enrollment: bool) -> Result<TlsAcceptor> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let certs = CertificateDer::pem_slice_iter(cert.as_bytes())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| NetworkError::Unavailable)?;
    let key =
        PrivateKeyDer::from_pem_slice(key.as_bytes()).map_err(|_| NetworkError::Unavailable)?;
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_slice(ca.as_bytes()).map_err(|_| NetworkError::Unavailable)?)
        .map_err(|_| NetworkError::Unavailable)?;
    let verifier = rustls::server::WebPkiClientVerifier::builder(Arc::new(roots));
    let verifier = if enrollment {
        verifier.allow_unauthenticated()
    } else {
        verifier
    }
    .build()
    .map_err(|_| NetworkError::Unavailable)?;
    let mut config = rustls::ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(certs, key)
        .map_err(|_| NetworkError::Unavailable)?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(TlsAcceptor::from(Arc::new(config)))
}

pub fn verify_server_chain(cert: &str, ca: &str, ip: std::net::IpAddr) -> Result<()> {
    use rustls::client::danger::ServerCertVerifier;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_slice(ca.as_bytes()).map_err(|_| NetworkError::Unavailable)?)
        .map_err(|_| NetworkError::Unavailable)?;
    let verifier = rustls::client::WebPkiServerVerifier::builder(Arc::new(roots))
        .build()
        .map_err(|_| NetworkError::Unavailable)?;
    let certificate =
        CertificateDer::from_pem_slice(cert.as_bytes()).map_err(|_| NetworkError::Unavailable)?;
    verifier
        .verify_server_cert(
            &certificate,
            &[],
            &rustls::pki_types::ServerName::IpAddress(ip.into()),
            &[],
            rustls::pki_types::UnixTime::now(),
        )
        .map_err(|_| NetworkError::Unavailable)?;
    Ok(())
}

#[derive(Clone, Debug)]
pub struct TlsPeer {
    pub address: SocketAddr,
    pub certificate_sha256: Option<String>,
    pub authority: String,
}
impl Connected<axum::serve::IncomingStream<'_, NetworkListener>> for TlsPeer {
    fn connect_info(stream: axum::serve::IncomingStream<'_, NetworkListener>) -> Self {
        stream.remote_addr().clone()
    }
}
type Handshake = Pin<Box<dyn Future<Output = io::Result<(BoundedIo, TlsPeer)>> + Send>>;
pub struct NetworkListener {
    listener: TcpListener,
    acceptor: TlsAcceptor,
    slots: Arc<Semaphore>,
    pending: FuturesUnordered<Handshake>,
    idle: Duration,
    authority: String,
}
impl NetworkListener {
    pub fn new(listener: TcpListener, acceptor: TlsAcceptor) -> Self {
        let authority = listener
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_default();
        Self {
            listener,
            acceptor,
            slots: Arc::new(Semaphore::new(128)),
            pending: FuturesUnordered::new(),
            idle: Duration::from_secs(30),
            authority,
        }
    }
    pub fn with_idle(mut self, idle: Duration) -> Self {
        self.idle = idle;
        self
    }
    pub fn with_authority(mut self, authority: String) -> Self {
        self.authority = authority;
        self
    }
}
impl axum::serve::Listener for NetworkListener {
    type Io = BoundedIo;
    type Addr = TlsPeer;
    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            tokio::select! {
                done=self.pending.next(),if !self.pending.is_empty()=>{if let Some(Ok(pair))=done{return pair;}},
                incoming=self.listener.accept()=>match incoming {
                    Ok((stream,address))=>if let Ok(slot)=self.slots.clone().try_acquire_owned() {
                        let acceptor=self.acceptor.clone();
                        let idle=self.idle;
                        let authority=self.authority.clone();
                        self.pending.push(Box::pin(async move {
                            let stream=tokio::time::timeout(Duration::from_secs(5),acceptor.accept(stream)).await.map_err(|_|io::ErrorKind::TimedOut)??;
                            let certificate_sha256=stream.get_ref().1.peer_certificates().and_then(|chain|chain.first()).map(|cert|format!("{:x}",Sha256::digest(cert.as_ref())));
                            Ok((BoundedIo{stream,_slot:slot,idle:Box::pin(tokio::time::sleep(idle)),idle_duration:idle},TlsPeer{address,certificate_sha256,authority}))
                        }));
                    },
                    Err(_)=>tokio::time::sleep(Duration::from_millis(50)).await,
                }
            }
        }
    }
    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(TlsPeer {
            address: self.listener.local_addr()?,
            certificate_sha256: None,
            authority: self.authority.clone(),
        })
    }
}
pub struct BoundedIo {
    stream: TlsStream<TcpStream>,
    _slot: OwnedSemaphorePermit,
    idle: Pin<Box<Sleep>>,
    idle_duration: Duration,
}
impl BoundedIo {
    fn timeout(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
        if self.idle.as_mut().poll(cx).is_ready() {
            Err(io::ErrorKind::TimedOut.into())
        } else {
            Ok(())
        }
    }
    fn progress(&mut self) {
        self.idle
            .as_mut()
            .reset(Instant::now() + self.idle_duration);
    }
}
impl AsyncRead for BoundedIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Err(e) = this.timeout(cx) {
            return Poll::Ready(Err(e));
        }
        let before = buf.filled().len();
        let result = Pin::new(&mut this.stream).poll_read(cx, buf);
        if buf.filled().len() > before {
            this.progress();
        }
        result
    }
}
impl AsyncWrite for BoundedIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if let Err(e) = this.timeout(cx) {
            return Poll::Ready(Err(e));
        }
        let result = Pin::new(&mut this.stream).poll_write(cx, buf);
        if matches!(result,Poll::Ready(Ok(n))if n>0) {
            this.progress();
        }
        result
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}

#[derive(Clone)]
pub struct DeviceIdentity {
    pub device_id: String,
    pub certificate_sha256: String,
    pub address: SocketAddr,
}
pub fn authority_matches(headers: &axum::http::HeaderMap, authority: &str) -> bool {
    let actual = headers.get("host").and_then(|h| h.to_str().ok());
    actual == Some(authority)
        || authority
            .strip_suffix(":443")
            .is_some_and(|host| actual == Some(host))
}
pub async fn guard(
    axum::extract::State(network): axum::extract::State<super::SharedNetwork>,
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let fail = |error| super::api::error(error).into_response();
    if request.headers().contains_key("origin") {
        return fail(NetworkError::Unauthorized);
    }
    let Some(ConnectInfo(peer)) = request.extensions().get::<ConnectInfo<TlsPeer>>().cloned()
    else {
        return fail(NetworkError::Unauthorized);
    };
    if !authority_matches(request.headers(), &peer.authority) {
        return fail(NetworkError::Unauthorized);
    }
    request.headers_mut().remove("x-moyai-verified-device");
    request.headers_mut().remove("x-moyai-verified-cert");
    request.extensions_mut().insert(ConnectInfo(peer.address));
    let path = request.uri().path();
    if path != "/v1/network/enroll" && path != "/v1/network/renew" {
        let Some(cert) = peer.certificate_sha256.clone() else {
            return fail(NetworkError::Unauthorized);
        };
        let result = network
            .lock()
            .map_err(|_| NetworkError::Unavailable)
            .and_then(|n| n.authenticate(&cert, crate::service::now_ms()));
        let id = match result {
            Ok(id) => id,
            Err(e) => return fail(e),
        };
        request
            .headers_mut()
            .insert("x-moyai-verified-device", id.parse().expect("generated ID"));
        request.headers_mut().insert(
            "x-moyai-verified-cert",
            cert.parse().expect("hex certificate fingerprint"),
        );
        request.extensions_mut().insert(DeviceIdentity {
            device_id: id,
            certificate_sha256: cert,
            address: peer.address,
        });
    }
    let mut response = next.run(request).await;
    if response.status().is_client_error()
        && !response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("application/json"))
    {
        response = (
            response.status(),
            axum::Json(serde_json::json!({"error":"invalid_request"})),
        )
            .into_response();
    }
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().expect("static header"));
    response
}
