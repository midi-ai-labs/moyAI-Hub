import { test } from "node:test";
import assert from "node:assert/strict";
import { DiscoveryOwner, availability, capabilityLabel, changeSummary, gatewayStatus, loadCounts, reviewStatus } from "../src/state.ts";
import type { Snapshot } from "../src/state.ts";
test("editing endpoint invalidates in-flight discovery even when returning to same endpoint", () => {
    const owner = new DiscoveryOwner();
    const request = owner.capture();
    owner.invalidate();
    owner.invalidate();
    assert.equal(owner.accepts(request), false);
    assert.equal(owner.accepts(owner.capture()), true);
});
test("status text separates metadata membership, load state and maintenance", () => {
    assert.match(availability.unknown.label, /不明/);
    assert.match(availability.jit_pending.reason, /未ロード/);
    assert.match(availability.jit_pending.reason, /未確認/);
    assert.match(availability.ready.reason, /報告/);
    assert.match(availability.maintenance.reason, /管理者/);
});
test("revision summary carries model removal and capacity review reasons", () => {
    assert.equal(changeSummary({
        revision: "9007199254740993",
        changed_at_ms: 1,
        added_model_ids: [],
        removed_model_ids: ["a"],
        changed_model_ids: [],
        deployments_changed: false,
        policy_changed: true,
        software_changed: false
    }), "モデル削除 1 件 / 実行枠・割当ルールの変更");
});

test("gateway status separates connected responses from upstream settlement after Desktop stops", () => {
    const gateway: NonNullable<Snapshot["gateway"]> = { enabled: true, ready: true, uncertain: false,
        responding: 1, draining: 0, running: 1, reserved: 0, quarantined_pools: [], scope: "gateway_requests_only" };
    assert.equal(gatewayStatus(gateway).label, "応答中 1 件 / 上流終了確認待ち 0 件 / 予約 0 件");
    const stopped = gatewayStatus({ ...gateway, responding: 0, draining: 1 });
    assert.equal(stopped.label, "応答中 0 件 / 上流終了確認待ち 1 件 / 予約 0 件");
    assert.match(stopped.detail, /接続先の処理終了を確認するまで/);
    assert.equal(stopped.attention, true);
    assert.equal(gatewayStatus({ ...gateway, responding: 0, running: 0 }).attention, false);
});

test("unavailable gateway observations and uncertain pools never imply free capacity", () => {
    const gateway: NonNullable<Snapshot["gateway"]> = { enabled: true, ready: false, uncertain: true,
        responding: null, draining: null, running: null, reserved: 0, quarantined_pools: [], scope: "gateway_requests_only" };
    assert.equal(gatewayStatus(gateway).label, "実行状態を確認できません");
    assert.match(gatewayStatus({ ...gateway, responding: 0, draining: 0, running: 0, quarantined_pools: ["gpu"] }).detail, /グループ 1 件.*保留/);
    assert.equal(gatewayStatus({ ...gateway, enabled: false }).label, "実行を保留中");
    assert.equal(gatewayStatus(undefined).label, "モデル実行は停止中");
});

test("load counts show unknown separately without inferring readiness from other states", () => {
    assert.deepEqual(loadCounts([{availability:"unknown"}, {availability:"offline"}, {availability:"jit_pending"}, {availability:"model_missing"}, {availability:"maintenance"}]), {ready:0, unknown:1});
    assert.deepEqual(loadCounts([{availability:"ready"}, {availability:"unknown"}]), {ready:1, unknown:1});
    assert.equal(capabilityLabel("tools"), "ツール利用");
    assert.equal(capabilityLabel("custom_capability"), "custom_capability");
});
test("Main review does not imply Side review and revision text stays precise", () => {
    const contexts = [{context_id:"main",reviewed_revision:"9007199254740993",review_required:false,active_model_id:null}];
    assert.equal(reviewStatus(contexts, "main"), "確認済み r9007199254740993");
    assert.equal(reviewStatus(contexts, "side_chat"), "未確認");
    contexts[0].review_required = true;
    assert.equal(reviewStatus(contexts, "main"), "再確認が必要");
});
