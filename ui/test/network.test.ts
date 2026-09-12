import assert from "node:assert/strict";
import test from "node:test";
import { NetworkEdits, networkAdmissionStatus, networkCertificateStatus, networkClientAction, networkClients, networkDeviceDeletion, networkDevices, networkDeviceStatus, networkDiagnosticMatches, networkDiagnosticStages, networkDiagnosticTarget, networkGatewayPort, networkHostingAddress, networkServerAddress, networkSubject, type NetworkSnapshot, type NetworkDiagnostics } from "../src/network_state.ts";

function snapshot(revision = "9007199254740993"): NetworkSnapshot {
    return { revision, hub_id: "hub-a", ca_certificate_pem: "public-ca", ca_sha256: "digest", gateway_port: 0, hosting: { configured: false, bind: null, advertise_ip: null, error: null }, server: { running: false, bind: null, gateway_bind: null, certificate: null },
        devices: [{ device_id: "device-a", label: "Win19", groups: ["dev", "gpu"], revoked: false, admission: "allowed", certificate_sha256: "device-cert",
            expires_at_ms: 99999999, last_seen_ms: 30, observed_ip: "192.168.1.19", online: true,
            publications: [{ profile_id: "receiver", name: "Temp受付", enabled: true, mode: "agent", endpoint: "https://192.168.1.19:7332" }] }],
        join_requests: [], groups: [{ id: "dev", label: "開発" }, { id: "gpu", label: "GPU搭載" }], rules: [] };
}

test("saved hosting restores its public address while stopped and preserves a dirty server editor across polls", () => {
    const current = snapshot();
    assert.equal(networkHostingAddress(current), null);
    current.hosting = { configured: true, bind: "0.0.0.0:9471", advertise_ip: "192.168.1.10", error: null };
    const owner = new NetworkEdits(); owner.accept(current);
    assert.deepEqual(owner.serverAddress(), { ip: "192.168.1.10", port: "9471" });
    owner.touch("network-server-form");
    owner.accept({ ...current, revision: "9007199254740994" });
    assert.equal(owner.serverAddress(), null, "an intentionally cleared IP or port must not be refilled by polling");
    owner.reviewLatest();
    assert.equal(owner.serverAddress(), null, "refresh retains the user's draft too");
    owner.saved("network-server-form");
    assert.deepEqual(owner.serverAddress(), { ip: "192.168.1.10", port: "9471" });
    current.server = { ...current.server, running: true, bind: "https://127.0.0.1:9941" };
    assert.deepEqual(networkHostingAddress(current), { ip: "127.0.0.1", port: "9941" }, "the active endpoint takes precedence");
    current.server.running = false;
    current.hosting = { configured: true, bind: "[::]:443", advertise_ip: "::1", error: null };
    assert.deepEqual(networkHostingAddress(current), { ip: "::1", port: "443" });
    current.hosting = { configured: false, bind: null, advertise_ip: null, error: "unsupported schema" };
    assert.equal(networkHostingAddress(current), null, "invalid settings must not invent a usable endpoint");
});

