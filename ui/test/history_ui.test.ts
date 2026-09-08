import assert from "node:assert/strict";
import test from "node:test";
import { attachHistory } from "../src/history.ts";
import type { HistoryRow, HistorySnapshot } from "../src/history_state.ts";
import type { NetworkSnapshot } from "../src/network_state.ts";

class NodeFixture {
    id = ""; hidden = false; disabled = false; value = ""; textContent = "";
    dataset: Record<string, string> = {};
    private markup = "";
    nodes = new Map<string, NodeFixture>();
    buttons: NodeFixture[] = [];
    listeners = new Map<string, ((event: { target: NodeFixture }) => unknown)[]>();
    classList = { toggle(_name: string, _enabled: boolean) {} };
    get innerHTML() { return this.markup; }
    set innerHTML(value: string) {
        this.markup = value;
        for (const match of value.matchAll(/<[^>]+\bid="([^"]+)"[^>]*>/g)) {
            if (!this.nodes.has(match[1])) {
                const child = new NodeFixture(); child.id = match[1]; child.hidden = /\shidden(?:\s|>)/.test(match[0]);
                child.disabled = /\sdisabled(?:\s|>)/.test(match[0]); this.nodes.set(match[1], child);
            }
        }
        this.buttons = Array.from(value.matchAll(/<button[^>]*data-history-detail="([^"]+)"[^>]*>/g), match => {
            const button = new NodeFixture(); button.dataset.historyDetail = match[1]; return button;
        });
        if (this.id === "history-device") {
            const options = Array.from(value.matchAll(/<option value="([^"]*)"/g), match => match[1]);
            if (!options.includes(this.value)) this.value = "";
        }
    }
    querySelector(selector: string) { return this.nodes.get(selector.slice(1)) ?? null; }
    querySelectorAll(selector: string) { return selector === "button" ? this.buttons : []; }
    replaceChildren() { this.markup = ""; this.buttons = []; }
    closest() { return this; }
    addEventListener(name: string, listener: (event: { target: NodeFixture }) => unknown) {
        const list = this.listeners.get(name) ?? []; list.push(listener); this.listeners.set(name, list);
    }
    fire(name: string, target = this) { for (const listener of this.listeners.get(name) ?? []) listener({ target }); }
}
function deferred<T>() {
    let resolve!: (value: T) => void;
    const promise = new Promise<T>(yes => { resolve = yes; }); return { resolve, promise };
}
function row(direction: HistoryRow["direction"] = "instruction"): HistoryRow {
    return { id: "job-b", direction, created_at_ms: 1000, updated_at_ms: null, title: '<script>bad()</script> "CPU"',
        peer_label: "WinA<img>", target_label: "temp", state: "completed", stop_status: "none", state_source: direction === "instruction" ? "last_observed" : "local_runtime",
        result_received: false, session_id: "chat-b", profile_id: "profile-b", job_id: "job-b", root_task_id: "chat-a", device_path: ["WinA", "WinB"], can_stop: false };
}
function response(args: Record<string, unknown>, overrides: Partial<HistorySnapshot> = {}): HistorySnapshot {
    const direction = args.direction as HistoryRow["direction"];
    return { request_id: args.id ? "detail-request" : `list-${direction}`, device_id: args.deviceId as string,
        device_label: "WinB", direction, offset: args.offset as number, anchor: args.anchor as string | null, id: args.id as string | null,
        status: "completed", requested_at_ms: 1000, completed_at_ms: 1000, expires_at_ms: 61_000,
        response: args.id ? { row: row(direction), markdown: "# 履歴\n<script>bad()</script>", truncated: false }
            : { rows: [row(direction)], next_offset: null, anchor: "anchor-a" },
        error: null, device_online: true, device_admission: "allowed", cached: true, ...overrides };
}
function network(online = true): NetworkSnapshot {
    return { devices: [{ device_id: "WinB", label: "WinB", online, admission: "allowed", revoked: false }],
        revision: "1" } as NetworkSnapshot;
}
const flush = () => new Promise<void>(resolve => setImmediate(resolve));
async function withUi(invoke: (name: string, args: Record<string, unknown>) => Promise<unknown>, run: (value: {
    host: NodeFixture; node: (id: string) => NodeFixture; ui: ReturnType<typeof attachHistory>;
    calls: { name: string; args: Record<string, unknown> }[]; tick: () => Promise<void>; setNow: (value: number) => void;
}) => Promise<void>) {
    const globals = ["window", "document", "location", "setTimeout", "clearTimeout"] as const;
    const originals = globals.map(key => Object.getOwnPropertyDescriptor(globalThis, key));
    const oldNow = Date.now; let now = 1000; Date.now = () => now;
    const calls: { name: string; args: Record<string, unknown> }[] = [];
    const timers = new Map<number, () => unknown>(); let nextTimer = 1;
    const values = [{ __TAURI_INTERNALS__: { invoke: async (name: string, args: Record<string, unknown>) => { calls.push({ name, args }); return invoke(name, args); } } },
        { activeElement: null }, { hash: "" }, (callback: () => unknown) => { const id = nextTimer++; timers.set(id, callback); return id; },
        (id: number) => timers.delete(id)];
    globals.forEach((key, index) => Object.defineProperty(globalThis, key, { configurable: true, writable: true, value: values[index] }));
    const host = new NodeFixture();
    try {
        const ui = attachHistory(host as unknown as HTMLElement);
        const node = (id: string) => host.nodes.get(id)!;
        node("history-direction").value = "instruction";
        await run({ host, node, ui, calls, setNow(value) { now = value; }, async tick() {
            const first = timers.entries().next().value; if (first) { timers.delete(first[0]); first[1](); } await flush();
        } });
    } finally {
        Date.now = oldNow;
        globals.forEach((key, index) => { if (originals[index]) Object.defineProperty(globalThis, key, originals[index]!); else delete (globalThis as Record<string, unknown>)[key]; });
    }
}

