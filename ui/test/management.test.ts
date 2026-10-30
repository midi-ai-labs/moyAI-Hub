import assert from "node:assert/strict";
import test from "node:test";
import { authenticate, copyText, invoke, invokeSharedAdmin, ManagementFailure, managementSession, onAuthenticationChange, reconnect, savedMessage } from "../src/management.ts";
import type { WebSession } from "../src/management.ts";

function session(authenticated = true, csrf = "session-csrf"): WebSession {
    return { csrf, epoch: "listener-epoch", authentication: { configured: true, bootstrap_allowed: false, login_allowed: true,
        session_id: authenticated ? "human-session-a" : null,
        user: authenticated ? { user_id: "admin-a", display_name: "管理者A", administrator: true } : null } };
}
function deferred<T>() {
    let resolve!: (value: T) => void;
    const promise = new Promise<T>(yes => { resolve = yes; });
    return { promise, resolve };
}
async function withFetch(fetcher: typeof fetch, run: () => Promise<void>) {
    const original = globalThis.fetch;
    globalThis.fetch = fetcher;
    try { await run(); } finally { globalThis.fetch = original; }
}
test("an administrator changing their own authority immediately loses cached management access", async () => {
    const changes: (WebSession | null)[] = [];
    let dispatched = 0;
    const dispose = onAuthenticationChange(value => changes.push(value));
    try {
        await withFetch(async input => {
            if (input === "/admin/session") return Response.json(session());
            dispatched += 1; return Response.json({ result: { reauthenticate: true } });
        }, async () => {
            await reconnect();
            await assert.rejects(invokeSharedAdmin("hub_shared_command", {}), error => error instanceof ManagementFailure && error.status === 401);
            assert.equal(changes.at(-1), null);
            await assert.rejects(invokeSharedAdmin("hub_shared_snapshot"));
            assert.equal(dispatched, 1);
        });
    } finally { dispose(); }
});

test("authenticated commands use CSRF and never replay an uncertain mutation", async () => {
    const calls: { path: string; options?: RequestInit }[] = [];
    let mode = "ok";
    await withFetch(async (input, options) => {
        const path = String(input); calls.push({ path, options });
        assert.equal(options?.credentials, "same-origin");
        assert.ok(options?.signal instanceof AbortSignal);
        if (path === "/admin/session") return Response.json(session());
        assert.equal(path, "/admin/command");
        assert.equal(new Headers(options?.headers).get("x-moyai-csrf"), "session-csrf");
        assert.equal(new Headers(options?.headers).get("x-moyai-session"), "human-session-a");
        if (mode === "expired") return Response.json({ error: "session expired" }, { status: 401 });
        if (mode === "lost") throw new Error("connection reset after send");
        return Response.json({ result: { revision: "8" } });
    }, async () => {
        await reconnect();
        assert.deepEqual(await invoke("hub_network_group", { expectedRevision: "7", label: "Team" }), { revision: "8" });
        assert.deepEqual(JSON.parse(String(calls[1].options?.body)), { command: "hub_network_group", args: { expectedRevision: "7", label: "Team" } });
        mode = "expired";
        await assert.rejects(invoke("hub_network_group"), error => error === "session expired");
        await assert.rejects(invoke("hub_network_group"), error => typeof error === "string" && error.includes("管理画面の利用許可が変わりました"));
        assert.equal(calls.length, 3, "revoked polling does not request another session or replay the command");
        mode = "lost";
        await reconnect();
        await assert.rejects(invoke("hub_network_group"), error => typeof error === "string" && error.includes("自動で再実行されません"));
        assert.equal(calls.filter(call => call.path === "/admin/command").length, 3);
        assert.match(savedMessage("result.md"), /^ダウンロードを開始しました/);
    });
});

