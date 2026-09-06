import assert from "node:assert/strict";
import test from "node:test";
import { NetworkEdits, networkDevices, networkDeviceStatus, networkServerAddress, networkSubject, type NetworkSnapshot } from "../src/network_state.ts";

function snapshot(revision = "9007199254740993"): NetworkSnapshot {
    return { revision, hub_id: "hub-a", ca_certificate_pem: "public-ca", ca_sha256: "digest", server: { running: false, bind: null },
        devices: [{ device_id: "device-a", label: "Win19", groups: ["dev", "gpu"], revoked: false, certificate_sha256: "device-cert",
            expires_at_ms: 99999999, last_seen_ms: 30, observed_ip: "192.168.1.19", online: true,
            publications: [{ profile_id: "receiver", name: "Temp受付", enabled: true, mode: "agent", endpoint: "https://192.168.1.19:7332" }] }],
        groups: [{ id: "dev", label: "開発" }, { id: "gpu", label: "GPU搭載" }], rules: [] };
}
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
test("administration status separates presence, reception and revocation from completed execution stop", () => {
    const device = snapshot().devices[0];
    assert.match(networkDeviceStatus(device).detail, /接続許可/);
    device.publications[0].enabled = false;
    assert.match(networkDeviceStatus(device).detail, /受付は停止/);
    device.online = false;
    assert.equal(networkDeviceStatus(device).label, "オフライン");
    device.revoked = true;
    assert.equal(networkDeviceStatus(device).label, "失効");
    assert.match(networkDeviceStatus(device).detail, /停止完了とは別/);
});
test("permission subjects preserve direction and typed device or group identity", () => {
    assert.deepEqual(networkSubject("device:from"), { kind: "device", id: "from" });
    assert.deepEqual(networkSubject("group:to"), { kind: "group", id: "to" });
    for (const invalid of ["", "device:", "from", "profile:to"]) assert.equal(networkSubject(invalid), null);
});

test("a running network snapshot restores its HTTPS bind as IP and port without adding a second scheme", () => {
    assert.deepEqual(networkServerAddress("https://192.168.1.10:9471"), { ip: "192.168.1.10", port: "9471" });
    assert.deepEqual(networkServerAddress("https://[::1]:9471"), { ip: "::1", port: "9471" });
    assert.deepEqual(networkServerAddress("https://127.0.0.1"), { ip: "127.0.0.1", port: "443" });
});
