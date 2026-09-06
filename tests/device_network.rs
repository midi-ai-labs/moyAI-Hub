use moyai_hub::{
    catalog::CatalogStore,
    core::HubCore,
    network::{
        self, Action, CertificateReceipt, DeviceNetwork, Effect, GrantRequest, NetworkError,
        Publication, PublishMode, Rule, Subject, Usage,
    },
    service,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    net::IpAddr,
    sync::{Arc, Mutex},
};

struct Fixture {
    directory: tempfile::TempDir,
    owner: DeviceNetwork,
    now: u64,
}
struct Device {
    receipt: CertificateReceipt,
    key: String,
    csr: String,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let now = service::now_ms();
        let owner =
            DeviceNetwork::open(&directory.path().join("network.json"), "hub-test").unwrap();
        Self {
            directory,
            owner,
            now,
        }
    }
    fn device(&mut self, label: &str) -> Device {
        let (key, csr) = csr();
        let invite = self
            .owner
            .invite(
                &self.owner.revision(),
                label.into(),
                BTreeSet::new(),
                self.now,
            )
            .unwrap();
        let receipt = self
            .owner
            .enroll(invite["code"].as_str().unwrap(), &csr, ip(), self.now)
            .unwrap();
        Device { receipt, key, csr }
    }
    fn publish(&mut self, device: &Device, enabled: bool) {
        self.owner
            .publish(
                &device.receipt.device_id,
                Publication {
                    profile_id: "agent".into(),
                    name: "受付".into(),
                    endpoint: "https://127.0.0.1:7332/mcp".into(),
                    mode: PublishMode::Agent,
                    scope_id: "scope-1".into(),
                    enabled,
                },
                ip(),
                self.now,
            )
            .unwrap();
    }
    fn allow(&mut self, from: &Device, to: &Device, usage: Usage) -> String {
        self.owner
            .set_rule(
                &self.owner.revision(),
                Rule {
                    id: String::new(),
                    from: Subject::Device {
                        id: from.receipt.device_id.clone(),
                    },
                    to: Subject::Device {
                        id: to.receipt.device_id.clone(),
                    },
                    usage,
                    effect: Effect::Allow,
                    max_depth: 3,
                    profile_id: Some("agent".into()),
                },
            )
            .unwrap();
        self.owner.snapshot(self.now)["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| {
                r["from"]["id"] == from.receipt.device_id
                    && r["to"]["id"] == to.receipt.device_id
                    && r["usage"] == serde_json::to_value(usage).unwrap()
            })
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .into()
    }
}
fn ip() -> IpAddr {
    IpAddr::from([127, 0, 0, 1])
}
fn csr() -> (String, String) {
    let key = rcgen::KeyPair::generate().unwrap();
    let params = rcgen::CertificateParams::new(vec![ip().to_string()]).unwrap();
    let csr = params.serialize_request(&key).unwrap().pem().unwrap();
    (key.serialize_pem(), csr)
}
fn request(to: &Device, key: &str) -> GrantRequest {
    GrantRequest {
        audience_device_id: to.receipt.device_id.clone(),
        profile_id: "agent".into(),
        root_task_id: "root-task".into(),
        request_key: key.into(),
        parent_grant_id: None,
        parent_job_id: None,
        action: Action::Execute,
    }
}
fn control_request(to: &Device, key: &str, action: Action) -> GrantRequest {
    let mut value = request(to, key);
    value.action = action;
    value
}

