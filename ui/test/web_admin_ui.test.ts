import assert from "node:assert/strict";
import test from "node:test";
import { attachWebManagement } from "../src/web_admin.ts";
import type { WebStatus } from "../src/web_admin.ts";

class NodeFixture {
    id = ""; className = ""; hidden = false; disabled = false; value = ""; textContent = "";
    nodes = new Map<string, NodeFixture>();
    listeners = new Map<string, (() => unknown)[]>();
    private markup = "";
    get innerHTML() { return this.markup; }
    set innerHTML(value: string) {
        this.markup = value;
        for (const match of value.matchAll(/<[^>]+\bid="([^"]+)"[^>]*>/g)) {
            const child = new NodeFixture(); child.id = match[1];
            child.hidden = /\shidden(?:\s|>)/.test(match[0]);
            child.disabled = /\sdisabled(?:\s|>)/.test(match[0]);
            this.nodes.set(child.id, child);
        }
    }
    querySelector(selector: string) { return this.nodes.get(selector.slice(1)) ?? null; }
    addEventListener(name: string, listener: () => unknown) {
        const list = this.listeners.get(name) ?? []; list.push(listener); this.listeners.set(name, list);
    }
    async click() {
        assert.equal(this.disabled, false); assert.equal(this.hidden, false);
        for (const listener of this.listeners.get("click") ?? []) await listener();
    }
}
function deferred<T>() {
    let resolve!: (value: T) => void;
    const promise = new Promise<T>(yes => { resolve = yes; });
    return { resolve, promise };
}
const running: WebStatus = { enabled: true, local_url: "http://127.0.0.1:9080/admin/", remote_url: null };
async function withUi(status: () => Promise<WebStatus>, run: (value: {
    node: (id: string) => NodeFixture; ui: ReturnType<typeof attachWebManagement>; calls: string[]; copied: string[];
    host: () => NodeFixture; setClipboard: (value: unknown) => void;
}) => Promise<void>) {
    const globals = ["fetch", "document", "navigator"] as const;
    const originals = globals.map(key => Object.getOwnPropertyDescriptor(globalThis, key));
    const calls: string[] = []; const copied: string[] = [];
    let host: NodeFixture | null = null;
    const navigator = { clipboard: { writeText: async (text: string) => { copied.push(text); } } as unknown };
    const values = [async (path: string, options?: RequestInit) => {
        if (path === "/admin/session") return Response.json({ csrf: "fixture-csrf", epoch: "fixture-epoch",
            authentication: { configured: true, bootstrap_allowed: false, login_allowed: true, session_id: "human-session", user: { user_id: "admin", display_name: "管理者", administrator: true } } });
        assert.equal(path, "/admin/command");
        const { command, args } = JSON.parse(String(options?.body));
        assert.equal(command, "hub_web_status"); assert.deepEqual(args, {}); calls.push(command);
        return Response.json({ result: await status() });
    }, { createElement: () => new NodeFixture(), getElementById: (id: string) => {
        assert.equal(id, "device-network"); return { before: (node: NodeFixture) => { host = node; } };
    } }, navigator];
    globals.forEach((key, index) => Object.defineProperty(globalThis, key, { configurable: true, writable: true, value: values[index] }));
    try { await run({ ui: attachWebManagement(), node: id => host!.nodes.get(id)!, host: () => host!, calls, copied, setClipboard(value) { navigator.clipboard = value; } }); }
    finally { globals.forEach((key, index) => {
        if (originals[index]) Object.defineProperty(globalThis, key, originals[index]!);
        else delete (globalThis as Record<string, unknown>)[key];
    }); }
}

test("authenticated browser shows server-owned URLs without keys or listener lifecycle controls", async () => {
    await withUi(async () => running, async ({ node, ui, host, copied }) => {
        assert.match(host().innerHTML, /管理者としてログインしたブラウザー/);
        for (const id of ["web-enable", "web-disable", "web-key", "web-logout", "web-open"]) assert.equal(node(id), undefined);
        await ui.refresh(); assert.equal(node("web-enabled-state").textContent, "稼働中");
        assert.equal(node("web-url").value, running.local_url);
        assert.equal(node("web-remote-address").hidden, true);
        const input = node("web-url");
        await node("web-copy-url").click(); await ui.refresh();
        assert.deepEqual(copied, [running.local_url]);
        assert.equal(node("web-url"), input); assert.equal(node("web-status").textContent, "URLをコピーしました。");
    });
});

test("remote HTTP exposes both URLs and missing Clipboard API offers manual selection", async () => {
    await withUi(async () => ({ ...running, remote_url: "http://192.0.2.10:9081/admin/" }), async ({ ui, node, setClipboard }) => {
        await ui.refresh(); assert.equal(node("web-remote-address").hidden, false);
        assert.equal(node("web-remote-url").value, "http://192.0.2.10:9081/admin/");
        setClipboard(undefined); await node("web-copy-remote-url").click();
        assert.match(node("web-status").textContent, /URL欄から選択/);
    });
});

test("slow clipboard completion cannot override a newer server address", async () => {
    const copy = deferred<void>(); let status = running;
    await withUi(async () => status, async ({ ui, node, setClipboard }) => {
        await ui.refresh(); setClipboard({ writeText: () => copy.promise });
        const pending = node("web-copy-url").click();
        status = { ...running, local_url: "http://127.0.0.1:9090/admin/" };
        await ui.refresh(); const message = node("web-status").textContent;
        copy.resolve(); await pending;
        assert.equal(node("web-status").textContent, message);
        assert.equal(node("web-url").value, status.local_url);
    });
});

test("failed status request is visible and a later refresh recovers without a restart command", async () => {
    let fail = true;
    await withUi(async () => { if (fail) throw new Error("offline"); return running; }, async ({ ui, node, calls }) => {
        await ui.refresh(); assert.match(node("web-enabled-state").textContent, /応答を確認できません/);
        fail = false; await ui.refresh(); assert.equal(node("web-enabled-state").textContent, "稼働中");
        assert.deepEqual(calls, ["hub_web_status", "hub_web_status"]);
    });
});