test("Hub pending direction change ignores an old in-flight result and releases Full errors for retry", async () => {
    const oldPoll = deferred<HistorySnapshot>(); let executionRequests = 0;
    await withUi(async (name, args) => {
        if (name === "hub_history_result") return oldPoll.promise;
        if (args.direction === "instruction") return response(args, { status: "pending", response: null, completed_at_ms: null });
        if (++executionRequests === 1) throw "この端末の履歴を取得中です。完了または期限切れを待ってから再取得してください。";
        return response(args);
    }, async ({ ui, node, tick, calls }) => {
        ui.updateNetwork(network()); ui.showDevice("WinB"); await flush();
        assert.equal(node("history-request").disabled, true);
        await tick();
        node("history-direction").value = "execution"; node("history-direction").fire("change");
        node("history-request").fire("click"); await flush();
        assert.match(node("history-notice").textContent, /取得中.*期限切れ/);
        assert.equal(node("history-request").disabled, false);
        oldPoll.resolve(response({ deviceId: "WinB", direction: "instruction", offset: 0, anchor: null, id: null })); await flush();
        assert.equal(node("history-rows").innerHTML, "");
        node("history-request").fire("click"); await flush();
        assert.match(node("history-rows").innerHTML, /実行側の保存記録/);
        assert.match(node("history-rows").innerHTML, /指示側の結果受信は、この端末では未確認/);
        assert.equal(node("history-request").disabled, false);
        assert.deepEqual(calls.map(call => call.name), ["hub_history_request", "hub_history_result", "hub_history_request", "hub_history_request"]);
    });
});

test("Hub cached offline details expire consistently, escape markup, and export the selected immutable request", async () => {
    const exportResult = deferred<string | null>();
    await withUi(async (name, args) => name === "hub_history_export" ? exportResult.promise : response(args), async ({ ui, node, calls, setNow }) => {
        ui.updateNetwork(network()); ui.showDevice("WinB"); await flush();
        const markup = node("history-rows").innerHTML;
        assert.match(markup, /&lt;script&gt;/); assert.match(markup, /WinA&lt;img&gt;/);
        assert.doesNotMatch(markup, /<script>|<img>/);
        node("history-rows").fire("click", node("history-rows").buttons[0]); await flush();
        assert.equal(node("history-markdown").textContent, "# 履歴\n<script>bad()</script>");
        node("history-export").fire("click"); await flush();
        assert.deepEqual(calls.at(-1), { name: "hub_history_export", args: { requestId: "detail-request" } });
        node("history-direction").value = "execution"; node("history-direction").fire("change");
        exportResult.resolve("C:/logs/old-detail.md"); await flush();
        assert.doesNotMatch(node("history-notice").textContent, /保存しました/);
        node("history-direction").value = "instruction"; node("history-direction").fire("change");
        ui.updateNetwork(network(false));
        assert.match(node("history-acquired").textContent, /未接続.*スナップショット/);
        assert.equal(node("history-request").disabled, true);
        const beforeOffline = calls.length;
        node("history-rows").fire("click", node("history-rows").buttons[0]); await flush();
        assert.equal(calls.length, beforeOffline);
        assert.equal(node("history-detail").hidden, false);
        assert.match(node("history-detail-acquired").textContent, /未接続.*スナップショット/);
        setNow(1000 + 30 * 60_000); ui.updateNetwork(network(false));
        assert.equal(node("history-detail").hidden, true);
        assert.equal(node("history-markdown").textContent, "");
        assert.equal(node("history-export").disabled, true);
        assert.equal(node("history-rows").innerHTML, "");
        assert.match(node("history-notice").textContent, /保持期限/);
    });
});

test("Hub errors and target mismatches never become a successful empty page", async () => {
    let number = 0;
    await withUi(async (_name, args) => ++number === 1 ? response(args, { device_id: "WinC" })
        : number === 2 ? response(args, { status: "expired", response: null, error: "history_request_expired" })
        : response(args, { response: { rows: [], anchor: null, next_offset: null } }), async ({ ui, node }) => {
        ui.updateNetwork(network()); ui.showDevice("WinB"); await flush();
        assert.match(node("history-notice").textContent, /対象が一致しません/);
        assert.equal(node("history-empty").hidden, true);
        assert.equal(node("history-request").disabled, false);
        assert.equal(node("history-rows").innerHTML, "");
        node("history-request").fire("click"); await flush();
        assert.match(node("history-notice").textContent, /時間内に応答がありません/);
        assert.equal(node("history-empty").hidden, true);
        assert.equal(node("history-request").disabled, false);
        node("history-request").fire("click"); await flush();
        assert.equal(node("history-empty").hidden, false);
        assert.equal(node("history-page-label").textContent, "0 件");
        assert.match(node("history-notice").textContent, /履歴を取得しました/);
    });
});
