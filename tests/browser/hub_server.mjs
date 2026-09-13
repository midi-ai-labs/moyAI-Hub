import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { access, mkdir, readFile, writeFile } from "node:fs/promises";
import { createConnection, createServer } from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { setTimeout as delay } from "node:timers/promises";

const repository = fileURLToPath(new URL("../../", import.meta.url));
const canonical = value => path.resolve(value.startsWith("\\\\?\\") ? value.slice(4) : value);
const requireValue = (condition, message) => { if (!condition) throw new Error(message); };

export function validateHubDescriptor(descriptor, { pid, catalog }) {
    // Errors deliberately exclude the descriptor, which contains a private credential.
    requireValue(descriptor?.schema_version === 1 && descriptor.pid === pid, "Hub descriptor process identity mismatch");
    requireValue(typeof descriptor.catalog_path === "string" && canonical(descriptor.catalog_path) === canonical(catalog), "Hub descriptor catalog identity mismatch");
    requireValue(Number.isInteger(descriptor.port) && descriptor.port > 0 && descriptor.port <= 65535, "Hub descriptor port invalid");
    requireValue(typeof descriptor.token === "string" && /^[A-Za-z0-9]{52}$/.test(descriptor.token), "Hub descriptor credential invalid");
    requireValue(typeof descriptor.instance_id === "string" && /^[A-Za-z0-9]{26}$/.test(descriptor.instance_id), "Hub descriptor instance identity invalid");
}

export function validateHubHealth(health, descriptor, catalog) {
    requireValue(health?.pid === descriptor.pid && health.instance_id === descriptor.instance_id
        && typeof health.catalog_path === "string" && canonical(health.catalog_path) === canonical(catalog)
        && health.closing === false, "Hub health identity mismatch");
}

async function poll(label, sample, accept, timeout = 15_000) {
    const until = Date.now() + timeout;
    do { const value = await sample(); if (accept(value)) return value; await delay(50); } while (Date.now() < until);
    throw new Error(`Hub fixture timed out: ${label}`);
}

export async function freePort() {
    const socket = createServer();
    await new Promise((resolve, reject) => { socket.once("error", reject); socket.listen(0, "127.0.0.1", resolve); });
    const { port } = socket.address();
    await new Promise((resolve, reject) => socket.close(error => error ? reject(error) : resolve()));
    return port;
}

async function binaryHash(binary) {
    const hash = createHash("sha256");
    for await (const chunk of createReadStream(binary)) hash.update(chunk);
    return hash.digest("hex");
}

async function listenerState(port) {
    return new Promise(resolve => {
        const socket = createConnection({ host: "127.0.0.1", port });
        const finish = state => { socket.destroy(); resolve(state); };
        socket.once("connect", () => finish("open"));
        socket.once("error", error => finish(error.code === "ECONNREFUSED" ? "closed" : `error:${error.code}`));
        socket.setTimeout(1_000, () => finish("timeout"));
    });
}

