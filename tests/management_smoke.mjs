// Actual Tauri/WebView2 smoke. Requires only Node.js; no product commands are mocked.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { createServer as createTcpServer } from "node:net";
import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { mkdir, readFile, writeFile, readdir } from "node:fs/promises";
import path from "node:path";
const args = Object.fromEntries(process.argv.slice(2).reduce((pairs, value, index, list) => index % 2 ? [...pairs, [list[index - 1], value]] : pairs, []));
if (!args["--binary"] || !args["--artifacts"] || !path.isAbsolute(args["--binary"]) || !path.isAbsolute(args["--artifacts"]))
    throw new Error("Use --binary ABS --artifacts ABS (fresh directory)");
const root = args["--artifacts"], binary = args["--binary"], started = Date.now();
await mkdir(root, {
    recursive: false
});
const data = path.join(root, "data"), webview = path.join(root, "webview");
await mkdir(data);
await mkdir(webview);
const results = {
    started_at: new Date(started).toISOString(),
    checks: [],
    launches: [],
    limits: ["WebView trusted pointer/key input, composition start/update, and pending text/focus are exercised. Composition completion trust is recorded separately; native OS IME candidate-window behavior, physical keyboard and native pointer remain manual_pending. See composition.status for protocol limitations.", "Cleanup verifies the exact spawned Hub process and fixture listener; WebView2 descendant process zero requires the external profile-owner audit.", "Desktop runtime generation routing, LAN hosting, provider permit enforcement and durable pairing remain unimplemented."]
};
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(predicate, label, timeout = 15000) {
    const end = Date.now() + timeout;
    while (Date.now() < end) {
        const value = await predicate();
        if (value)
            return value;
        await sleep(80);
    }
    throw new Error(`Timed out: ${label}`);
}
function pass(name) {
    results.checks.push(name);
    process.stdout.write(`PASS ${name}\n`);
}
let fixtureMode = "normal", child = null, cdp = null, childExited = false;
const requests = [];
const fixture = createServer((request, response) => {
    requests.push(request.url);
    const respond = () => {
        if (fixtureMode === "invalid") {
            response.writeHead(200, {
                "content-type": "application/json"
            });
            response.end('{"broken":true}');
            return;
        }
        const alpha = {
            key: "alpha",
            id: "alpha",
            type: "llm",
            display_name: "Alpha",
            loaded_instances: [{}],
            capabilities: {
                trained_for_tool_use: true
            }
        };
        const beta = {
            key: "beta",
            id: "beta",
            type: "llm",
            display_name: "Beta",
            loaded_instances: [],
            capabilities: {
                trained_for_tool_use: true
            }
        };
        const rows = fixtureMode === "missing" ? [beta] : [alpha, beta];
        response.writeHead(200, {
            "content-type": "application/json"
        });
        response.end(JSON.stringify(request.url.endsWith("/api/v1/models") ? {
            models: rows
        } : {
            data: rows
        }));
    };
    if (request.url.startsWith("/slow/"))
        setTimeout(respond, 1500);
    else
        respond();
});
await new Promise(resolve => fixture.listen(0, "127.0.0.1", resolve));
const endpoint = `127.0.0.1:${fixture.address().port}`;
class Cdp {
    constructor(socket) {
        this.socket = socket;
        this.next = 0;
        this.pending = new Map();
        socket.addEventListener("message", event => {
            const value = JSON.parse(event.data);
            const waiting = this.pending.get(value.id);
            if (waiting) {
                this.pending.delete(value.id);
                clearTimeout(waiting.timer);
                value.error ? waiting.reject(Object.assign(new Error(value.error.message), { code: value.error.code })) : waiting.resolve(value.result);
            }
        });
    }
    static async connect(url) {
        const socket = new WebSocket(url);
        await new Promise((resolve, reject) => {
            socket.addEventListener("open", resolve, {
                once: true
            });
            socket.addEventListener("error", reject, {
                once: true
            });
        });
        return new Cdp(socket);
    }
    send(method, params = {}) {
        const id = ++this.next;
        return new Promise((resolve, reject) => {
            const timer = setTimeout(() => {
                this.pending.delete(id);
                reject(new Error(`CDP deadline: ${method}`));
            }, 10000);
            this.pending.set(id, {
                resolve,
                reject,
                timer
            });
            this.socket.send(JSON.stringify({
                id,
                method,
                params
            }));
        });
    }
    async evaluate(expression) {
        const result = await this.send("Runtime.evaluate", {
            expression,
            returnByValue: true,
            awaitPromise: true
        });
        if (result.exceptionDetails) {
            const details = result.exceptionDetails;
            throw new Error(details.exception?.description ?? details.exception?.value ?? details.text);
        }
        return result.result.value;
    }
    close() {
        for (const { reject, timer } of this.pending.values()) {
            clearTimeout(timer);
            reject(new Error("CDP closed"));
        }
        this.pending.clear();
        this.socket.close();
    }
}
async function findPort(directory) {
    for (const item of await readdir(directory, {
        withFileTypes: true
    })) {
        const current = path.join(directory, item.name);
        if (item.isDirectory()) {
            const found = await findPort(current);
            if (found)
                return found;
        }
        else if (item.name === "DevToolsActivePort")
            return Number((await readFile(current, "utf8")).split(/\r?\n/)[0]);
    }
    return null;
}
async function launch(generation) {
    const profile = path.join(webview, String(generation));
    await mkdir(profile);
    childExited = false;
    child = spawn(binary, [], {
        cwd: root,
        windowsHide: true,
        stdio: ["ignore", "pipe", "pipe"],
        env: {
            ...process.env,
            MOYAI_HUB_DATA_DIR: data,
            WEBVIEW2_USER_DATA_FOLDER: profile,
            WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=0"
        }
    });
    const output = [];
    child.stderr.on("data", chunk => output.push(chunk.toString()));
    child.stdout.resume();
    child.once("exit", () => {
        childExited = true;
    });
    await new Promise((resolve, reject) => {
        child.once("spawn", resolve);
        child.once("error", reject);
    });
    const launchRecord = { generation, pid: child.pid, profile, binary, launched_at: new Date().toISOString() };
    results.launches.push(launchRecord);
    const port = await until(async () => {
        if (childExited)
            throw new Error(`Hub exited during launch: ${output.join("")}`);
        return findPort(profile);
    }, "WebView2 endpoint", 30000);
    launchRecord.cdp_port = port;
    const target = await until(async () => {
        try {
            const values = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
            const pages = values.filter(value => value.type === "page" && (value.url.startsWith("http://tauri.localhost") || value.url.startsWith("tauri://localhost")));
            assert.ok(pages.length <= 1);
            return pages[0];
        }
        catch {
            return null;
        }
    }, "Tauri target", 20000);
    const targetUrl = new URL(target.webSocketDebuggerUrl);
    assert.equal(targetUrl.hostname, "127.0.0.1");
    assert.equal(Number(targetUrl.port), port);
    cdp = await Cdp.connect(target.webSocketDebuggerUrl);
    await cdp.send("Runtime.enable");
    await cdp.send("Page.enable");
    await until(() => cdp.evaluate("(()=>{const value=document.getElementById('metric-revision')?.textContent?.trim();return Boolean(value && value !== '—');})()"), "initial snapshot");
}
async function stopProcess() {
    if (cdp) {
        try {
            await cdp.send("Page.close");
        }
        catch {
        }
        cdp.close();
        cdp = null;
    }
    if (child && !childExited) {
        // CloseMainWindow requests the native Tauri owner to exit, without taking foreground.
        const closer = spawn("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", `$p = Get-Process -Id ${child.pid} -ErrorAction SilentlyContinue; if ($p) { [void]$p.CloseMainWindow() }`], {
            windowsHide: true,
            stdio: "ignore"
        });
        await new Promise(resolve => closer.once("exit", resolve));
        const end = Date.now() + 4000;
        while (!childExited && Date.now() < end)
            await sleep(80);
        if (!childExited) {
            const killer = spawn("taskkill.exe", ["/PID", String(child.pid), "/T", "/F"], {
                windowsHide: true,
                stdio: "ignore"
            });
            await new Promise(resolve => killer.once("exit", resolve));
        }
        await until(() => childExited, "exact Hub process cleanup", 5000);
    }
}
async function key(key, code = key, modifiers = 0) {
    const virtual = {
        Tab: 9,
        Enter: 13,
        Escape: 27,
        Home: 36,
        ArrowDown: 40,
        a: 65
    }[key] ?? 0;
    await cdp.send("Input.dispatchKeyEvent", {
        type: "keyDown",
        key,
        code,
        windowsVirtualKeyCode: virtual,
        modifiers
    });
    await cdp.send("Input.dispatchKeyEvent", {
        type: "keyUp",
        key,
        code,
        windowsVirtualKeyCode: virtual,
        modifiers
    });
}
async function click(selector) {
    for (let i = 0; i < 12; i++) {
        const rect = await cdp.evaluate(`(()=>{const nodes=document.querySelectorAll(${JSON.stringify(selector)});if(nodes.length!==1)throw Error('selector cardinality');const el=nodes[0],r=el.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2,height:innerHeight,disabled:el.disabled,visible:r.width>0&&r.height>0};})()`);
        assert.ok(rect.visible, `visible ${selector}`);
        assert.ok(!rect.disabled, `enabled ${selector}`);
        if (rect.y > 30 && rect.y < rect.height - 35) {
            const hit = await cdp.evaluate(`(()=>{const el=document.querySelector(${JSON.stringify(selector)});return el.contains(document.elementFromPoint(${rect.x},${rect.y}));})()`);
            assert.equal(hit, true, `hit target ${selector}`);
            await cdp.send("Input.dispatchMouseEvent", {
                type: "mousePressed",
                x: rect.x,
                y: rect.y,
                button: "left",
                clickCount: 1
            });
            await cdp.send("Input.dispatchMouseEvent", {
                type: "mouseReleased",
                x: rect.x,
                y: rect.y,
                button: "left",
                clickCount: 1
            });
            return;
        }
        await cdp.send("Input.dispatchMouseEvent", {
            type: "mouseWheel",
            x: Math.max(350, rect.x),
            y: Math.min(rect.height - 100, 300),
            deltaX: 0,
            deltaY: Math.sign(rect.y - rect.height / 2) * Math.min(500, Math.abs(rect.y - rect.height / 2))
        });
        await sleep(130);
    }
    throw new Error(`Could not reach ${selector}`);
}
async function fill(selector, text) {
    await click(selector);
    await key("a", "KeyA", 2);
    await cdp.send("Input.insertText", {
        text
    });
}
async function choose(selector, index) {
    await click(selector);
    await key("Home");
    for (let i = 0; i < index; i++)
        await key("ArrowDown");
    await key("Enter");
    await key("Tab");
}
const text = selector => cdp.evaluate(`document.querySelector(${JSON.stringify(selector)})?.textContent ?? ''`);
const clientRowText = id => cdp.evaluate(`(() => {
    const rows = [...document.querySelectorAll('#client-rows tr')];
    const matches = rows.filter(row => row.querySelector('td:first-child small')?.textContent === ${JSON.stringify(id)});
    if (matches.length > 1) throw Error('duplicate client identity row');
    return matches[0]?.textContent ?? '';
})()`);
async function screenshot(name) {
    const result = await cdp.send("Page.captureScreenshot", {
        format: "png",
        captureBeyondViewport: false
    });
    await writeFile(path.join(root, name), Buffer.from(result.data, "base64"));
}
async function api(base, pathName, token, body) {
    return fetch(`${base}${pathName}`, {
        method: body ? "POST" : "GET",
        headers: {
            authorization: `Bearer ${token}`,
            "content-type": "application/json"
        },
        ...(body ? {
            body: JSON.stringify(body)
        } : {}),
        signal: AbortSignal.timeout(5000)
    });
}