#[test]
fn hub_server_role_cannot_be_obtained_by_same_ca_same_ip_device() {
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime, pem::PemObject};
    use tokio_rustls::rustls::{self, client::danger::ServerCertVerifier};

    let mut f = Fixture::new();
    let worker = f.device("worker-on-hub-pc");
    let (server, _) = f.owner.server_certificate(ip(), f.now).unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_slice(f.owner.ca_certificate().as_bytes()).unwrap())
        .unwrap();
    let verifier = rustls::client::WebPkiServerVerifier::builder(Arc::new(roots))
        .build()
        .unwrap();
    let validate = |pem: &str, name: ServerName<'static>| {
        let der = CertificateDer::from_pem_slice(pem.as_bytes()).unwrap();
        verifier
            .verify_server_cert(&der, &[], &name, &[], UnixTime::now())
            .is_ok()
    };
    assert!(validate(
        &worker.receipt.certificate_pem,
        ServerName::IpAddress(ip().into())
    ));
    assert!(!validate(
        &worker.receipt.certificate_pem,
        ServerName::try_from("hub.moyai.invalid").unwrap()
    ));
    assert!(validate(&server, ServerName::IpAddress(ip().into())));
    assert!(!validate(
        &server,
        ServerName::IpAddress(IpAddr::from([127, 0, 0, 2]).into())
    ));
    assert!(validate(
        &server,
        ServerName::try_from("hub.moyai.invalid").unwrap()
    ));

    let key = rcgen::KeyPair::generate().unwrap();
    let params =
        rcgen::CertificateParams::new(vec![ip().to_string(), "hub.moyai.invalid".into()]).unwrap();
    let csr = params.serialize_request(&key).unwrap().pem().unwrap();
    let invite = f
        .owner
        .invite(
            &f.owner.revision(),
            "cannot-request-hub-role".into(),
            BTreeSet::new(),
            f.now,
        )
        .unwrap();
    assert!(matches!(
        f.owner
            .enroll(invite["code"].as_str().unwrap(), &csr, ip(), f.now),
        Err(NetworkError::InvalidCsr)
    ));
    assert!(matches!(
        f.owner
            .renew_authenticated(&worker.receipt.certificate_sha256, &csr, ip(), f.now),
        Err(NetworkError::InvalidCsr)
    ));
}

#[test]
fn control_grants_remain_available_when_execute_and_observe_quotas_are_full() {
    let mut f = Fixture::new();
    let a = f.device("A");
    let b = f.device("B");
    f.publish(&b, true);
    f.allow(&a, &b, Usage::Direct);
    let first = f
        .owner
        .grant(&a.receipt.device_id, request(&b, "one"), f.now)
        .unwrap();
    for _ in 1..4096 {
        f.owner
            .grant(&a.receipt.device_id, request(&b, "one"), f.now)
            .unwrap();
    }
    assert!(matches!(
        f.owner
            .grant(&a.receipt.device_id, request(&b, "one"), f.now),
        Err(NetworkError::Full)
    ));
    let mut observe = request(&b, "one");
    observe.action = Action::Observe;
    let observed = f
        .owner
        .grant(
            &a.receipt.device_id,
            control_request(&b, "one", Action::Observe),
            f.now,
        )
        .unwrap();
    for _ in 1..4096 {
        f.owner
            .grant(
                &a.receipt.device_id,
                control_request(&b, "one", Action::Observe),
                f.now,
            )
            .unwrap();
    }
    assert!(matches!(
        f.owner.grant(&a.receipt.device_id, observe, f.now),
        Err(NetworkError::Full)
    ));
    let mut cancel = request(&b, "one");
    cancel.action = Action::Cancel;
    let cancel = f.owner.grant(&a.receipt.device_id, cancel, f.now).unwrap();
    assert!(
        f.owner
            .introspect(
                &b.receipt.device_id,
                &cancel.token,
                &a.receipt.certificate_sha256,
                Action::Cancel,
                f.now
            )
            .is_ok()
    );
    assert!(
        f.owner
            .introspect(
                &b.receipt.device_id,
                &observed.token,
                &a.receipt.certificate_sha256,
                Action::Cancel,
                f.now
            )
            .is_err()
    );
    assert!(
        f.owner
            .introspect(
                &b.receipt.device_id,
                &cancel.token,
                &a.receipt.certificate_sha256,
                Action::Execute,
                f.now
            )
            .is_err()
    );
    let later = f.now + 60_001;
    assert!(
        f.owner
            .introspect(
                &b.receipt.device_id,
                &observed.token,
                &a.receipt.certificate_sha256,
                Action::Observe,
                later
            )
            .is_err()
    );
    assert!(
        f.owner
            .introspect(
                &b.receipt.device_id,
                &cancel.token,
                &a.receipt.certificate_sha256,
                Action::Cancel,
                later
            )
            .is_err()
    );
    // The original execute receipt still carries the authorized job's 24h ancestry.
    assert!(
        f.owner
            .introspect(
                &b.receipt.device_id,
                &first.token,
                &a.receipt.certificate_sha256,
                Action::Observe,
                later
            )
            .is_ok()
    );
    let mut fresh = request(&b, "one");
    fresh.action = Action::Observe;
    f.owner.grant(&a.receipt.device_id, fresh, later).unwrap();
    assert!(matches!(
        f.owner
            .grant(&a.receipt.device_id, request(&b, "one"), later),
        Err(NetworkError::Full)
    ));
}

