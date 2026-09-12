//! Public one-click CLI contract. Every host uses a disposable task profile and
//! --no-browser; these tests never open or control the user's browser.
use moyai_hub::host_process::HostClient;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    process::{Output, Stdio},
    time::{Duration, Instant},
};
use tokio::net::{TcpListener, TcpStream};

#[cfg(windows)]
fn listener_handle_flags(listener: &TcpListener) -> u32 {
    use std::{ffi::c_void, os::windows::io::AsRawSocket};
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetHandleInformation(handle: *mut c_void, flags: *mut u32) -> i32;
    }
    let mut flags = 0;
    assert_ne!(
        unsafe { GetHandleInformation(listener.as_raw_socket() as *mut c_void, &mut flags) },
        0,
        "fixture socket handle could not be inspected: {}",
        std::io::Error::last_os_error()
    );
    flags
}

fn bind_fixture_listener(address: impl std::net::ToSocketAddrs) -> std::io::Result<TcpListener> {
    // std creates non-inheritable Windows sockets. The fixture's occupied port
    // must not travel through the short-lived CLI into the detached server,
    // where it would remain bound even after the test drops its own listener.
    let listener = std::net::TcpListener::bind(address)?;
    listener.set_nonblocking(true)?;
    let listener = TcpListener::from_std(listener)?;
    #[cfg(windows)]
    assert_eq!(
        listener_handle_flags(&listener) & 1,
        0,
        "fixture listener must not be inherited by public CLI children"
    );
    Ok(listener)
}

fn executable() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_moyai-hub"))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchReply {
    pid: u32,
    web_url: String,
    network_running: bool,
    needs_setup: bool,
    network_error: Option<String>,
}

struct Profile {
    directory: tempfile::TempDir,
}