/** Fresh real server shared by browser and actual Desktop adapters; never starts a device network. */
export async function startHubServer({ dataDirectory, binary: explicitBinary = process.env.MOYAI_HUB_TEST_BINARY, remoteWeb = false, record = async () => {} }) {
    requireValue(path.isAbsolute(dataDirectory), "Hub fixture data directory must be absolute");
    requireValue(!explicitBinary || path.isAbsolute(explicitBinary), "MOYAI_HUB_TEST_BINARY must be an absolute executable path");
    const binary = explicitBinary ?? path.join(repository, "target/debug", process.platform === "win32" ? "moyai-hub.exe" : "moyai-hub");
    await access(binary).catch(() => { throw new Error("Build the current Hub first: npm run build && cargo build --bin moyai-hub; or set MOYAI_HUB_TEST_BINARY"); });
    const data = path.resolve(dataDirectory);
    await mkdir(path.dirname(data), { recursive: true });
    await mkdir(data, { recursive: false });
    const catalog = path.join(data, "catalog.json"), descriptorFile = path.join(data, "host-control.json");
    // Exercise the public remote listener configuration without exposing this
    // fixture outside loopback or introducing a second process lifecycle owner.
    let remoteOrigin = null;
    const args = ["--data-dir", data];
    if (remoteWeb) {
        remoteOrigin = `http://127.0.0.1:${await freePort()}`;
        const settingsFile = path.join(data, "fixture-web-settings.json");
        await writeFile(settingsFile, JSON.stringify({ local_port: 0,
            remote: { mode: "http", bind: new URL(remoteOrigin).host, public_origin: remoteOrigin } }), { flag: "wx" });
        args.push("--web-settings", settingsFile);
    } else args.push("--web-port", "0");
    const started = Date.now();
    const child = spawn(binary, args, { windowsHide: true, stdio: "ignore" });
    let spawnError, descriptor, verifiedHost = false, closePromise;
    const exited = new Promise(resolve => {
        child.once("exit", code => resolve(code));
        child.once("error", error => { spawnError = error; resolve(null); });
    });
    const listeners = new Map();
    const rememberListener = (name, address) => {
        if (typeof address !== "string") return;
        const url = new URL(address.includes("://") ? address : `http://${address}`);
        requireValue(url.hostname === "127.0.0.1" && url.port && Number(url.port) > 0, `Fixture ${name} listener must use an explicit loopback port`);
        listeners.set(Number(url.port), name);
    };
    const request = async (route, body, timeoutMs = 30_000) => {
        requireValue(route === "health" || verifiedHost, "Hub ownership must be verified before commands or shutdown");
        const response = await fetch(`http://127.0.0.1:${descriptor.port}/host/${route}`, {
            method: body === undefined ? "GET" : "POST",
            headers: { Authorization: `Bearer ${descriptor.token}`, "Content-Type": "application/json" },
            body: body === undefined ? undefined : JSON.stringify(body), redirect: "error", signal: AbortSignal.timeout(timeoutMs),
        });
        if (!response.ok) throw new Error(`Fixture host ${route}: HTTP ${response.status}`);
        return response.json();
    };
    const command = async (name, args = {}, timeoutMs) => {
        const reply = await request("command", { command: name, args }, timeoutMs);
        requireValue(reply.ok === true, `Fixture host command failed: ${name}`);
        return reply.value;
    };
    const observeNetwork = async timeoutMs => {
        const snapshot = await command("hub_network_snapshot", {}, timeoutMs);
        if (snapshot.server.running) { rememberListener("network", snapshot.server.bind); rememberListener("gateway", snapshot.server.gateway_bind); }
        return snapshot;
    };
    const close = () => closePromise ??= (async () => {
        let cleanShutdown = false;
        if (verifiedHost && child.exitCode === null && !spawnError) {
            try { await observeNetwork(2_000); } catch { /* Still shut down the exact verified host. */ }
            try {
                await request("shutdown", {}, 5_000);
                const result = await Promise.race([exited.then(code => ({ code })), delay(25_000, null, { ref: false })]);
                cleanShutdown = result?.code === 0;
            } catch { /* Exact-child fallback is a failed graceful cleanup. */ }
        }
        if (child.exitCode === null && !spawnError) { child.kill(); await Promise.race([exited, delay(5_000, null, { ref: false })]); }
        const listenerStates = await Promise.all([...listeners].map(async ([port, name]) => ({ name, port, state: await listenerState(port) })));
        let descriptorRemoved = false;
        try { await access(descriptorFile); } catch (error) { descriptorRemoved = error.code === "ENOENT"; }
        const result = { pid: child.pid, clean_shutdown: cleanShutdown, exit_code: child.exitCode,
            started: new Date(started).toISOString(), elapsed_ms: Date.now() - started, listeners: listenerStates,
            descriptor_removed: descriptorRemoved, pass: cleanShutdown && descriptorRemoved && listenerStates.every(listener => listener.state === "closed") };
        await record("hub-lifecycle", result);
        return result;
    })();
    try {
        descriptor = await poll("descriptor", async () => {
            if (spawnError) throw new Error("Hub fixture process could not start");
            if (child.exitCode !== null) throw new Error(`Hub fixture exited: ${child.exitCode}`);
            try { return JSON.parse(await readFile(descriptorFile, "utf8")); }
            catch (error) {
                if (error.code === "ENOENT") return null;
                if (error instanceof SyntaxError) throw new Error("Hub descriptor JSON is invalid");
                throw error;
            }
        }, Boolean);
        validateHubDescriptor(descriptor, { pid: child.pid, catalog });
        validateHubHealth(await request("health"), descriptor, catalog);
        verifiedHost = true;
        rememberListener("host control", `http://127.0.0.1:${descriptor.port}`);
        const web = await poll("Web listener", () => command("hub_web_status"), value => value.enabled === true);
        const url = new URL(web.local_url);
        requireValue(url.protocol === "http:" && url.hostname === "127.0.0.1" && url.pathname === "/admin/"
            && web.remote_url === (remoteOrigin ? `${remoteOrigin}/admin/` : null), "Hub fixture Web listener is not isolated loopback HTTP");
        rememberListener("web", web.local_url);
        if (web.remote_url) rememberListener("remote web", web.remote_url);
        const networkPort = await freePort();
        requireValue((await observeNetwork()).server.running === false, "Hub fixture must not implicitly start the device network");
        await record("hub-build", { binary, sha256: await binaryHash(binary), pid: child.pid, local_url: web.local_url, remote_url: web.remote_url });
        return Object.freeze({ binary, dataDirectory: data, pid: child.pid, url: web.local_url, remoteUrl: web.remote_url, command, networkPort, observeNetwork, close });
    } catch (error) {
        try { await close(); } catch (cleanupError) { throw new AggregateError([error, cleanupError], "Hub fixture setup and cleanup failed"); }
        throw error;
    }
}
