import assert from "node:assert/strict";
import test from "node:test";
import { HistorySelection, historyDeviceCanRead, historyQueryKey, historyResultMatches, historyStateLabel,
    type HistoryDetail, type HistoryPage, type HistoryQuery, type HistoryRow, type HistorySnapshot } from "../src/history_state.ts";
import type { NetworkDevice } from "../src/network_state.ts";

function query(overrides: Partial<HistoryQuery> = {}): HistoryQuery {
    return { deviceId: "WinB", direction: "execution", offset: 0, anchor: null, id: null, ...overrides };
}
function row(overrides: Partial<HistoryRow> = {}): HistoryRow {
    return { id: "job-b", direction: "execution", created_at_ms: 100, updated_at_ms: 200, title: "CPU使用率を調査",
        peer_label: "WinA", target_label: "temp", state: "running", stop_status: "none", state_source: "local_runtime",
        result_received: false, session_id: "chat-b", profile_id: "profile-b", job_id: "job-b", root_task_id: "chat-a",
        device_path: ["WinA", "WinB"], can_stop: true, ...overrides };
}
function snapshot(overrides: Partial<HistorySnapshot> = {}): HistorySnapshot {
    return { request_id: "request-a", device_id: "WinB", device_label: "WinB", direction: "execution", offset: 0,
        anchor: null, id: null, status: "pending", requested_at_ms: 100, completed_at_ms: null, expires_at_ms: 1000,
        response: null, error: null, device_online: true, device_admission: "allowed", cached: false, ...overrides };
}
function page(rows = [row()]): HistoryPage { return { rows, anchor: "page-anchor", next_offset: null }; }
function detail(value = row()): HistoryDetail { return { row: value, markdown: "# 実行記録\n\n調査結果", truncated: false }; }
function device(overrides: Partial<NetworkDevice> = {}): NetworkDevice {
    return { device_id: "WinB", label: "WinB", groups: [], revoked: false, admission: "allowed", certificate_sha256: "digest",
        expires_at_ms: 1000, last_seen_ms: 100, observed_ip: "192.168.1.20", online: true, publications: [], ...overrides };
}

test("each Hub history query identifies the exact device, direction, page anchor and task", () => {
    const original = query();
    assert.equal(historyQueryKey(original), historyQueryKey(structuredClone(original)));
    for (const changed of [query({ deviceId: "WinC" }), query({ direction: "instruction" }), query({ offset: 50 }),
        query({ anchor: "older-page" }), query({ id: "job-c" })]) {
        assert.notEqual(historyQueryKey(original), historyQueryKey(changed));
    }
});

test("selection snapshots input and rejects late replies across A/B/A navigation and clear", () => {
    const selection = new HistorySelection();
    const original = query();
    const generationA = selection.begin(original);
    original.deviceId = "mutated-caller";
    assert.equal(selection.query?.deviceId, "WinB");
    const generationB = selection.begin(query({ deviceId: "WinC" }));
    assert.equal(selection.accept(generationA, snapshot()), false);
    assert.equal(selection.accept(generationB, snapshot({ device_id: "WinC", request_id: "request-b" })), true);
    const generationAgain = selection.begin(query());
    assert.equal(selection.accept(generationA, snapshot()), false);
    assert.equal(selection.accept(generationAgain, snapshot({ request_id: "request-new" })), true);
    selection.clear();
    assert.equal(selection.query, null);
    assert.equal(selection.requestId, null);
    assert.equal(selection.accept(generationAgain, snapshot({ request_id: "request-new" })), false);
});

test("a pending Hub request pins its request ID while accepting its own terminal response", () => {
    const selection = new HistorySelection();
    const generation = selection.begin(query());
    assert.equal(selection.accept(generation, snapshot()), true);
    assert.equal(selection.requestId, "request-a");
    assert.equal(selection.accept(generation, snapshot({ request_id: "request-other", status: "completed", response: page() })), false);
    assert.equal(selection.requestId, "request-a");
    assert.equal(selection.accept(generation, snapshot({ status: "completed", completed_at_ms: 200, response: page() })), true);
});

test("envelope identity rejects device, direction, offset, anchor and task mismatches", () => {
    const current = query({ offset: 50, anchor: "page-one" });
    const matching = snapshot({ offset: 50, anchor: "page-one", status: "completed", response: page() });
    assert.equal(historyResultMatches(current, matching), true);
    for (const changed of [{ device_id: "WinC" }, { direction: "instruction" as const }, { offset: 0 },
        { anchor: "another-anchor" }, { id: "job-b" }]) {
        assert.equal(historyResultMatches(current, { ...matching, ...changed }), false);
    }
});

test("completed page/detail shape and direction must match, while valid empty pages remain distinguishable", () => {
    assert.equal(historyResultMatches(query(), snapshot({ status: "completed", response: page([]) })), true);
    assert.equal(historyResultMatches(query(), snapshot({ status: "completed", response: null })), false);
    assert.equal(historyResultMatches(query(), snapshot({ status: "completed", response: page([row({ direction: "instruction" })]) })), false);
    assert.equal(historyResultMatches(query(), snapshot({ status: "completed", response: detail() })), false);
    const taskQuery = query({ id: "job-b" });
    const task = snapshot({ id: "job-b", status: "completed", response: detail() });
    assert.equal(historyResultMatches(taskQuery, task), true);
    assert.equal(historyResultMatches(taskQuery, { ...task, response: page() }), false);
    assert.equal(historyResultMatches(taskQuery, { ...task, response: detail(row({ id: "job-c" })) }), false);
    assert.equal(historyResultMatches(taskQuery, { ...task, response: detail(row({ direction: "instruction" })) }), false);
});

test("pending, failed and expired snapshots carry no invented result or empty-success state", () => {
    for (const status of ["pending", "failed", "expired"] as const) {
        assert.equal(historyResultMatches(query(), snapshot({ status, response: null, error: status === "failed" ? "device_offline" : null })), true);
    }
    const selection = new HistorySelection();
    const generation = selection.begin(query({ deviceId: "WinC" }));
    assert.equal(selection.accept(generation, snapshot({ status: "failed", error: "old_failure" })), false);
    assert.equal(selection.requestId, null);
});

test("administrator history reads allow an online stopped device but reject offline, revoked or missing devices", () => {
    assert.equal(historyDeviceCanRead(device()), true);
    assert.equal(historyDeviceCanRead(device({ admission: "stopped" })), true);
    assert.equal(historyDeviceCanRead(device({ online: false })), false);
    assert.equal(historyDeviceCanRead(device({ revoked: true })), false);
    assert.equal(historyDeviceCanRead(device({ admission: "revoked" })), false);
    assert.equal(historyDeviceCanRead(undefined), false);
});

test("completed execution remains independent of result delivery at the instruction device", () => {
    const execution = row({ state: "completed", can_stop: false, result_received: false });
    const reply = snapshot({ status: "completed", response: page([execution]), cached: true, device_online: false });
    assert.equal(historyResultMatches(query(), reply), true);
    assert.equal(historyStateLabel(execution.state), "完了");
    assert.equal((reply.response as HistoryPage).rows[0].result_received, false);
    assert.equal(reply.device_online, false);
    assert.equal(reply.cached, true);
    assert.equal(historyStateLabel("unknown"), "状態未確認");
});