impl Profile {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("project_sandbox/hub-launch-cli-tests");
        std::fs::create_dir_all(&parent).unwrap();
        Self {
            directory: tempfile::Builder::new()
                .prefix("launch-")
                .tempdir_in(parent)
                .unwrap(),
        }
    }

    fn catalog(&self) -> PathBuf {
        self.directory.path().join("catalog.json")
    }

    fn hosting_path(&self) -> PathBuf {
        self.directory.path().join("network-hosting.json")
    }

    async fn command(&self, args: &[&str]) -> Output {
        let mut command = tokio::process::Command::new(executable());
        command
            .args(args)
            .arg("--data-dir")
            .arg(self.directory.path())
            .stdin(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        tokio::time::timeout(Duration::from_secs(25), command.output())
            .await
            .expect("public CLI did not terminate")
            .expect("public CLI could not be started")
    }

    async fn launch(&self, first: bool, success: bool) -> LaunchReply {
        let args = if first {
            vec!["--launch", "--no-browser", "--web-port", "0"]
        } else {
            vec!["--launch", "--no-browser"]
        };
        let output = self.command(&args).await;
        assert_eq!(
            output.status.code(),
            Some(if success { 0 } else { 1 }),
            "launch stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("launch must print one structured result")
    }

    async fn connect(&self) -> HostClient {
        HostClient::connect(&self.catalog())
            .await
            .expect("successful launch must leave a reachable authenticated host")
    }

    async fn stop(&self, client: &HostClient, endpoints: &[SocketAddr]) {
        let output = self.command(&["--stop"]).await;
        assert!(output.status.success());
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let mut closed = !self.directory.path().join("host-control.json").exists();
                for endpoint in endpoints {
                    if TcpStream::connect(endpoint).await.is_ok() {
                        closed = false;
                    }
                }
                if closed {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .expect("owned host/Web/network/Gateway endpoints must close after --stop");
        assert!(client.dispatch("hub_snapshot", json!({})).await.is_err());
        assert!(HostClient::connect(&self.catalog()).await.is_err());
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        // Match the existing host-process test cleanup: authenticated control for
        // this exact disposable profile only, never a PID/name-based host kill.
        if !self.directory.path().join("host-control.json").exists() {
            return;
        }
        let mut command = std::process::Command::new(executable());
        command
            .arg("--stop")
            .arg("--data-dir")
            .arg(self.directory.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        if let Ok(mut child) = command.spawn() {
            let deadline = Instant::now() + Duration::from_secs(8);
            while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

fn browser() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

async fn assert_web(client: &HostClient, reply: &LaunchReply) -> SocketAddr {
    assert_eq!(reply.pid, client.pid());
    let web = client.dispatch("hub_web_status", json!({})).await.unwrap();
    assert_eq!(web["enabled"], true);
    assert_eq!(web["local_url"], reply.web_url);
    let url = reqwest::Url::parse(&reply.web_url).unwrap();
    assert_eq!(url.scheme(), "http");
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert_eq!(url.path(), "/admin/");
    assert!(url.username().is_empty());
    assert!(url.password().is_none() && url.query().is_none() && url.fragment().is_none());
    let address = SocketAddr::from(([127, 0, 0, 1], url.port().unwrap()));
    let response = browser().get(url).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert!(!response.headers().contains_key("set-cookie"));
    assert!(response.text().await.unwrap().contains("moyAI Hub"));
    address
}

async fn configure_network(client: &HostClient) -> (SocketAddr, SocketAddr) {
    // Simultaneous reservations prevent selecting the same port for two owners.
    let network_probe = bind_fixture_listener("127.0.0.1:0").unwrap();
    let gateway_probe = bind_fixture_listener("127.0.0.1:0").unwrap();
    let network = network_probe.local_addr().unwrap();
    let gateway = gateway_probe.local_addr().unwrap();
    let snapshot = client
        .dispatch("hub_network_snapshot", json!({}))
        .await
        .unwrap();
    client
        .dispatch(
            "hub_network_settings",
            json!({"expectedRevision":snapshot["revision"],"gatewayPort":gateway.port()}),
        )
        .await
        .unwrap();
    drop(network_probe);
    drop(gateway_probe);
    client
        .dispatch(
            "hub_network_start",
            json!({"bind":network.to_string(),"advertiseIp":"127.0.0.1"}),
        )
        .await
        .unwrap();
    (network, gateway)
}

async fn assert_network(client: &HostClient, network: SocketAddr, gateway: SocketAddr) -> Value {
    let snapshot = client
        .dispatch("hub_network_snapshot", json!({}))
        .await
        .unwrap();
    assert_eq!(snapshot["server"]["running"], true);
    assert_eq!(snapshot["server"]["bind"], format!("https://{network}"));
    assert_eq!(
        snapshot["server"]["gateway_bind"],
        format!("https://{gateway}")
    );
    let model = client.dispatch("hub_snapshot", json!({})).await.unwrap();
    assert_eq!(model["server"]["running"], true);
    assert_eq!(model["gateway"]["ready"], true);
    let tls = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .add_root_certificate(
            reqwest::Certificate::from_pem(
                snapshot["ca_certificate_pem"].as_str().unwrap().as_bytes(),
            )
            .unwrap(),
        )
        .build()
        .unwrap();
    // Anonymous 401 proves real TLS and device API readiness without enrollment.
    assert_eq!(
        tls.get(format!("https://{network}/v1/network/self"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert!(TcpStream::connect(gateway).await.is_ok());
    snapshot
}

#[tokio::test]
async fn launch_first_profile_reports_setup_and_keeps_web_running() {
    let profile = Profile::new();
    let reply = profile.launch(true, true).await;
    assert!(reply.needs_setup);
    assert!(!reply.network_running);
    assert!(reply.network_error.is_none());
    assert!(!profile.hosting_path().exists());
    let client = profile.connect().await;
    let web = assert_web(&client, &reply).await;
    assert_eq!(
        client
            .dispatch("hub_network_snapshot", json!({}))
            .await
            .unwrap()["server"]["running"],
        false
    );
    profile.stop(&client, &[web]).await;
}

#[tokio::test]
async fn launch_saved_network_restores_gateway_and_serializes_duplicate_callers() {
    let profile = Profile::new();
    let first = profile.launch(true, true).await;
    let client = profile.connect().await;
    let web = assert_web(&client, &first).await;
    let (network, gateway) = configure_network(&client).await;
    let before = assert_network(&client, network, gateway).await;
    let saved = std::fs::read(profile.hosting_path()).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&saved).unwrap(),
        json!({"schema_version":1,"bind":network.to_string(),"advertise_ip":"127.0.0.1"})
    );
    let old_instance = client.instance_id().to_owned();
    profile.stop(&client, &[web, network, gateway]).await;

    // Two independent public CLI callers race from a stopped saved profile.
    let (left, right) = tokio::join!(profile.launch(false, true), profile.launch(false, true));
    assert_eq!(left.pid, right.pid);
    assert_eq!(left.web_url, right.web_url);
    for reply in [&left, &right] {
        assert!(reply.network_running);
        assert!(!reply.needs_setup);
        assert!(reply.network_error.is_none());
    }
    let restored = profile.connect().await;
    assert_ne!(restored.instance_id(), old_instance);
    let web = assert_web(&restored, &left).await;
    let after = assert_network(&restored, network, gateway).await;
    assert_eq!(after["hub_id"], before["hub_id"]);
    assert_eq!(after["ca_certificate_pem"], before["ca_certificate_pem"]);
    assert_eq!(after["gateway_port"], before["gateway_port"]);
    assert_eq!(std::fs::read(profile.hosting_path()).unwrap(), saved);

    let duplicate = profile.launch(false, true).await;
    let same = profile.connect().await;
    assert_eq!(same.instance_id(), restored.instance_id());
    assert_eq!(duplicate.pid, left.pid);
    assert_eq!(duplicate.web_url, left.web_url);
    assert!(duplicate.network_running && !duplicate.needs_setup);
    assert!(duplicate.network_error.is_none());
    let repeated = assert_network(&same, network, gateway).await;
    assert_eq!(repeated["revision"], after["revision"]);
    assert_eq!(repeated["server"]["bind"], after["server"]["bind"]);
    assert_eq!(
        repeated["server"]["gateway_bind"],
        after["server"]["gateway_bind"]
    );
    assert_eq!(std::fs::read(profile.hosting_path()).unwrap(), saved);
    profile.stop(&same, &[web, network, gateway]).await;
}

#[tokio::test]
async fn launch_web_port_conflict_fails_without_leaving_a_host() {
    let profile = Profile::new();
    let occupied = bind_fixture_listener("127.0.0.1:0").unwrap();
    let port = occupied.local_addr().unwrap().port().to_string();
    let output = profile
        .command(&["--launch", "--no-browser", "--web-port", &port])
        .await;
    assert_eq!(output.status.code(), Some(1));
    assert!(!output.stderr.is_empty());
    assert!(output.stdout.is_empty());
    assert!(HostClient::connect(&profile.catalog()).await.is_err());
    assert!(!profile.directory.path().join("host-control.json").exists());
    assert!(!profile.hosting_path().exists());
    assert!(
        TcpStream::connect(occupied.local_addr().unwrap())
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn launch_network_port_conflict_keeps_web_and_saved_settings_for_retry() {
    let profile = Profile::new();
    let first = profile.launch(true, true).await;
    let client = profile.connect().await;
    let web = assert_web(&client, &first).await;
    let (network, gateway) = configure_network(&client).await;
    let saved = std::fs::read(profile.hosting_path()).unwrap();
    profile.stop(&client, &[web, network, gateway]).await;

    let occupied = bind_fixture_listener(network).unwrap();
    let failed = profile.launch(false, false).await;
    assert!(!failed.network_running);
    assert!(!failed.needs_setup);
    assert!(
        failed
            .network_error
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
    );
    let retained = profile.connect().await;
    let web = assert_web(&retained, &failed).await;
    assert_eq!(std::fs::read(profile.hosting_path()).unwrap(), saved);
    assert_eq!(
        retained
            .dispatch("hub_network_snapshot", json!({}))
            .await
            .unwrap()["server"]["running"],
        false
    );
    assert_eq!(
        retained.dispatch("hub_snapshot", json!({})).await.unwrap()["gateway"]["ready"],
        false
    );
    assert!(TcpStream::connect(gateway).await.is_err());

    drop(occupied);
    let recovered = profile.launch(false, true).await;
    assert_eq!(recovered.pid, failed.pid);
    assert_eq!(recovered.web_url, failed.web_url);
    assert!(recovered.network_running && !recovered.needs_setup);
    assert!(recovered.network_error.is_none());
    let same = profile.connect().await;
    assert_eq!(same.instance_id(), retained.instance_id());
    assert_network(&same, network, gateway).await;
    assert_eq!(std::fs::read(profile.hosting_path()).unwrap(), saved);
    profile.stop(&same, &[web, network, gateway]).await;
}

#[tokio::test]
async fn launch_stale_saved_ip_reports_failure_without_rewriting_the_address() {
    let profile = Profile::new();
    let first = profile.launch(true, true).await;
    let client = profile.connect().await;
    let web = assert_web(&client, &first).await;
    let (network, gateway) = configure_network(&client).await;
    profile.stop(&client, &[web, network, gateway]).await;

    // A saved address can become non-local after an adapter/IP change. This is
    // a persisted public settings fixture, not a runtime projection injection.
    // The documentation address is only passed to bind(), never contacted.
    let stale_ip = "203.0.113.254";
    let stale_bind: SocketAddr = format!("{stale_ip}:{}", network.port()).parse().unwrap();
    assert!(
        bind_fixture_listener(stale_bind).is_err(),
        "fixture requires an address that is not assigned locally"
    );
    let saved = serde_json::to_vec_pretty(&json!({
        "schema_version":1,"bind":stale_bind.to_string(),"advertise_ip":stale_ip
    }))
    .unwrap();
    std::fs::write(profile.hosting_path(), &saved).unwrap();

    let failed = profile.launch(false, false).await;
    assert!(!failed.network_running && !failed.needs_setup);
    assert!(
        failed
            .network_error
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
    );
    let retained = profile.connect().await;
    let web = assert_web(&retained, &failed).await;
    assert_eq!(std::fs::read(profile.hosting_path()).unwrap(), saved);
    let network_status = retained
        .dispatch("hub_network_snapshot", json!({}))
        .await
        .unwrap();
    assert_eq!(network_status["server"]["running"], false);
    assert_eq!(network_status["gateway_port"], gateway.port());
    let model = retained.dispatch("hub_snapshot", json!({})).await.unwrap();
    assert_eq!(model["server"]["running"], false);
    assert_eq!(model["gateway"]["ready"], false);
    assert!(TcpStream::connect(network).await.is_err());
    assert!(TcpStream::connect(gateway).await.is_err());
    profile.stop(&retained, &[web, network, gateway]).await;
    assert_eq!(std::fs::read(profile.hosting_path()).unwrap(), saved);
}