#[test]
fn control_grants_expire_during_twelve_hours_of_polling_without_losing_job_ancestry() {
    let mut f = Fixture::new();
    let a = f.device("A");
    let b = f.device("B");
    f.publish(&b, true);
    f.allow(&a, &b, Usage::Direct);
    f.owner
        .grant(&a.receipt.device_id, request(&b, "one"), f.now)
        .unwrap();
    let mut request = request(&b, "one");
    request.action = Action::Observe;
    for n in 0..4320 {
        let now = f.now + n * 10_000;
        let grant = f
            .owner
            .grant(
                &a.receipt.device_id,
                control_request(&b, "one", Action::Observe),
                now,
            )
            .unwrap();
        assert!(
            f.owner
                .introspect(
                    &b.receipt.device_id,
                    &grant.token,
                    &a.receipt.certificate_sha256,
                    Action::Observe,
                    now
                )
                .is_ok()
        );
    }
    request.action = Action::Cancel;
    f.owner
        .grant(
            &a.receipt.device_id,
            control_request(&b, "one", Action::Cancel),
            f.now + 43_200_000,
        )
        .unwrap();
    request.request_key = "never-authorized".into();
    assert!(matches!(
        f.owner
            .grant(&a.receipt.device_id, request, f.now + 43_200_000),
        Err(NetworkError::GrantDenied)
    ));
}

#[test]
fn control_introspection_reports_its_action_deadline_after_retirement() {
    for publication_off in [true, false] {
        let mut f = Fixture::new();
        let a = f.device("A");
        let b = f.device("B");
        f.publish(&b, true);
        let rule_id = f.allow(&a, &b, Usage::Direct);
        let execute = f
            .owner
            .grant(&a.receipt.device_id, request(&b, "one"), f.now)
            .unwrap();
        let controls: Vec<_> = [Action::Observe, Action::Cancel]
            .into_iter()
            .map(|action| {
                let receipt = f
                    .owner
                    .grant(
                        &a.receipt.device_id,
                        control_request(&b, "one", action),
                        f.now,
                    )
                    .unwrap();
                (action, receipt)
            })
            .collect();
        if publication_off {
            f.publish(&b, false);
        } else {
            f.owner.delete_rule(&f.owner.revision(), &rule_id).unwrap();
        }
        for (action, receipt) in controls {
            let reply = f
                .owner
                .introspect(
                    &b.receipt.device_id,
                    &receipt.token,
                    &a.receipt.certificate_sha256,
                    action,
                    f.now + 1,
                )
                .unwrap();
            // The successful wire response describes this operation's remaining
            // authority, not the retired execute deadline (the internal zero).
            assert!(reply.active);
            assert_eq!(reply.expires_at_ms, receipt.expires_at_ms);
            assert!(reply.expires_at_ms > f.now + 1);
            assert!(
                f.owner
                    .introspect(
                        &b.receipt.device_id,
                        &receipt.token,
                        &a.receipt.certificate_sha256,
                        Action::Execute,
                        f.now + 1,
                    )
                    .is_err()
            );
            if action == Action::Observe {
                assert!(
                    f.owner
                        .introspect(
                            &b.receipt.device_id,
                            &receipt.token,
                            &a.receipt.certificate_sha256,
                            Action::Cancel,
                            f.now + 1,
                        )
                        .is_err()
                );
            }
            assert!(
                f.owner
                    .introspect(
                        &b.receipt.device_id,
                        &receipt.token,
                        &a.receipt.certificate_sha256,
                        action,
                        receipt.expires_at_ms,
                    )
                    .is_err()
            );
        }
        // An accepted execute receipt already permits control for 24 hours.
        // Its execution expiry and retirement do not extend that retention.
        let control_deadline = f.now + 86_400_000;
        for action in [Action::Observe, Action::Cancel] {
            let reply = f
                .owner
                .introspect(
                    &b.receipt.device_id,
                    &execute.token,
                    &a.receipt.certificate_sha256,
                    action,
                    execute.expires_at_ms,
                )
                .unwrap();
            assert_eq!(reply.expires_at_ms, control_deadline);
            assert!(
                f.owner
                    .introspect(
                        &b.receipt.device_id,
                        &execute.token,
                        &a.receipt.certificate_sha256,
                        action,
                        control_deadline,
                    )
                    .is_err()
            );
        }
    }
}

