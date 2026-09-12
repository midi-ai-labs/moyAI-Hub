use super::*;

async fn fixture() -> (
    tempfile::TempDir,
    tokio::task::JoinHandle<Result<(), String>>,
    HostClient,
) {
    let directory = tempfile::tempdir().unwrap();
    let catalog = directory.path().join("catalog.json");
    let server_path = catalog.clone();
    let server = tokio::spawn(async move {
        run_server(
            server_path,
            Some(json!({"local_port":0,"remote":{"mode":"off"}})),
        )
        .await
    });
    let client = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(client) = HostClient::connect(&catalog).await {
                break client;
            }
            assert!(
                !server.is_finished(),
                "host startup failed before publishing its descriptor"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    (directory, server, client)
}

#[tokio::test]
async fn management_client_lifetime_does_not_stop_host_and_explicit_shutdown_does() {
    let (directory, server, client) = fixture().await;
    let instance = client.instance_id().to_owned();
    let snapshot = client.dispatch("hub_snapshot", json!({})).await.unwrap();
    assert!(snapshot["store"].is_object());
    drop(client);
    let client = HostClient::connect(&directory.path().join("catalog.json"))
        .await
        .unwrap();
    assert_eq!(client.instance_id(), instance);
    assert!(profile_busy(&client.descriptor.catalog_path).unwrap());
    client.shutdown().await.unwrap();
    server.await.unwrap().unwrap();
    assert!(!directory.path().join(DESCRIPTOR).exists());
    assert!(
        HostClient::connect(&directory.path().join("catalog.json"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn management_requires_private_token_and_rejects_browser_and_foreign_host_headers() {
    let (_directory, server, client) = fixture().await;
    let url = format!("{}/host/command", client.descriptor.base_url());
    let request = || {
        client
            .http
            .post(&url)
            .json(&json!({"command":"hub_snapshot","args":{}}))
    };
    for response in [
        request().send().await.unwrap(),
        request().bearer_auth("wrong-token").send().await.unwrap(),
        request()
            .bearer_auth(&client.descriptor.token)
            .header("origin", "http://127.0.0.1")
            .send()
            .await
            .unwrap(),
        request()
            .bearer_auth(&client.descriptor.token)
            .header("sec-fetch-site", "same-origin")
            .send()
            .await
            .unwrap(),
        request()
            .bearer_auth(&client.descriptor.token)
            .header("host", "localhost")
            .send()
            .await
            .unwrap(),
    ] {
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    assert!(client.dispatch("hub_snapshot", json!({})).await.is_ok());
    client.shutdown().await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn invalid_and_oversized_management_commands_are_bounded_without_affecting_host() {
    let (_directory, server, client) = fixture().await;
    assert!(client.dispatch("no_such_command", json!({})).await.is_err());
    assert!(client.dispatch("hub_snapshot", Value::Null).await.is_err());
    let oversized = json!({"payload":"x".repeat(COMMAND_LIMIT)});
    assert!(client.dispatch("hub_snapshot", oversized).await.is_err());
    let response = client
        .http
        .post(format!("{}/host/command", client.descriptor.base_url()))
        .bearer_auth(&client.descriptor.token)
        .header("content-type", "application/json")
        .body(" ".repeat(COMMAND_LIMIT + 1))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let response = client
        .http
        .post(format!("{}/host/command", client.descriptor.base_url()))
        .bearer_auth(&client.descriptor.token)
        .json(&json!({"command":"hub_snapshot","args":{},"extra":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    client.health().await.unwrap();
    client.shutdown().await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn live_profile_lock_prevents_a_second_host_or_descriptor_replacement() {
    let (directory, server, client) = fixture().await;
    let before = std::fs::read(directory.path().join(DESCRIPTOR)).unwrap();
    assert!(
        run_server(directory.path().join("catalog.json"), None)
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(directory.path().join(DESCRIPTOR)).unwrap(),
        before
    );
    client.shutdown().await.unwrap();
    server.await.unwrap().unwrap();
}

#[test]
fn credential_descriptor_is_private_and_does_not_accept_remote_or_cross_profile_targets() {
    let directory = tempfile::tempdir().unwrap();
    let catalog = canonical_catalog(&directory.path().join("catalog.json"), true).unwrap();
    let mut descriptor = Descriptor {
        schema_version: 1,
        instance_id: ulid::Ulid::new().to_string(),
        pid: std::process::id(),
        port: 1,
        token: format!("{}{}", ulid::Ulid::new(), ulid::Ulid::new()),
        catalog_path: catalog.clone(),
    };
    write_descriptor(&descriptor).unwrap();
    let file = File::open(directory.path().join(DESCRIPTOR)).unwrap();
    private_file::verify(&file).unwrap();
    assert!(read_descriptor(&catalog).is_ok());
    descriptor.catalog_path = directory.path().join("other.json");
    assert!(descriptor.validate(&catalog).is_err());
    descriptor.catalog_path = catalog;
    descriptor.port = 0;
    assert!(descriptor.validate(&descriptor.catalog_path).is_err());
    assert!(descriptor.base_url().starts_with("http://127.0.0.1:"));
}

#[cfg(unix)]
#[test]
fn readable_by_other_accounts_descriptor_is_rejected() {
    use std::os::unix::fs::PermissionsExt;
    let file = tempfile::tempfile().unwrap();
    file.set_permissions(std::fs::Permissions::from_mode(0o644))
        .unwrap();
    assert!(private_file::verify(&file).is_err());
}