test("anonymous or non-administrator sessions cannot dispatch management commands", async () => {
    for (const value of [session(false), { ...session(), authentication: { ...session().authentication,
        user: { user_id: "member", display_name: "利用者", administrator: false } } }]) {
        const paths: string[] = [];
        await withFetch(async input => { paths.push(String(input)); return Response.json(value); }, async () => {
            await reconnect();
            await assert.rejects(invoke("hub_snapshot"), error => typeof error === "string" && error.includes("Hub PCから管理画面を開いて"));
            assert.deepEqual(paths, ["/admin/session"]);
        });
    }
});

test("a remote action requiring the Hub PC preserves its valid management session", async () => {
    const changes: (WebSession | null)[] = [];
    const dispose = onAuthenticationChange(value => changes.push(value));
    let calls = 0;
    try {
        await withFetch(async input => {
            if (input === "/admin/session") return Response.json({...session(),authentication:{...session().authentication,local_management:false}});
            ++calls;
            return calls === 1 ? Response.json({error:"Hub PCでこの操作を行ってください。",code:"requires_hub_pc"},{status:403}) : Response.json({result:{revision:"7"}});
        }, async () => {
            await reconnect(); changes.length=0;
            await assert.rejects(invoke("hub_discover"), error => typeof error === "string" && error.includes("Hub PC"));
            assert.equal(changes.includes(null),false);
            assert.deepEqual(await invokeSharedAdmin("hub_shared_snapshot"),{revision:"7"});
            assert.equal(calls,2);
        });
    } finally { dispose(); }
});

test("logout immediately revokes pending management results before the server responds", async () => {
    const command = deferred<Response>(); const logout = deferred<Response>(); const sent = deferred<void>();
    const changes: (WebSession | null)[] = [];
    const dispose = onAuthenticationChange(value => changes.push(value));
    try {
        await withFetch(async input => {
            if (input === "/admin/session") return Response.json(session());
            if (input === "/admin/logout") { sent.resolve(); return logout.promise; }
            return command.promise;
        }, async () => {
            await reconnect();
            const pending = invoke("hub_snapshot");
            await new Promise(resolve => setImmediate(resolve));
            const signingOut = authenticate("logout");
            await sent.promise;
            assert.equal(changes.at(-1), null);
            const rejected = assert.rejects(pending, error => typeof error === "string" && error.includes("管理画面の利用許可が変わりました"));
            command.resolve(Response.json({ result: { confidential: "old owner" } }));
            await rejected;
            logout.resolve(Response.json(session(false)));
            assert.equal((await signingOut).authentication.user, null);
            await assert.rejects(invoke("hub_snapshot"));
        });
    } finally { dispose(); }
});

test("an older session check cannot replace a newer anonymous session", async () => {
    const old = deferred<Response>(); let calls = 0;
    await withFetch(async () => ++calls === 1 ? old.promise : Response.json(session(false)), async () => {
        const first = managementSession(true);
        const rejected = assert.rejects(first, error => typeof error === "string" && error.includes("管理画面の利用許可が変わりました"));
        assert.equal((await managementSession(true)).authentication.user, null);
        old.resolve(Response.json(session()));
        await rejected;
        assert.equal((await managementSession()).authentication.user, null);
    });
});

test("logout invalidates a queued command before its cached session continuation dispatches", async () => {
    let commands = 0;
    await withFetch(async input => {
        if (input === "/admin/session") return Response.json(session());
        if (input === "/admin/logout") return Response.json(session(false));
        ++commands; return Response.json({ result: {} });
    }, async () => {
        await reconnect();
        const queued = invoke("hub_network_group", { label: "should not be sent" });
        const rejected = assert.rejects(queued);
        await authenticate("logout");
        await rejected;
        assert.equal(commands, 0);
    });
});

test("launch access sends its ticket only to the access endpoint and retains no ticket in session state", async () => {
    const calls: { path: string; options?: RequestInit }[] = [];
    await withFetch(async (input, options) => {
        calls.push({ path: String(input), options });
        return Response.json(input === "/admin/session" ? { ...session(false), authentication: {
            configured: false, bootstrap_allowed: true, login_allowed: true, session_id: null, user: null } } : session());
    }, async () => {
        await reconnect();
        const credentials = { ticket: "one-use-launch-ticket" };
        const accepted = await authenticate("access", credentials);
        assert.equal(calls[2].path, "/admin/access");
        assert.equal(new Headers(calls[2].options?.headers).get("x-moyai-csrf"), "session-csrf");
        assert.deepEqual(JSON.parse(String(calls[2].options?.body)), credentials);
        assert.equal(accepted.authentication.user?.administrator, true);
        assert.equal(JSON.stringify(await managementSession()).includes(credentials.ticket), false);
    });
});

