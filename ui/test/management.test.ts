import assert from "node:assert/strict";
import test from "node:test";
import { copyText, invoke, reconnect, savedMessage } from "../src/management.ts";

test("browser management uses anonymous CSRF and never replays an uncertain mutation", async () => {
    const names = ["fetch", "navigator"] as const;
    const originals = names.map(name => Object.getOwnPropertyDescriptor(globalThis, name));
    const replace = (name: string, value: unknown) => Object.defineProperty(globalThis, name, { configurable: true, writable: true, value });
    const calls: { path: string; options?: RequestInit }[] = [];
    try {
        let rejectMutation = false;
        replace("fetch", async (path: string, options?: RequestInit) => {
            calls.push({ path, options });
            assert.equal(options?.credentials, "same-origin");
            assert.ok(options?.signal instanceof AbortSignal);
            if (path === "/admin/session") return Response.json({ csrf: "session-csrf" });
            assert.equal(path, "/admin/command");
            assert.equal(new Headers(options?.headers).get("x-moyai-csrf"), "session-csrf");
            if (rejectMutation) return Response.json({ error: "csrf expired" }, { status: 403 });
            return Response.json({ result: { revision: "8" } });
        });
        assert.deepEqual(await invoke("hub_network_group", { expectedRevision: "7", label: "Team" }), { revision: "8" });
        assert.deepEqual(JSON.parse(String(calls[1].options?.body)), { command: "hub_network_group", args: { expectedRevision: "7", label: "Team" } });
        rejectMutation = true;
        await assert.rejects(invoke("hub_network_group", { expectedRevision: "8", label: "New" }), error => error === "csrf expired");
        assert.deepEqual(calls.map(call => call.path), ["/admin/session", "/admin/command", "/admin/command"]);
        replace("fetch", async (path: string, options?: RequestInit) => {
            calls.push({ path, options });
            if (path === "/admin/session") return Response.json({ csrf: "new-session" });
            throw new Error("connection reset after send");
        });
        await assert.rejects(invoke("hub_network_group", { expectedRevision: "8", label: "New" }), error => typeof error === "string" && error.includes("自動で再実行されません"));
        assert.deepEqual(calls.slice(3).map(call => call.path), ["/admin/session", "/admin/command"]);
        assert.match(savedMessage("result.md"), /^ダウンロードを開始しました/);
        await reconnect();
        assert.equal(calls.at(-1)?.path, "/admin/session");
        assert.equal(calls.filter(call => call.path === "/admin/command").length, 3, "reconnect does not start a process or replay an operation");
        replace("navigator", {});
        await assert.rejects(copyText("http://hub.test/admin/"), /クリップボードを利用できません/);
    } finally {
        names.forEach((name, index) => {
            if (originals[index]) Object.defineProperty(globalThis, name, originals[index]!);
            else delete (globalThis as Record<string, unknown>)[name];
        });
    }
});