#[test]
fn maximum_three_onward_hops_accepts_depth_four_and_rejects_a_fifth() {
    let mut f = Fixture::new();
    let devices: Vec<_> = (0..6).map(|n| f.device(&format!("D{n}"))).collect();
    for device in &devices {
        f.publish(device, true);
    }
    for n in 0..5 {
        f.allow(&devices[n], &devices[n + 1], Usage::Direct);
        if n > 0 {
            f.allow(&devices[0], &devices[n + 1], Usage::Delegated);
        }
    }
    let mut parent = None;
    for n in 0..4 {
        let mut onward = request(&devices[n + 1], &format!("hop-{n}"));
        onward.parent_grant_id = parent;
        onward.parent_job_id = (n > 0).then(|| format!("job-{n}"));
        let grant = f
            .owner
            .grant(&devices[n].receipt.device_id, onward, f.now)
            .unwrap();
        assert_eq!(grant.claims.depth, n as u8 + 1);
        assert_eq!(grant.claims.device_path.len(), n + 2);
        assert!(
            f.owner
                .introspect(
                    &devices[n + 1].receipt.device_id,
                    &grant.token,
                    &devices[n].receipt.certificate_sha256,
                    Action::Execute,
                    f.now
                )
                .is_ok()
        );
        parent = Some(grant.grant_id);
    }
    let mut fifth = request(&devices[5], "hop-5");
    fifth.parent_grant_id = parent;
    fifth.parent_job_id = Some("job-4".into());
    assert!(matches!(
        f.owner.grant(&devices[4].receipt.device_id, fifth, f.now),
        Err(NetworkError::GrantDenied)
    ));
}