test("an uncertain logout leaves commands blocked until an explicit session check", async () => {
    let commands = 0;
    await withFetch(async input => {
        if (input === "/admin/session") return Response.json(session());
        if (input === "/admin/logout") throw new Error("offline");
        ++commands; return Response.json({ result: {} });
    }, async () => {
        await reconnect();
        await assert.rejects(authenticate("logout"));
        await assert.rejects(invoke("hub_snapshot"));
        assert.equal(commands, 0);
        await reconnect(); await invoke("hub_snapshot");
        assert.equal(commands, 1);
    });
});

test("legacy sessions without human authentication fail closed", async () => {
    await withFetch(async () => Response.json({ csrf: "csrf", epoch: "epoch" }), async () => {
        await assert.rejects(managementSession(true), error => typeof error === "string" && error.includes("認証情報を確認できません"));
    });
});

test("a listener that disallows access receives no ticket or management commands", async () => {
    const paths: string[] = [];
    await withFetch(async input => {
        paths.push(String(input));
        return Response.json({ ...session(), authentication: { ...session().authentication, login_allowed: false } });
    }, async () => {
        await reconnect();
        await assert.rejects(invoke("hub_snapshot"));
        await assert.rejects(authenticate("access", { ticket: "sensitive-ticket" }),
            error => typeof error === "string" && error.includes("Hub PC"));
        assert.deepEqual(paths, ["/admin/session", "/admin/session"]);
    });
});

test("CSRF preparation for launch access never re-admits the preceding cookie session", async () => {
    const changes: (WebSession | null)[] = [];
    const dispose = onAuthenticationChange(value => changes.push(value));
    try {
        await withFetch(async input => {
            if (input === "/admin/session") return Response.json(session());
            if (input === "/admin/logout") throw new Error("uncertain logout");
            return Response.json({ error: "invalid_credentials" }, { status: 401 });
        }, async () => {
            await reconnect();
            await assert.rejects(authenticate("logout"));
            changes.length = 0;
            await assert.rejects(authenticate("access", { ticket: "expired-ticket" }));
            assert.deepEqual(changes, [null]);
            await assert.rejects(invoke("hub_snapshot"));
        });
    } finally { dispose(); }
});

test("reopening a tab after Hub restart uses the new process CSRF", async () => {
    let restarted = false;
    const changes: (WebSession | null)[] = [];
    const dispose = onAuthenticationChange(value => changes.push(value));
    try {
        await withFetch(async (input, options) => {
            const value = { ...session(true, restarted ? "new-csrf" : "old-csrf"), epoch: restarted ? "new" : "old" };
            if (input === "/admin/session") return Response.json(value);
            assert.equal(new Headers(options?.headers).get("x-moyai-csrf"), "new-csrf");
            return Response.json(value);
        }, async () => {
            await reconnect(); restarted = true; changes.length = 0;
            const result = await authenticate("access", { ticket: "new-process-ticket" });
            assert.equal(result.epoch, "new");
            assert.deepEqual(changes.map(value => value?.epoch ?? null), [null, "new"]);
        });
    } finally { dispose(); }
});

test("missing Clipboard API offers the caller a selection fallback", async () => {
    const original = Object.getOwnPropertyDescriptor(globalThis, "navigator");
    Object.defineProperty(globalThis, "navigator", { configurable: true, value: {} });
    try { await assert.rejects(copyText("http://hub.test/admin/"), /クリップボードを利用できません/); }
    finally {
        if (original) Object.defineProperty(globalThis, "navigator", original);
        else delete (globalThis as Record<string, unknown>).navigator;
    }
});