async function exerciseComposition() {
    // Official protocol: https://chromedevtools.github.io/devtools-protocol/tot/Input/#method-imeSetComposition
    // Chromium's protocol test commits composition with Input.insertText:
    // https://raw.githubusercontent.com/chromium/chromium/main/third_party/blink/web_tests/inspector-protocol/input/imeSetComposition-01.js
    // This observer records native browser events only; it never dispatches or cancels an event.
    await cdp.evaluate(`(() => {
        if (window.__hubCompositionProbe) throw Error('composition probe already exists');
        const target = document.getElementById('edit-label');
        if (!target || document.activeElement !== target) throw Error('edit label is not focused');
        const events = [];
        const names = ['compositionstart', 'compositionupdate', 'compositionend'];
        const record = event => {
            if (events.length < 32) events.push({
                type: event.type, trusted: event.isTrusted, data: event.data,
                exact_target: event.target === target, value: target.value
            });
        };
        for (const name of names) target.addEventListener(name, record, { passive: true });
        window.__hubCompositionProbe = {
            target, events,
            dispose() {
                for (const name of names) target.removeEventListener(name, record);
                delete window.__hubCompositionProbe;
            }
        };
    })()`);
    const observe = () => cdp.evaluate(`(() => {
        const probe = window.__hubCompositionProbe;
        return {
            events: probe.events,
            value: probe.target.value,
            focused: document.activeElement === probe.target,
            same_node: document.getElementById('edit-label') === probe.target
        };
    })()`);
    results.composition = { status: "running", completion_method: "Input.insertText", observations: [] };
    const record = async phase => {
        const state = await observe();
        results.composition.observations.push({ phase, ...state });
        await writeFile(path.join(root, "composition.json"), JSON.stringify(results.composition, null, 2) + "\n");
        return state;
    };
    try {
        const previousLength = await cdp.evaluate("document.getElementById('edit-label').value.length");
        try {
            await cdp.send("Input.imeSetComposition", {
                text: "にほん", selectionStart: 3, selectionEnd: 3,
                replacementStart: 0, replacementEnd: previousLength
            });
        } catch (error) {
            if (error.code === -32601 || /method.*(?:not found|wasn't found|not supported)/i.test(error.message)) {
                results.composition = { status: "blocked", reason: "Input.imeSetComposition is unsupported by this WebView2 protocol." };
                process.stdout.write("BLOCKED WebView composition protocol unavailable; no synthetic replacement used\n");
                return;
            }
            throw error;
        }
        await record("start_dispatched");
        await until(async () => (await observe()).events.some(event => event.type === "compositionstart" && event.trusted), "trusted compositionstart", 5000);
        const candidate = "日本語入力中";
        await cdp.send("Input.imeSetComposition", {
            text: candidate, selectionStart: candidate.length, selectionEnd: candidate.length,
            replacementStart: 0, replacementEnd: 3
        });
        await record("update_dispatched");
        await until(async () => {
            const state = await observe();
            return state.value === candidate && state.events.some(event => event.type === "compositionupdate" && event.data === candidate && event.trusted);
        }, "trusted Japanese compositionupdate", 5000);
        await sleep(2600);
        const held = await record("held_across_poll");
        assert.equal(held.value, candidate);
        assert.equal(held.focused, true);
        assert.equal(held.same_node, true);
        assert.equal(held.events.some(event => event.type === "compositionend"), false);
        // Commit the existing composition, matching Chromium's own protocol test.
        // Do not equate a successful empty replacement command with a DOM end event.
        await cdp.send("Input.insertText", { text: candidate });
        await record("commit_dispatched");
        try {
            await until(async () => (await observe()).events.some(event => event.type === "compositionend"), "compositionend after commit", 5000);
        } catch (error) {
            await record("commit_completion_not_observed");
            results.composition.status = "harness_ng";
            results.composition.reason = "The browser protocol did not produce an observable compositionend after explicit commit.";
            throw Object.assign(new Error(results.composition.reason, { cause: error }), { classification: "harness_ng" });
        }
        const finished = await record("commit_completed");
        assert.ok(finished.events.every(event => event.exact_target));
        assert.ok(finished.events.filter(event => event.type !== "compositionend").every(event => event.trusted));
        assert.equal(finished.value, candidate);
        assert.equal(finished.focused, true);
        assert.equal(finished.same_node, true);
        results.composition.held_across_poll_ms = 2600;
        results.composition.events = finished.events;
        results.composition.native_ime = "manual_pending";
        const completionTrusted = finished.events.filter(event => event.type === "compositionend").every(event => event.trusted);
        results.composition.completion_trusted = completionTrusted;
        pass("trusted Japanese composition start/update preserve pending text, node and focus across polling");
        if (completionTrusted) {
            results.composition.status = "pass";
            pass("explicit WebView commit emits trusted compositionend");
        } else {
            results.composition.status = "partial";
            results.composition.reason = "Input.insertText produced compositionend with isTrusted=false. Start/update, text, node and focus passed; trusted native IME completion remains manual_pending.";
            process.stdout.write("PARTIAL compositionend observed with isTrusted=false; native IME completion remains manual_pending\n");
        }
    } finally {
        await writeFile(path.join(root, "composition.json"), JSON.stringify(results.composition, null, 2) + "\n");
        await cdp.evaluate("window.__hubCompositionProbe?.dispose()");
    }
}
try {
    await launch(1);
    assert.match(await text("#header-status"), /停止/);
    pass("server starts stopped");
    await choose("#profile", 1);
    await fill("#endpoint", `${endpoint}/slow`);
    await key("Tab");
    await until(() => requests.some(value => value.startsWith("/slow/")), "slow discovery begins");
    await fill("#endpoint", endpoint);
    await key("Tab");
    await until(async () => (await text("#discovery-status")).includes("2 件"), "fresh discovery");
    await sleep(1700);
    assert.equal(await cdp.evaluate("document.getElementById('endpoint').value"), endpoint);
    pass("endpoint discovery latest identity wins");
    await choose("#model", 1);
    await fill("#label", "検証 Alpha");
    await click("#register");
    await until(async () => {
        const rows = await text("#model-rows");
        return rows.includes("検証 Alpha") && rows.includes("ロード済み");
    }, "alpha registration and observed load state");
    pass("native model dropdown registration with ready status");
    await choose("#model", 2);
    await fill("#label", "検証 Beta");
    await click("#register");
    await until(async () => {
        const rows = await text("#model-rows");
        return rows.includes("検証 Beta") && rows.includes("未ロード");
    }, "beta registration and observed unloaded state");
    pass("unloaded model displayed separately");
    await click("#model-rows tr:first-child [data-edit]");
    await fill("#edit-label", "検証 Alpha 編集");
    await exerciseComposition();
    await fill("#edit-label", "検証 Alpha 編集");
    await sleep(2600);
    assert.equal(await cdp.evaluate("document.getElementById('edit-label').value"), "検証 Alpha 編集");
    assert.equal(await cdp.evaluate("document.activeElement.id"), "edit-label");
    await click("#edit-enabled");
    await click("#save-edit");
    await until(async () => (await text("#model-rows")).includes("メンテナンス"), "maintenance saved");
    assert.equal(await cdp.evaluate("document.activeElement?.matches('#model-rows [data-edit]')"), true);
    pass("edit draft and focus survive polling; maintenance saved");
    await click("#model-rows tr:first-child [data-edit]");
    await click("#edit-enabled");
    await click("#save-edit");
    await until(async () => !(await text("#model-rows")).includes("メンテナンス"), "maintenance cleared");
    fixtureMode = "missing";
    await click("#refresh-models");
    await until(async () => (await text("#model-rows")).includes("モデルなし"), "missing model");
    pass("successful listing omission is missing model");
    fixtureMode = "invalid";
    await click("#refresh-models");
    await until(async () => !(await text("#model-rows")).includes("モデルなし"), "invalid metadata cleared certainty");
    assert.match(await text("#model-rows"), /ロード状態不明/);
    pass("invalid metadata does not claim offline or missing");
    fixtureMode = "normal";
    await click("#refresh-models");
    await until(async () => (await text("#model-rows")).includes("ロード済み"), "restored model status");
    const reservation = createTcpServer();
    await new Promise(resolve => reservation.listen(0, "127.0.0.1", resolve));
    const hubPort = reservation.address().port;
    await new Promise(resolve => reservation.close(resolve));
    const token = randomBytes(32).toString("hex"), base = `http://127.0.0.1:${hubPort}`;
    await click("#manual-server-connection > summary");
    await until(() => cdp.evaluate("document.getElementById('manual-server-connection').open"), "manual server controls opened");
    await fill("#bind", `127.0.0.1:${hubPort}`);
    await fill("#token", token);
    await click("#start-server");
    await until(async () => (await text("#header-status")).includes("稼働中"), "server running");
    assert.equal(await cdp.evaluate("document.getElementById('token').value"), "");
    assert.equal((await fetch(`${base}/v1/catalog`)).status, 401);
    const catalog = await (await api(base, "/v1/catalog", token)).json();
    assert.equal(typeof catalog.revision, "string");
    assert.equal(catalog.models.length, 2);
    assert.ok(!JSON.stringify(catalog).includes(endpoint));
    assert.ok(!JSON.stringify(catalog).includes(token));
    assert.equal((await fetch(`${base}/v1/catalog`, {
        headers: {
            authorization: `Bearer ${token}`,
            origin: "http://example.invalid"
        }
    })).status, 403);
    const paired = await (await api(base, "/v1/clients/register", token, {
        label: "検証 Desktop"
    })).json();
    assert.equal(paired.hub_id, catalog.hub_id);
    assert.equal(paired.revision, catalog.revision);
    const ownedCatalogResponse = await api(base, "/v1/catalog", paired.client_token);
    assert.equal(ownedCatalogResponse.status, 200);
    assert.deepEqual(await ownedCatalogResponse.json(), catalog);
    assert.equal((await api(base, "/v1/clients/heartbeat", token, {
        id: paired.id,
        activity: "running"
    })).status, 401);
    assert.equal((await api(base, "/v1/clients/heartbeat", paired.client_token, {
        id: paired.id,
        activity: "running"
    })).status, 200);
    await until(async () => (await text("#client-rows")).includes("実行中"), "client heartbeat grid");
    assert.match(await text("#client-rows"), /127\.0\.0\.1/);
    pass("authenticated API, private endpoints, Origin rejection, bound client token and presence grid");
    pass("registration binds Hub identity and revision; device token reads the same public catalog");

    const otherClient = await (await api(base, "/v1/clients/register", token, { label: "別端末の検証" })).json();
    const reviewBody = (context, revision = catalog.revision) => ({
        id: paired.id, context, expected_hub_id: catalog.hub_id, reviewed_revision: revision,
        selection: {
            allowed_model_ids: [catalog.models[context === "main" ? 0 : 1].id],
            preferred_model_id: catalog.models[context === "main" ? 0 : 1].id,
            required_capabilities: ["tools"], wait_policy: "wait_for_preferred", affinity_turns: 2
        }
    });
    assert.equal((await api(base, "/v1/clients/review", otherClient.client_token, reviewBody("main"))).status, 401);
    assert.equal((await api(base, "/v1/clients/review", token, reviewBody("main"))).status, 401);
    assert.equal((await api(base, "/v1/clients/review", paired.client_token, reviewBody("main"))).status, 200);
    await until(async () => {
        const row = await clientRowText(paired.id);
        return row.includes(`Main: 確認済み r${catalog.revision}`) && row.includes("Side Chat: 未確認");
    }, "Main reviewed while Side remains unreviewed in exact device row");
    await click('a.nav-link[href="#clients"]');
    await screenshot("management-main-reviewed.png");
    pass("Main review is reflected in the device Grid without marking Side reviewed");

    assert.equal((await api(base, "/v1/clients/review", paired.client_token, reviewBody("side_chat"))).status, 200);
    await until(async () => {
        const row = await clientRowText(paired.id);
        return row.includes(`Main: 確認済み r${catalog.revision}`) && row.includes(`Side Chat: 確認済み r${catalog.revision}`);
    }, "both contexts explicitly reviewed");
    assert.match(await clientRowText(otherClient.id), /Main: 未確認/);
    assert.match(await clientRowText(otherClient.id), /Side Chat: 未確認/);
    await screenshot("management-both-reviewed.png");
    pass("Side review remains independent; other device token cannot change this device review");

    // A real administrator GUI edit advances the catalog; no Tauri invocation or file mutation.
    await click('a.nav-link[href="#overview"]');
    await click("#model-rows tr:first-child [data-edit]");
    await click("#edit-enabled");
    await click("#save-edit");
    await until(async () => (await text("#model-rows")).includes("メンテナンス"), "policy change through GUI");
    const changedCatalog = await (await api(base, "/v1/catalog", paired.client_token)).json();
    assert.notEqual(changedCatalog.revision, catalog.revision);
    const staleReview = await api(base, "/v1/clients/review", paired.client_token, reviewBody("main"));
    assert.equal(staleReview.status, 409);
    assert.deepEqual(await staleReview.json(), { error: "review_required", current_revision: changedCatalog.revision });
    await until(async () => {
        const row = await clientRowText(paired.id);
        return row.includes("Main: 再確認が必要") && row.includes("Side Chat: 再確認が必要");
    }, "catalog change invalidates both context review states");
    await click('a.nav-link[href="#clients"]');
    await screenshot("management-review-required.png");
    assert.equal((await api(base, "/v1/clients/review", paired.client_token, reviewBody("main", changedCatalog.revision))).status, 200);
    await until(async () => {
        const row = await clientRowText(paired.id);
        return row.includes(`Main: 確認済み r${changedCatalog.revision}`) && row.includes("Side Chat: 再確認が必要");
    }, "reviewing new Main revision does not clear Side review gate");
    await screenshot("management-main-rereviewed.png");
    pass("GUI catalog change rejects stale review with 409; re-reviewing Main preserves Side gate");

    assert.equal((await api(base, "/v1/clients/disconnect", otherClient.client_token, { id: paired.id })).status, 401);
    assert.equal((await api(base, "/v1/clients/disconnect", paired.client_token, { id: paired.id })).status, 200);
    await until(async () => !(await clientRowText(paired.id)), "disconnected exact client row disappears");
    assert.ok((await clientRowText(otherClient.id)).includes("別端末の検証"));
    assert.equal((await api(base, "/v1/catalog", paired.client_token)).status, 401);
    assert.equal((await api(base, "/v1/clients/heartbeat", paired.client_token, { id: paired.id, activity: "running" })).status, 401);
    assert.equal((await api(base, "/v1/clients/review", paired.client_token, reviewBody("main", changedCatalog.revision))).status, 401);
    await screenshot("management-client-disconnected.png");
    pass("disconnect removes only owned device row and revokes its catalog, heartbeat and review token");
    await click('a.nav-link[href="#overview"]');
    await screenshot("management-overview.png");
    await click('a.nav-link[href="#clients"]');
    await screenshot("management-clients.png");
    await click("#stop-server");
    await until(async () => (await text("#header-status")).includes("停止"), "server stopped");
    assert.equal(await cdp.evaluate("document.querySelectorAll('#client-rows tr').length"), 0);
    pass("explicit stop clears active client registrations");
    const persisted = JSON.parse(await readFile(path.join(data, "catalog.json"), "utf8"));
    const oldRevision = persisted.revision;
    assert.equal(persisted.catalog.models.length, 2);
    assert.ok(!JSON.stringify(persisted).includes(token));
    assert.ok(!JSON.stringify(persisted).includes(paired.client_token));
    assert.ok(!JSON.stringify(persisted).includes(otherClient.client_token));
    await stopProcess();
    await launch(2);
    await until(async () => (await text("#model-rows")).includes("検証 Alpha 編集"), "persisted registration after restart");
    assert.equal(await text("#metric-revision"), String(oldRevision));
    assert.match(await text("#model-rows"), /ロード状態不明/);
    assert.match(await text("#header-status"), /停止/);
    pass("catalog identity/revision persist; observations and server do not auto-resume");
    results.status = ["partial", "blocked"].includes(results.composition?.status) ? "manual_pending" : "pass";
}
catch (error) {
    results.status = error.classification ?? "fail";
    results.error = error.stack;
    process.exitCode = 1;
}
finally {
    try {
        await stopProcess();
        await new Promise(resolve => fixture.close(resolve));
        results.cleanup = "pass";
    }
    catch (error) {
        results.cleanup = "fail";
        results.cleanup_error = error.message;
        process.exitCode = 1;
    }
    results.finished_at = new Date().toISOString();
    results.elapsed_ms = Date.now() - started;
    await writeFile(path.join(root, "results.json"), JSON.stringify(results, null, 2) + "\n");
}
