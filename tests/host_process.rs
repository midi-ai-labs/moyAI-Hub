use moyai_hub::host_process::HostClient;
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

const TOKEN: &str = "host-lifecycle-test-secret-00000000000000000000";
fn executable() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_moyai-hub"))
}

#[tokio::test]
async fn public_server_cli_starts_browser_management_restores_settings_and_stops() {
    let profile = Profile::new();
    let port_probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = port_probe.local_addr().unwrap().port();
    drop(port_probe);
    let mut original_hub = None;
    for first in [true, false] {
        let mut command = tokio::process::Command::new(executable());
        command.arg("--data-dir").arg(profile.directory.path());
        if first {
            command.arg("--web-port").arg(port.to_string());
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut process = command.spawn().unwrap();
        let client = tokio::time::timeout(Duration::from_secs(12), async {
            loop {
                if let Ok(client) = HostClient::connect(&profile.catalog()).await {
                    break client;
                }
                assert!(
                    process.try_wait().unwrap().is_none(),
                    "server exited during startup"
                );
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .unwrap();
        let web = client.dispatch("hub_web_status", json!({})).await.unwrap();
        assert_eq!(web["enabled"], true);
        let url = format!("http://127.0.0.1:{port}/admin/");
        assert_eq!(web["local_url"], url);
        assert!(web.get("key").is_none());
        let mut status_command = tokio::process::Command::new(executable());
        status_command
            .arg("--status")
            .arg("--data-dir")
            .arg(profile.directory.path())
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        status_command.creation_flags(0x08000000);
        let status = status_command.output().await.unwrap();
        assert!(status.status.success());
        let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
        assert_eq!(status["pid"], client.pid());
        assert_eq!(status["web"]["local_url"], url);
        let browser = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let response = browser.get(&url).send().await.unwrap();
        assert_eq!(response.status(), 200);
        assert!(!response.headers().contains_key("set-cookie"));
        assert!(response.text().await.unwrap().contains("moyAI Hub"));
        let snapshot = client.dispatch("hub_snapshot", json!({})).await.unwrap();
        if let Some(hub) = &original_hub {
            assert_eq!(&snapshot["store"]["hub_id"], hub);
        } else {
            original_hub = Some(snapshot["store"]["hub_id"].clone());
        }
        let stopped = tokio::process::Command::new(executable())
            .arg("--stop")
            .arg("--data-dir")
            .arg(profile.directory.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .status()
            .await
            .unwrap();
        assert!(stopped.success());
        assert!(
            tokio::time::timeout(Duration::from_secs(6), process.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
        assert!(browser.get(&url).send().await.is_err());
        assert!(!profile.directory.path().join("host-control.json").exists());
    }
}

struct Profile {
    directory: tempfile::TempDir,
}
impl Profile {
    fn new() -> Self {
        Self {
            directory: tempfile::tempdir().unwrap(),
        }
    }
    fn catalog(&self) -> PathBuf {
        self.directory.path().join("catalog.json")
    }
    fn spawn(&self) -> tokio::process::Child {
        let mut command = tokio::process::Command::new(executable());
        command
            .arg("--data-dir")
            .arg(self.directory.path())
            .arg("--web-port")
            .arg("0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        command.spawn().unwrap()
    }
    async fn connect(&self) -> HostClient {
        tokio::time::timeout(Duration::from_secs(12), async {
            loop {
                if let Ok(client) = HostClient::connect(&self.catalog()).await {
                    return client;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .unwrap()
    }
    async fn start(&self) -> (tokio::process::Child, HostClient) {
        let process = self.spawn();
        let client = self.connect().await;
        assert_eq!(process.id(), Some(client.pid()));
        (process, client)
    }
}
impl Drop for Profile {
    fn drop(&mut self) {
        // Best-effort cleanup targets only the authenticated host in this fixture.
        // It never kills a process by a saved PID or a broad executable name.
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
            let deadline = std::time::Instant::now() + Duration::from_secs(6);
            while matches!(child.try_wait(), Ok(None)) && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

async fn exited_successfully(process: &mut tokio::process::Child) {
    assert!(
        tokio::time::timeout(Duration::from_secs(6), process.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}

async fn health(url: &str) -> bool {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap()
        .get(format!("{url}/v1/health"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

#[tokio::test]
async fn public_server_keeps_api_and_gateway_running_after_control_client_is_dropped() {
    let profile = Profile::new();
    let (mut process, client) = profile.start().await;
    let pid = client.pid();
    let instance = client.instance_id().to_owned();
    assert_ne!(pid, std::process::id());
    let port_probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = port_probe.local_addr().unwrap();
    drop(port_probe);
    client
        .dispatch(
            "hub_start",
            json!({"bind":address.to_string(),"token":TOKEN,"enableGateway":true}),
        )
        .await
        .unwrap();
    let url = format!("http://{address}");
    assert!(health(&url).await);
    let snapshot = client.dispatch("hub_snapshot", json!({})).await.unwrap();
    assert_eq!(snapshot["gateway"]["ready"], true);
    drop(client);
    assert!(health(&url).await);
    let client = HostClient::connect(&profile.catalog()).await.unwrap();
    assert_eq!(client.pid(), pid);
    assert_eq!(client.instance_id(), instance);
    let snapshot = client.dispatch("hub_snapshot", json!({})).await.unwrap();
    assert_eq!(snapshot["server"]["running"], true);
    assert_eq!(snapshot["gateway"]["ready"], true);
    client.shutdown().await.unwrap();
    exited_successfully(&mut process).await;
    assert!(!health(&url).await);
    assert!(HostClient::connect(&profile.catalog()).await.is_err());
}

#[tokio::test]
async fn simultaneous_public_servers_keep_one_owner_and_replace_only_stale_descriptors() {
    let profile = Profile::new();
    let catalog = profile.catalog();
    let mut first_process = profile.spawn();
    let mut second_process = profile.spawn();
    let first_pid = first_process.id().unwrap();
    let second_pid = second_process.id().unwrap();
    let left = profile.connect().await;
    let right = HostClient::connect(&catalog).await.unwrap();
    assert_eq!(left.pid(), right.pid());
    assert_eq!(left.instance_id(), right.instance_id());
    let (running, rejected) = if left.pid() == first_pid {
        (&mut first_process, &mut second_process)
    } else {
        assert_eq!(left.pid(), second_pid);
        (&mut second_process, &mut first_process)
    };
    assert!(
        !tokio::time::timeout(Duration::from_secs(6), rejected.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    let old_instance = left.instance_id().to_owned();
    let descriptor = profile.directory.path().join("host-control.json");
    let retained = profile.directory.path().join("old-host-control.json");
    // A hard link preserves the exact private descriptor ACL for the stale-file case.
    std::fs::hard_link(&descriptor, &retained).unwrap();
    left.shutdown().await.unwrap();
    exited_successfully(running).await;
    assert!(!descriptor.exists());
    std::fs::rename(retained, &descriptor).unwrap();
    assert!(
        HostClient::connect(&catalog).await.is_err(),
        "connect-only must not start an absent host"
    );
    let (mut replacement_process, replacement) = profile.start().await;
    assert_ne!(replacement.instance_id(), old_instance);
    assert!(
        right.dispatch("hub_snapshot", json!({})).await.is_err(),
        "old management credentials do not attach to the replacement"
    );
    replacement.shutdown().await.unwrap();
    exited_successfully(&mut replacement_process).await;
}

#[tokio::test]
async fn descriptor_cannot_retarget_another_profile_and_does_not_stop_its_owner() {
    let owner = Profile::new();
    let other = Profile::new();
    let (mut process, client) = owner.start().await;
    let instance = client.instance_id().to_owned();
    std::fs::hard_link(
        owner.directory.path().join("host-control.json"),
        other.directory.path().join("host-control.json"),
    )
    .unwrap();
    assert!(HostClient::connect(&other.catalog()).await.is_err());
    let attached = HostClient::connect(&owner.catalog()).await.unwrap();
    assert_eq!(attached.instance_id(), instance);
    assert!(attached.dispatch("hub_snapshot", json!({})).await.is_ok());
    // Remove the deliberately invalid fixture descriptor so its cleanup cannot
    // mistake this rejected cross-profile record for a host it owns.
    std::fs::remove_file(other.directory.path().join("host-control.json")).unwrap();
    client.shutdown().await.unwrap();
    exited_successfully(&mut process).await;
}

#[tokio::test]
async fn actual_host_web_and_device_network_stop_independently() {
    let profile = Profile::new();
    let (mut process, client) = profile.start().await;
    let host_instance = client.instance_id().to_owned();
    // Reserve both simultaneously so the OS cannot select the same free port.
    let network_probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gateway_probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let network_address = network_probe.local_addr().unwrap();
    let gateway_address = gateway_probe.local_addr().unwrap();
    let settings = client
        .dispatch("hub_network_snapshot", json!({}))
        .await
        .unwrap();
    client
        .dispatch(
            "hub_network_settings",
            json!({
                "expectedRevision":settings["revision"], "gatewayPort":gateway_address.port()
            }),
        )
        .await
        .unwrap();
    drop(network_probe);
    drop(gateway_probe);
    client
        .dispatch(
            "hub_network_start",
            json!({
                "bind":network_address.to_string(), "advertiseIp":"127.0.0.1"
            }),
        )
        .await
        .unwrap();
    let network_before = client
        .dispatch("hub_network_snapshot", json!({}))
        .await
        .unwrap();
    let model_before = client.dispatch("hub_snapshot", json!({})).await.unwrap();
    assert_eq!(network_before["server"]["running"], true);
    assert_eq!(model_before["server"]["running"], true);
    assert_eq!(model_before["gateway"]["ready"], true);
    let tls = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .add_root_certificate(
            reqwest::Certificate::from_pem(
                network_before["ca_certificate_pem"]
                    .as_str()
                    .unwrap()
                    .as_bytes(),
            )
            .unwrap(),
        )
        .build()
        .unwrap();
    let device_url = format!("https://{network_address}/v1/network/self");
    // This is intentionally an anonymous probe: a real TLS handshake and the
    // device API's 401 prove availability without enrolling another fixture PC.
    assert_eq!(tls.get(&device_url).send().await.unwrap().status(), 401);
    let web_before = client.dispatch("hub_web_status", json!({})).await.unwrap();
    assert_eq!(web_before["enabled"], true);
    let web_url = web_before["local_url"].as_str().unwrap();
    let browser = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    assert_eq!(browser.get(web_url).send().await.unwrap().status(), 200);

    client.dispatch("hub_web_disable", json!({})).await.unwrap();
    assert!(browser.get(web_url).send().await.is_err());
    let network_after = client
        .dispatch("hub_network_snapshot", json!({}))
        .await
        .unwrap();
    let model_after = client.dispatch("hub_snapshot", json!({})).await.unwrap();
    assert_eq!(network_after["server"]["running"], true);
    assert_eq!(
        network_after["server"]["bind"],
        network_before["server"]["bind"]
    );
    assert_eq!(
        network_after["server"]["gateway_bind"],
        network_before["server"]["gateway_bind"]
    );
    assert_eq!(model_after["server"]["running"], true);
    assert_eq!(
        model_after["server"]["bind"],
        model_before["server"]["bind"]
    );
    assert_eq!(model_after["gateway"]["enabled"], true);
    assert_eq!(model_after["gateway"]["ready"], true);
    assert_eq!(tls.get(&device_url).send().await.unwrap().status(), 401);
    assert_eq!(client.instance_id(), host_instance);

    let web_restarted = client.dispatch("hub_web_enable", json!({})).await.unwrap();
    let web_url = web_restarted["local_url"].as_str().unwrap();
    client
        .dispatch("hub_network_stop", json!({}))
        .await
        .unwrap();
    let stopped = client
        .dispatch("hub_network_snapshot", json!({}))
        .await
        .unwrap();
    assert_eq!(stopped["server"]["running"], false);
    let model_stopped = client.dispatch("hub_snapshot", json!({})).await.unwrap();
    assert_eq!(model_stopped["server"]["running"], false);
    assert_eq!(model_stopped["gateway"]["enabled"], false);
    assert!(tls.get(&device_url).send().await.is_err());
    assert_eq!(browser.get(web_url).send().await.unwrap().status(), 200);
    let web_after = client.dispatch("hub_web_status", json!({})).await.unwrap();
    assert_eq!(web_after["enabled"], true);
    assert_eq!(web_after["epoch"], web_restarted["epoch"]);
    assert_eq!(web_after["local_url"], web_restarted["local_url"]);
    client.shutdown().await.unwrap();
    exited_successfully(&mut process).await;
    assert!(browser.get(web_url).send().await.is_err());
    assert!(HostClient::connect(&profile.catalog()).await.is_err());
}