#[test]
fn enrollment_requires_csr_possession_and_exact_retry_recovers_without_duplicate_identity() {
    let mut f = Fixture::new();
    let (_, csr) = csr();
    let invite = f
        .owner
        .invite(&f.owner.revision(), "Win19".into(), BTreeSet::new(), f.now)
        .unwrap();
    let code = invite["code"].as_str().unwrap();
    let mut corrupted = csr.as_bytes().to_vec();
    let end = csr.find("-----END").unwrap();
    let index = (0..end)
        .rev()
        .filter(|i| corrupted[*i].is_ascii_alphanumeric())
        .nth(2)
        .unwrap();
    corrupted[index] = if corrupted[index] == b'A' { b'B' } else { b'A' };
    assert!(
        f.owner
            .enroll(code, std::str::from_utf8(&corrupted).unwrap(), ip(), f.now)
            .is_err()
    );
    assert!(
        f.owner
            .enroll(code, "not a signed CSR", ip(), f.now)
            .is_err()
    );
    assert!(
        f.owner
            .enroll(code, &csr, IpAddr::from([127, 0, 0, 2]), f.now)
            .is_err()
    );
    let receipt = f.owner.enroll(code, &csr, ip(), f.now).unwrap();
    let repeated = f.owner.enroll(code, &csr, ip(), f.now + 1).unwrap();
    assert_eq!(receipt.device_id, repeated.device_id);
    assert_eq!(receipt.certificate_sha256, repeated.certificate_sha256);
    let (_, other) = self::csr();
    assert!(f.owner.enroll(code, &other, ip(), f.now + 2).is_err());
    let reopened =
        DeviceNetwork::open(&f.directory.path().join("network.json"), "hub-test").unwrap();
    assert_eq!(
        reopened.snapshot(f.now)["devices"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(reopened.ca_certificate(), f.owner.ca_certificate());
    assert!(
        reopened
            .authenticate(&receipt.certificate_sha256, f.now)
            .is_ok()
    );
}

#[test]
fn renewal_recovers_exact_old_certificate_retry_but_rejects_old_identity_elsewhere() {
    let mut f = Fixture::new();
    let a = f.device("A");
    let (_, csr) = csr();
    let renewed = f
        .owner
        .renew_authenticated(&a.receipt.certificate_sha256, &csr, ip(), f.now + 1000)
        .unwrap();
    assert_eq!(renewed.device_id, a.receipt.device_id);
    assert_ne!(renewed.certificate_sha256, a.receipt.certificate_sha256);
    assert!(
        f.owner
            .authenticate(&a.receipt.certificate_sha256, f.now + 1001)
            .is_err()
    );
    assert_eq!(
        f.owner
            .renew_authenticated(&a.receipt.certificate_sha256, &csr, ip(), f.now + 1001)
            .unwrap()
            .certificate_sha256,
        renewed.certificate_sha256
    );
    assert!(
        f.owner
            .renew_authenticated(&a.receipt.certificate_sha256, &a.csr, ip(), f.now + 1001)
            .is_err()
    );
    assert!(
        f.owner
            .renew_authenticated(&a.receipt.certificate_sha256, &csr, ip(), f.now + 700_000)
            .is_err()
    );
}

#[test]
fn audience_actor_publication_and_permission_revocation_do_not_resurrect_old_grants() {
    let mut f = Fixture::new();
    let a = f.device("A");
    let b = f.device("B");
    let c = f.device("C");
    f.publish(&b, true);
    f.allow(&a, &b, Usage::Direct);
    let g = f
        .owner
        .grant(&a.receipt.device_id, request(&b, "one"), f.now)
        .unwrap();
    assert!(
        f.owner
            .introspect(
                &b.receipt.device_id,
                &g.token,
                &a.receipt.certificate_sha256,
                Action::Execute,
                f.now
            )
            .is_ok()
    );
    assert!(
        f.owner
            .introspect(
                &c.receipt.device_id,
                &g.token,
                &a.receipt.certificate_sha256,
                Action::Execute,
                f.now
            )
            .is_err()
    );
    assert!(
        f.owner
            .introspect(
                &b.receipt.device_id,
                &g.token,
                &c.receipt.certificate_sha256,
                Action::Execute,
                f.now
            )
            .is_err()
    );
    f.publish(&b, false);
    f.publish(&b, true);
    assert!(
        f.owner
            .introspect(
                &b.receipt.device_id,
                &g.token,
                &a.receipt.certificate_sha256,
                Action::Execute,
                f.now
            )
            .is_err()
    );
    assert!(
        f.owner
            .introspect(
                &b.receipt.device_id,
                &g.token,
                &a.receipt.certificate_sha256,
                Action::Cancel,
                f.now
            )
            .is_ok()
    );
    let g = f
        .owner
        .grant(&a.receipt.device_id, request(&b, "two"), f.now)
        .unwrap();
    let mut rule: Rule =
        serde_json::from_value(f.owner.snapshot(f.now)["rules"][0].clone()).unwrap();
    rule.effect = Effect::Deny;
    f.owner.set_rule(&f.owner.revision(), rule.clone()).unwrap();
    rule.effect = Effect::Allow;
    f.owner.set_rule(&f.owner.revision(), rule).unwrap();
    assert!(
        f.owner
            .introspect(
                &b.receipt.device_id,
                &g.token,
                &a.receipt.certificate_sha256,
                Action::Execute,
                f.now
            )
            .is_err()
    );
    assert!(
        f.owner
            .grant(&a.receipt.device_id, request(&b, "two"), f.now)
            .is_ok()
    );
}

#[test]
fn delegated_permission_is_distinct_from_direct_access_and_checks_ancestors() {
    let mut f = Fixture::new();
    let a = f.device("Win00");
    let b = f.device("Win19");
    let c = f.device("Win20");
    let unrelated = f.device("Win03");
    f.publish(&b, true);
    f.publish(&c, true);
    f.allow(&a, &b, Usage::Direct);
    f.allow(&b, &c, Usage::Direct);
    f.allow(&a, &c, Usage::Delegated);
    assert!(
        f.owner
            .directory(&a.receipt.device_id, None, f.now)
            .unwrap()
            .iter()
            .all(|p| p.device_id != c.receipt.device_id)
    );
    let parent = f
        .owner
        .grant(&a.receipt.device_id, request(&b, "parent"), f.now)
        .unwrap();
    let (_, newcsr) = csr();
    f.owner
        .renew(&unrelated.receipt.device_id, &newcsr, ip(), f.now + 1)
        .unwrap();
    assert!(
        f.owner
            .introspect(
                &b.receipt.device_id,
                &parent.token,
                &a.receipt.certificate_sha256,
                Action::Execute,
                f.now + 2
            )
            .is_ok()
    );
    let peers = f
        .owner
        .directory(&b.receipt.device_id, Some(&parent.grant_id), f.now)
        .unwrap();
    assert_eq!(peers.len(), 1);
    let mut child = request(&c, "child");
    child.parent_grant_id = Some(parent.grant_id.clone());
    child.parent_job_id = Some("B-canonical-job".into());
    // The original 60-second bearer expiry is not the lifetime of the running parent job.
    f.owner
        .seen(&b.receipt.device_id, ip(), f.now + 61_000)
        .unwrap();
    f.owner
        .seen(&c.receipt.device_id, ip(), f.now + 61_000)
        .unwrap();
    let grant = f
        .owner
        .grant(&b.receipt.device_id, child, f.now + 61_000)
        .unwrap();
    assert_eq!(grant.claims.origin_device_id, a.receipt.device_id);
    assert_eq!(grant.claims.depth, 2);
    let mut forged = request(&c, "forged");
    forged.parent_grant_id = Some(parent.grant_id.clone());
    forged.parent_job_id = Some("job".into());
    assert!(
        f.owner
            .grant(&unrelated.receipt.device_id, forged, f.now + 61_000)
            .is_err()
    );
    f.now += 61_000;
    f.publish(&b, false);
    assert!(
        f.owner
            .introspect(
                &c.receipt.device_id,
                &grant.token,
                &b.receipt.certificate_sha256,
                Action::Execute,
                f.now
            )
            .is_err()
    );
}

#[test]
fn root_cancellation_preserves_exact_job_control_and_distinct_intent_budget() {
    let mut f = Fixture::new();
    let a = f.device("A");
    let b = f.device("B");
    f.publish(&b, true);
    f.allow(&a, &b, Usage::Direct);
    let first = f
        .owner
        .grant(&a.receipt.device_id, request(&b, "0"), f.now)
        .unwrap();
    for _ in 0..40 {
        f.owner
            .grant(&a.receipt.device_id, request(&b, "0"), f.now)
            .unwrap();
    }
    for n in 1..32 {
        f.owner
            .grant(&a.receipt.device_id, request(&b, &n.to_string()), f.now)
            .unwrap();
    }
    assert!(matches!(
        f.owner
            .grant(&a.receipt.device_id, request(&b, "32"), f.now),
        Err(NetworkError::Full)
    ));
    f.owner
        .cancel_lineage(&a.receipt.device_id, "root-task", f.now)
        .unwrap();
    assert!(
        f.owner
            .introspect(
                &b.receipt.device_id,
                &first.token,
                &a.receipt.certificate_sha256,
                Action::Execute,
                f.now
            )
            .is_err()
    );
    let mut control = request(&b, "0");
    control.action = Action::Cancel;
    let control = f.owner.grant(&a.receipt.device_id, control, f.now).unwrap();
    assert!(
        f.owner
            .introspect(
                &b.receipt.device_id,
                &control.token,
                &a.receipt.certificate_sha256,
                Action::Cancel,
                f.now
            )
            .is_ok()
    );
    assert!(
        f.owner
            .introspect(
                &b.receipt.device_id,
                &control.token,
                &a.receipt.certificate_sha256,
                Action::Execute,
                f.now
            )
            .is_err()
    );
    assert_eq!(
        f.owner
            .self_projection(&b.receipt.device_id, f.now)
            .unwrap()["cancelled_lineages"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let reopened =
        DeviceNetwork::open(&f.directory.path().join("network.json"), "hub-test").unwrap();
    assert!(
        reopened
            .introspect(
                &b.receipt.device_id,
                &control.token,
                &a.receipt.certificate_sha256,
                Action::Cancel,
                f.now
            )
            .is_err()
    );
    assert!(
        reopened
            .directory(&a.receipt.device_id, None, f.now)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn unknown_nested_persisted_fields_fail_without_rewriting_and_groups_are_admin_owned() {
    let mut f = Fixture::new();
    let a = f.device("A");
    assert!(
        f.owner
            .set_device(
                &f.owner.revision(),
                &a.receipt.device_id,
                "A".into(),
                BTreeSet::from(["self-appointed-admin".into()]),
                false
            )
            .is_err()
    );
    let path = f.directory.path().join("network.json");
    let mut saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    saved["devices"][&a.receipt.device_id]["require_signed_permits"] = json!(true);
    let bytes = serde_json::to_vec(&saved).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    assert!(DeviceNetwork::open(&path, "hub-test").is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn certificate_authority_key_mismatch_is_rejected_before_server_start() {
    let f = Fixture::new();
    let path = f.directory.path().join("network.json");
    let mut saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    saved["ca_private_key_pem"] = json!(rcgen::KeyPair::generate().unwrap().serialize_pem());
    let bytes = serde_json::to_vec(&saved).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    assert!(DeviceNetwork::open(&path, "hub-test").is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn redelegation_cycle_depth_origin_and_profile_scope_are_denied() {
    let mut f = Fixture::new();
    let a = f.device("A");
    let b = f.device("B");
    let c = f.device("C");
    let d = f.device("D");
    for device in [&a, &b, &c, &d] {
        f.publish(device, true);
    }
    f.allow(&a, &b, Usage::Direct);
    f.allow(&b, &c, Usage::Direct);
    f.allow(&c, &d, Usage::Direct);
    f.allow(&c, &a, Usage::Direct);
    f.allow(&a, &c, Usage::Delegated);
    let rule_id = f.allow(&a, &d, Usage::Delegated);
    let mut rule: Rule = serde_json::from_value(
        f.owner.snapshot(f.now)["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == rule_id)
            .unwrap()
            .clone(),
    )
    .unwrap();
    rule.max_depth = 1;
    f.owner.set_rule(&f.owner.revision(), rule).unwrap();
    let parent = f
        .owner
        .grant(&a.receipt.device_id, request(&b, "ab"), f.now)
        .unwrap();
    let mut child = request(&c, "bc");
    child.parent_grant_id = Some(parent.grant_id.clone());
    child.parent_job_id = Some("job-b".into());
    child.root_task_id = "forged-origin-task".into();
    assert!(f.owner.grant(&b.receipt.device_id, child, f.now).is_err());
    let mut child = request(&c, "bc");
    child.parent_grant_id = Some(parent.grant_id);
    child.parent_job_id = Some("job-b".into());
    let child = f.owner.grant(&b.receipt.device_id, child, f.now).unwrap();
    for target in [&d, &a] {
        let mut onward = request(target, "onward");
        onward.parent_grant_id = Some(child.grant_id.clone());
        onward.parent_job_id = Some("job-c".into());
        assert!(f.owner.grant(&c.receipt.device_id, onward, f.now).is_err());
    }
    let mut other_profile = request(&b, "other");
    other_profile.profile_id = "unpublished-admin".into();
    assert!(
        f.owner
            .grant(&a.receipt.device_id, other_profile, f.now)
            .is_err()
    );
}

#[test]
fn group_allow_with_device_deny_exception_and_root_budget_retention() {
    let mut f = Fixture::new();
    let a = f.device("A");
    let b = f.device("B");
    f.publish(&b, true);
    f.owner
        .set_group(&f.owner.revision(), None, "inputs".into())
        .unwrap();
    let group = f.owner.snapshot(f.now)["groups"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    f.owner
        .set_device(
            &f.owner.revision(),
            &a.receipt.device_id,
            "A".into(),
            BTreeSet::from([group.clone()]),
            false,
        )
        .unwrap();
    f.owner
        .set_rule(
            &f.owner.revision(),
            Rule {
                id: String::new(),
                from: Subject::Group { id: group },
                to: Subject::Device {
                    id: b.receipt.device_id.clone(),
                },
                usage: Usage::Direct,
                effect: Effect::Allow,
                max_depth: 1,
                profile_id: Some("agent".into()),
            },
        )
        .unwrap();
    assert_eq!(
        f.owner
            .directory(&a.receipt.device_id, None, f.now)
            .unwrap()
            .len(),
        1
    );
    for n in 0..128 {
        let mut r = request(&b, "one");
        r.root_task_id = format!("root-{n}");
        f.owner.grant(&a.receipt.device_id, r, f.now).unwrap();
    }
    let mut exhausted = request(&b, "one");
    exhausted.root_task_id = "extra-root".into();
    assert!(matches!(
        f.owner.grant(&a.receipt.device_id, exhausted, f.now),
        Err(NetworkError::Full)
    ));
    f.now += 86_400_001;
    f.owner.seen(&b.receipt.device_id, ip(), f.now).unwrap();
    assert!(
        f.owner
            .grant(&a.receipt.device_id, request(&b, "after-expiry"), f.now)
            .is_ok()
    );
    f.owner
        .set_rule(
            &f.owner.revision(),
            Rule {
                id: String::new(),
                from: Subject::Device {
                    id: a.receipt.device_id.clone(),
                },
                to: Subject::Device {
                    id: b.receipt.device_id.clone(),
                },
                usage: Usage::Direct,
                effect: Effect::Deny,
                max_depth: 1,
                profile_id: Some("agent".into()),
            },
        )
        .unwrap();
    assert!(
        f.owner
            .directory(&a.receipt.device_id, None, f.now)
            .unwrap()
            .is_empty()
    );
}

fn http(ca: &str, device: Option<&Device>) -> reqwest::Client {
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .tls_built_in_root_certs(false)
        .add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap());
    if let Some(device) = device {
        builder = builder.identity(
            reqwest::Identity::from_pem(
                format!("{}\n{}", device.receipt.certificate_pem, device.key).as_bytes(),
            )
            .unwrap(),
        );
    }
    builder.build().unwrap()
}

#[tokio::test]
async fn actual_tls_enrollment_model_bridge_spoofing_and_revocation() {
    let mut f = Fixture::new();
    let a = f.device("A");
    let b = f.device("B");
    let ca = f.owner.ca_certificate().to_string();
    let (cert, key) = f.owner.server_certificate(ip(), f.now).unwrap();
    let network = Arc::new(Mutex::new(f.owner));
    let core = Arc::new(Mutex::new(
        HubCore::new(CatalogStore::empty("test")).unwrap(),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("https://{}", listener.local_addr().unwrap());
    let listener = network::tls::NetworkListener::new(
        listener,
        network::tls::acceptor(&cert, &key, &ca, true).unwrap(),
    );
    let router = service::network_router(core, network.clone(), None);
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<network::tls::TlsPeer>(),
        )
        .await
        .unwrap()
    });
    let anonymous = http(&ca, None);
    assert_eq!(
        anonymous
            .get(format!("{endpoint}/v1/network/self"))
            .header("x-moyai-verified-device", &a.receipt.device_id)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let (_, newcsr) = csr();
    let invitation = {
        let mut owner = network.lock().unwrap();
        let revision = owner.revision();
        owner
            .invite(&revision, "New device".into(), BTreeSet::new(), f.now)
            .unwrap()
    };
    let enrolled = anonymous
        .post(format!("{endpoint}/v1/network/enroll"))
        .json(&json!({"code":invitation["code"],"csr_pem":newcsr}))
        .send()
        .await
        .unwrap();
    assert_eq!(enrolled.status(), 200);
    let client = http(&ca, Some(&a));
    assert_eq!(
        client
            .get(format!("{endpoint}/v1/network/self"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        client
            .get(format!("{endpoint}/v1/network/self"))
            .header("origin", "https://untrusted.invalid")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client
            .get(format!("{endpoint}/v1/network/self"))
            .header("host", "untrusted.invalid")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let session: Value = client
        .post(format!("{endpoint}/v1/network/model-session"))
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(session["identity_scope"], "device_session");
    let second: Value = client
        .post(format!("{endpoint}/v1/network/model-session"))
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_ne!(session["id"], second["id"]);
    assert_eq!(
        client
            .get(format!("{endpoint}/v1/catalog"))
            .bearer_auth(session["client_token"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        http(&ca, Some(&b))
            .get(format!("{endpoint}/v1/catalog"))
            .bearer_auth(session["client_token"].as_str().unwrap())
            .header("x-moyai-verified-device", &a.receipt.device_id)
            .header("x-moyai-verified-cert", &a.receipt.certificate_sha256)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    {
        let mut owner = network.lock().unwrap();
        let revision = owner.revision();
        owner
            .set_device(
                &revision,
                &a.receipt.device_id,
                "A".into(),
                BTreeSet::new(),
                true,
            )
            .unwrap();
    }
    assert_eq!(
        client
            .get(format!("{endpoint}/v1/catalog"))
            .bearer_auth(session["client_token"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    task.abort();
    let _ = task.await;
}
