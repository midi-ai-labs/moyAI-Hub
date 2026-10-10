import { test } from "node:test";
import assert from "node:assert/strict";
import { DiscoveryOwner, availability, capabilityLabel, changeSummary, deploymentStatus, gatewayStatus, modelCounts, reviewStatus } from "../src/state.ts";
import type { Snapshot } from "../src/state.ts";
test("editing endpoint invalidates in-flight discovery even when returning to same endpoint", () => {
    const owner = new DiscoveryOwner();
    const request = owner.capture();
    owner.invalidate();
    owner.invalidate();
    assert.equal(owner.accepts(request), false);
    assert.equal(owner.accepts(owner.capture()), true);
});
test("primary model status uses fresh listing presence independent of provider load hints", () => {
    for (const hint of ["unknown", "ready", "jit_pending"] as const) {
        assert.equal(deploymentStatus({ availability: hint, model_present: true }).label, "一覧あり");
        assert.equal(deploymentStatus({ availability: hint, model_present: false }).label, "未確認");
    }
    assert.equal(deploymentStatus({ availability: "maintenance", model_present: true }), availability.maintenance);
    assert.equal(deploymentStatus({ availability: "offline", model_present: false }), availability.offline);
    assert.equal(deploymentStatus({ availability: "model_missing", model_present: false }), availability.model_missing);
});
test("previously checked models await recheck without being described as never checked", () => {
    for (const hint of ["unknown", "ready", "jit_pending"] as const) {
        for (const observed of [0, 1_000]) {
            const row = { availability: hint, model_present: false, last_observed_ms: observed };
            const status = deploymentStatus(row);
            assert.equal(status.label, "再確認待ち");
            assert.match(status.reason, /確認結果が古い/);
            assert.match(status.reason, /直近の確認に失敗/);
            assert.match(status.reason, /実行前に再確認/);
            assert.doesNotMatch(status.reason, /まだ確認できていません/);
            assert.equal(deploymentStatus({ ...row, model_present: true }), availability.ready);
        }
        assert.equal(deploymentStatus({ availability: hint, model_present: false, last_observed_ms: null }), availability.unknown);
    }
    for (const hint of ["offline", "model_missing", "maintenance"] as const) {
        assert.equal(deploymentStatus({ availability: hint, model_present: false, last_observed_ms: 1_000 }), availability[hint]);
    }
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
    }), "モデル削除 1 件 / 同時接続数と接続先の選び方の変更");
});

test("gateway status reports current Hub communications without claiming provider execution state", () => {
    const gateway: NonNullable<Snapshot["gateway"]> = { enabled: true, ready: true, uncertain: false,
        responding: 1, draining: 0, running: 1, reserved: 0, quarantined_pools: [], scope: "gateway_requests_only" };
    assert.equal(gatewayStatus(gateway).label, "中継中 1 件 / 予約 0 件");
    const closing = gatewayStatus({ ...gateway, responding: 0, draining: 1, reserved: 2 });
    assert.equal(closing.label, "中継中 1 件 / 予約 2 件");
    assert.match(closing.detail, /ホスト側/);
    assert.equal(closing.attention, false);
    assert.equal(gatewayStatus({ ...gateway, responding: 0, running: 0 }).attention, false);
});

test("unavailable gateway observation is a Hub communication error and stopped state needs no provider acknowledgement", () => {
    const gateway: NonNullable<Snapshot["gateway"]> = { enabled: true, ready: false, uncertain: true,
        responding: null, draining: null, running: null, reserved: 0, quarantined_pools: [], scope: "gateway_requests_only" };
    assert.equal(gatewayStatus(gateway).label, "Hub の中継状態を確認できません");
    assert.match(gatewayStatus(gateway).detail, /中継プロセスとの通信/);
    assert.equal(gatewayStatus(gateway).attention, true);
    assert.equal(gatewayStatus({ ...gateway, enabled: false }).label, "Hub のモデル中継は停止中");
    assert.equal(gatewayStatus({ ...gateway, enabled: false }).attention, false);
    assert.equal(gatewayStatus({ ...gateway, enabled: false, stopping: true }).label, "Hub のモデル中継を停止中");
    assert.equal(gatewayStatus(undefined).label, "Hub のモデル中継は停止中");
});

test("model counts report catalog presence without inferring loaded model counts", () => {
    assert.deepEqual(modelCounts([{model_present:true}, {model_present:false}, {model_present:true}]), {present:2, unconfirmed:1});
    assert.deepEqual(modelCounts([]), {present:0, unconfirmed:0});
    assert.equal(capabilityLabel("tools"), "ツール利用");
    assert.equal(capabilityLabel("custom_capability"), "custom_capability");
});
test("Main review does not imply Side review and revision text stays precise", () => {
    const contexts = [{context_id:"main",reviewed_revision:"9007199254740993",review_required:false,active_model_id:null}];
    assert.equal(reviewStatus(contexts, "main"), "確認済み（更新 9007199254740993）");
    assert.equal(reviewStatus(contexts, "side_chat"), "未確認");
    assert.equal(reviewStatus(contexts, "approve"), "未確認");
    contexts[0].review_required = true;
    assert.equal(reviewStatus(contexts, "main"), "再確認が必要");
});