test("fixed Gateway port stays explicit and renewal failure is distinct from an expired certificate", () => {
    assert.equal(networkGatewayPort(""), 0);
    assert.equal(networkGatewayPort(" 7443 "), 7443);
    for (const value of ["-1", "65536", "1.5", "abc"]) assert.equal(networkGatewayPort(value), null);
    const certificate = { sha256: "public-digest", expires_at_ms: "2000", renew_after_ms: "500", last_success_ms: "100", last_error: null as string | null };
    assert.equal(networkCertificateStatus(certificate, 1000).warning, false);
    certificate.last_error = "certificate_renewal_failed";
    assert.match(networkCertificateStatus(certificate, 1000).label, /現在の証明書は有効期限まで/);
    assert.equal(networkCertificateStatus(certificate, 1000).warning, true);
    assert.match(networkCertificateStatus(certificate, 2000).label, /有効期限が切れています/);
});
test("local diagnostic reports internal checks without claiming remote reachability and expires with the listener identity", () => {
    const current = snapshot();
    const certificate = { sha256: "current-leaf", expires_at_ms: "2000", renew_after_ms: "500", last_success_ms: "100", last_error: null };
    current.server = { running: true, bind: "https://192.168.1.10:9471", gateway_bind: "https://192.168.1.10:7443", certificate };
    const result: NetworkDiagnostics = { hub_id: current.hub_id, metadata_endpoint: current.server.bind!, gateway_endpoint: current.server.gateway_bind,
        gateway_ready: true, certificate, gateway_certificate: certificate };
    assert.equal(networkDiagnosticMatches(result, current), true);
    assert.deepEqual(networkDiagnosticStages(result, 1000).map(stage => stage.status), ["pass", "pass", "skipped"]);
    assert.match(networkDiagnosticStages(result, 1000)[2].detail, /設定は自動で変更しません/);
    assert.deepEqual(networkDiagnosticStages({ ...result, gateway_ready: false }, 1000).map(stage => stage.status), ["pass", "fail", "skipped"]);
    assert.deepEqual(networkDiagnosticStages(result, 2000).map(stage => stage.status), ["fail", "fail", "skipped"]);
    const target = networkDiagnosticTarget(current);
    current.server.certificate = { ...certificate, sha256: "renewed-leaf" };
    assert.equal(networkDiagnosticMatches(result, current), false);
    assert.notEqual(networkDiagnosticTarget(current), target);
    current.server.certificate = certificate;
    current.server.running = false;
    assert.equal(networkDiagnosticMatches(result, current), false);
    assert.notEqual(networkDiagnosticTarget(current), target);
});
test("each administration editor keeps its reviewed exact decimal revision across polling", () => {
    const owner = new NetworkEdits();
    owner.accept(snapshot()); owner.touch("rule"); owner.touch("device");
    owner.accept(snapshot("9007199254740994"));
    assert.equal(owner.target("rule"), "9007199254740993");
    assert.equal(owner.target("device"), "9007199254740993");
    owner.saved("device");
    assert.equal(owner.target("device"), "9007199254740994");
    assert.equal(owner.accept(snapshot()), false);
    assert.equal(owner.snapshot?.revision, "9007199254740994");
    owner.reviewLatest();
    assert.equal(owner.target("rule"), "9007199254740994");
});
test("overlapping memberships remain searchable by either group, device name, identity or observed IP", () => {
    const current = snapshot();
    for (const query of ["開発", "gpu", "WIN19", "device-a", "1.19"]) assert.equal(networkDevices(current, query).length, 1, query);
    assert.equal(networkDevices(current, "Win20").length, 0);
    assert.deepEqual(current.devices[0].groups, ["dev", "gpu"]);
});
test("approval and presence stay independent while a stopped device remains online", () => {
    const device = snapshot().devices[0];
    assert.equal(networkAdmissionStatus(device).label, "許可");
    assert.equal(networkDeviceStatus(device).label, "オンライン");
    assert.match(networkDeviceStatus(device).detail, /接続ルール/);
    device.admission = "stopped";
    assert.equal(networkAdmissionStatus(device).label, "停止");
    assert.match(networkAdmissionStatus(device).detail, /再許可/);
    assert.equal(networkDeviceStatus(device).label, "オンライン");
    device.publications[0].enabled = false;
    assert.match(networkDeviceStatus(device).detail, /受付の申告なし/);
    device.online = false;
    assert.equal(networkDeviceStatus(device).label, "オフライン");
    assert.equal(networkAdmissionStatus(device).label, "停止");
    device.revoked = true;
    assert.equal(networkAdmissionStatus(device).label, "恒久失効");
    assert.equal(networkDeviceStatus(device).label, "オフライン");
});
test("one searchable list transitions an approved request to its registered device without a second management owner", () => {
    const current = snapshot();
    const pending = { request_id: "request-b", label: "Win20", observed_ip: "192.168.1.20", key_sha256: "fingerprint-b",
        created_at_ms: 100, expires_at_ms: 200, status: "pending" as const };
    current.join_requests = [{ ...pending, request_id: "expired", status: "expired" }, pending];
    assert.deepEqual(networkClients(current, "").map(row => row.kind), ["request", "device", "request"]);
    assert.equal(networkClients(current, "", "pending").length, 1);
    for (const term of ["WIN20", "1.20", "fingerprint-b", "request-b"]) {
        const rows = networkClients(current, term, "pending");
        assert.equal(rows.length, 1, term);
        assert.equal(rows[0].kind === "request" && rows[0].request.request_id, "request-b");
    }
    current.join_requests = current.join_requests.filter(row => row.request_id !== pending.request_id);
    current.devices.push({ ...current.devices[0], device_id: "device-b", label: pending.label, observed_ip: pending.observed_ip, groups: [] });
    const registered = networkClients(current, "device-b");
    assert.equal(registered.length, 1);
    assert.equal(registered[0].kind, "device");
    assert.equal(networkClients(current, "", "pending").length, 0);
    assert.equal(networkClients(current, "開発")[0].kind, "device");
});
test("one-click approval uses the displayed request identity and revision with no required editor input", () => {
    const current = snapshot();
    const request = { request_id: "request-b", label: "申請PC", observed_ip: "192.168.1.20", key_sha256: "key-b",
        created_at_ms: 100, expires_at_ms: 200, status: "pending" as const };
    current.join_requests = [request];
    const action = networkClientAction(networkClients(current, "", "pending")[0], current.revision);
    assert.deepEqual(action, { command: "hub_network_approve", label: "許可", args: {
        expectedRevision: "9007199254740993", requestId: "request-b", label: "申請PC", groups: [],
    } });
    const owner = new NetworkEdits(); owner.accept(current); owner.touch("network-device-form");
    owner.accept({ ...current, revision: "9007199254740994" });
    assert.equal(action?.args.expectedRevision, "9007199254740993", "polling must not silently retarget a displayed button");
    assert.equal(owner.target("network-device-form"), "9007199254740993");
    assert.equal(networkClientAction({ kind: "request", request: { ...request, status: "expired" } }, current.revision), null);
});
test("stop and reallow keep the same registered identity and never invoke irreversible revocation", () => {
    const current = snapshot();
    const device = current.devices[0];
    assert.deepEqual(networkClientAction({ kind: "device", device }, current.revision), {
        command: "hub_network_admission", label: "停止", args: { expectedRevision: current.revision, deviceId: device.device_id, stopped: true },
    });
    device.admission = "stopped";
    assert.deepEqual(networkClientAction({ kind: "device", device }, current.revision), {
        command: "hub_network_admission", label: "再許可", args: { expectedRevision: current.revision, deviceId: device.device_id, stopped: false },
    });
    assert.deepEqual(device.groups, ["dev", "gpu"]);
    assert.equal(device.revoked, false);
    device.admission = "revoked";
    assert.equal(networkClientAction({ kind: "device", device }, current.revision), null);
    device.admission = "allowed"; device.revoked = true;
    assert.equal(networkClientAction({ kind: "device", device }, current.revision), null);
});
test("permission subjects preserve direction and typed device or group identity", () => {
    assert.deepEqual(networkSubject("device:from"), { kind: "device", id: "from" });
    assert.deepEqual(networkSubject("group:to"), { kind: "group", id: "to" });
    for (const invalid of ["", "device:", "from", "profile:to"]) assert.equal(networkSubject(invalid), null);
});

test("deletion confirms the exact identity even for duplicate names and reports unsettled work", () => {
    const current = snapshot(); const first = current.devices[0];
    const second = { ...first, device_id: "replacement", registered_ip: "192.168.1.20", unsettled_jobs: 2 };
    const a = networkDeviceDeletion(first, current.revision); const b = networkDeviceDeletion(second, current.revision);
    assert.notDeepEqual(a.args, b.args);
    assert.match(b.identity, /replacement/); assert.match(b.identity, /192\.168\.1\.20/);
    assert.match(b.warning, /2 件/); assert.match(b.warning, /停止または完了/);
    assert.match(a.warning, /過去の履歴/); assert.match(a.warning, /停止する操作ではありません/);
    first.device_id = "a later registration";
    assert.equal(a.args.deviceId, "device-a", "the confirmation target must not follow later snapshots");
});

test("legacy pending name collisions require the name editor instead of blind one-click approval", () => {
    const request = { request_id: "pending", label: "Win19", observed_ip: "192.168.1.20", key_sha256: "key", created_at_ms: 1, expires_at_ms: 99, status: "pending" as const, name_conflict: true };
    assert.equal(networkClientAction({ kind: "request", request }, "2"), null);
    assert.equal(networkClientAction({ kind: "request", request: { ...request, name_conflict: false } }, "2")?.command, "hub_network_approve");
});

test("a running network snapshot restores its HTTPS bind as IP and port without adding a second scheme", () => {
    assert.deepEqual(networkServerAddress("https://192.168.1.10:9471"), { ip: "192.168.1.10", port: "9471" });
    assert.deepEqual(networkServerAddress("https://[::1]:9471"), { ip: "::1", port: "9471" });
    assert.deepEqual(networkServerAddress("https://127.0.0.1"), { ip: "127.0.0.1", port: "443" });
});
