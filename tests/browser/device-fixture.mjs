import { generateKeyPairSync, randomUUID, sign } from "node:crypto";
import { createServer as createHttpsServer, request as httpsRequest } from "node:https";
import { openSenderMcpSession } from "./mcp-sender-fixture.mjs";
import { MCP_PROTOCOL } from "./mcp-sender-fixture.mjs";

// Test participant only. It uses the real enrollment/mTLS protocol; no Admin
// mutation or Hub storage access. Its private key never leaves this process.
const der = (tag, ...parts) => {
    const data = Buffer.concat(parts);
    let length = Buffer.from([data.length]);
    if (data.length >= 128) {
        const bytes = []; let value = data.length;
        while (value) { bytes.unshift(value & 255); value >>>= 8; }
        length = Buffer.from([128 | bytes.length, ...bytes]);
    }
    return Buffer.concat([Buffer.from([tag]), length, data]);
};
const sequence = (...parts) => der(0x30, ...parts);
const oid = hex => der(0x06, Buffer.from(hex, "hex"));
function csr(keys, commonName) {
    const subject = sequence(der(0x31, sequence(oid("550403"), der(0x0c, Buffer.from(commonName)))));
    const san = sequence(oid("551d11"), der(0x04, sequence(der(0x87, Buffer.from([127, 0, 0, 1])))));
    const extensions = sequence(oid("2a864886f70d01090e"), der(0x31, sequence(san)));
    const info = sequence(der(0x02, Buffer.from([0])), subject,
        keys.publicKey.export({ type: "spki", format: "der" }), der(0xa0, extensions));
    const body = sequence(info, sequence(oid("2a8648ce3d040302")),
        der(0x03, Buffer.from([0]), sign("sha256", info, keys.privateKey))).toString("base64");
    return `-----BEGIN CERTIFICATE REQUEST-----\n${body.match(/.{1,64}/g).join("\n")}\n-----END CERTIFICATE REQUEST-----\n`;
}

export async function createDeviceParticipant(network, label, windowsUsername = "fixture\\operator") {
    const keys = generateKeyPairSync("ec", { namedCurve: "prime256v1" });
    const key = keys.privateKey.export({ type: "pkcs8", format: "pem" });
    let certificate = null;
    async function request(path, body, authenticated = true, sharedToken = null, namespace = "network") {
        const data = body === undefined ? null : Buffer.from(JSON.stringify(body));
        return new Promise((resolve, reject) => {
            const req = httpsRequest(new URL(`/v1/${namespace}/${path}`, network.server.bind), {
                method: data ? "POST" : "GET", ca: network.ca_certificate_pem,
                ...(authenticated && certificate ? { key, cert: certificate.certificate_pem } : {}),
                agent: false, timeout: 5000,
                headers: { ...(data ? { "content-type": "application/json", "content-length": data.length } : {}), ...(sharedToken ? { authorization: `Bearer ${sharedToken}` } : {}),
                    ...(namespace === "shared" && path.startsWith("runner/") ? { "x-moyai-runner-capabilities": "multi_device_session_v2" } : {}) },
            }, response => {
                const chunks = [];
                response.on("data", chunk => chunks.push(chunk));
                response.on("end", () => {
                    try {
                        const result = JSON.parse(Buffer.concat(chunks).toString());
                        if (response.statusCode !== 200) reject(new Error(`Participant ${path}: HTTP ${response.statusCode}: ${result.error ?? "request failed"}`));
                        else resolve(result);
                    } catch (error) { reject(error); }
                });
            });
            req.on("error", reject); req.on("timeout", () => req.destroy(new Error(`Participant ${path} timed out`)));
            req.end(data);
        });
    }
    const receipt = await request("join/request", { label, windows_username: windowsUsername, csr_pem: csr(keys, "Browser test participant") }, false);
    const proof = csr(keys, `moyAI join ${receipt.hub_id} ${receipt.request_id} ${receipt.challenge}`);
    let humanToken = null;
    return {
        requestId: receipt.request_id,
        async collectApproval() {
            const result = await request("join/status", { request_id: receipt.request_id, proof_csr_pem: proof }, false);
            certificate = result.certificate;
            return { status: result.status, deviceId: certificate?.device_id };
        },
        presence: () => request("presence", {}),
        async sharedDeviceSession() {
            if (!certificate) throw new Error("Shared session requires an enrolled device");
            const session = await request("device-session", {}, true, null, "shared");
            humanToken = session.token;
            return { principal: session.principal, expires_at_ms: session.expires_at_ms };
        },
        async sharedCall(operation, body) {
            const routes = { snapshot: "admin/snapshot", command: "admin/command", createUser: "admin/users", createProject: "admin/projects", membership: "admin/memberships", environment: "admin/environments", projects: "projects", submit: "jobs", claim: "runner/claim", report: "runner/report" };
            if (!Object.hasOwn(routes, operation) || !humanToken) throw new Error("Unknown or unauthenticated shared fixture operation");
            return request(routes[operation], body, true, humanToken, "shared");
        },
        async sharedCommand(operation) {
            if (!humanToken) throw new Error("Shared command requires a device session");
            const snapshot = await request("admin/snapshot", undefined, true, humanToken, "shared");
            return request("admin/command", { request_id: randomUUID(), expected_revision: snapshot.revision, request: operation }, true, humanToken, "shared");
        },
        sharedAssignments(environmentIds) {
            if (!certificate || !Array.isArray(environmentIds) || environmentIds.length === 0 || environmentIds.length > 32 || environmentIds.some(id => typeof id !== "string" || !/^[A-Za-z0-9_-]{1,128}$/.test(id))) throw new Error("Assignments require an enrolled runner and bounded environment IDs");
            return request(`runner/assignments?environment_ids=${encodeURIComponent(environmentIds.join(","))}`, undefined, true, null, "shared");
        },
        sharedConsumeApproval(approvalId, attemptId, generation) {
            if (!certificate || ![approvalId, attemptId].every(id => typeof id === "string" && /^[A-Za-z0-9_-]{1,128}$/.test(id)) || !Number.isSafeInteger(generation) || generation < 1) throw new Error("Approval consumption requires an enrolled Runner and exact approval/attempt identities");
            return request(`runner/approvals/${approvalId}/consume`, { attempt_id: attemptId, generation }, true, null, "shared");
        },
        sharedTemplates: templates => request("runner/templates", { templates }, true, null, "shared"),
        sharedProvisioning: () => request("runner/provisioning", undefined, true, null, "shared"),
        sharedProvisionReceipt: receipt => request("runner/provisioning", receipt, true, null, "shared"),
        sharedDeviceProjects: () => request("device-projects", undefined, true, null, "shared"),
        publish: publication => request("publish", publication),
        historyRequests: async () => (await request("history/requests")).requests,
        submitHistory: (requestId, response, error = null) => request("history/results", { request_id: requestId, response, error }),
        async startMcpReceiver({ tools, call }) {
            if (!certificate || !Array.isArray(tools) || typeof call !== "function") throw new Error("Approved participant and bounded tools are required");
            const sessions = new Set(), calls = [], sockets = new Set();
            const server = createHttpsServer({ key, cert: certificate.certificate_pem, ca: network.ca_certificate_pem,
                requestCert: true, rejectUnauthorized: true }, async (req, res) => {
                const reply = (status, value, headers = {}) => { res.writeHead(status, { "content-type": "application/json", ...headers }); res.end(value === null ? "" : JSON.stringify(value)); };
                if (req.url !== "/mcp") { reply(404, null); return; }
                // The ordinary TLS diagnostic probes GET before the MCP handshake.
                if (req.method === "GET") { reply(405, null); return; }
                if (!/^Bearer [A-Za-z0-9._~-]{32,256}$/.test(req.headers.authorization ?? "")) { reply(401, null); return; }
                if (req.method === "DELETE") { sessions.delete(req.headers["mcp-session-id"]); reply(204, null); return; }
                if (req.method !== "POST") { reply(405, null); return; }
                try {
                    const chunks = []; let bytes = 0;
                    for await (const chunk of req) { bytes += chunk.length; if (bytes > 512 * 1024) throw new Error("MCP fixture request too large"); chunks.push(chunk); }
                    const body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
                    calls.push({ method: body.method, tool: body.params?.name ?? null });
                    if (body.method === "initialize") {
                        if (sessions.size >= 16) throw new Error("MCP fixture session bound exceeded");
                        const session = randomUUID(); sessions.add(session);
                        reply(200, { jsonrpc: "2.0", id: body.id, result: { protocolVersion: MCP_PROTOCOL,
                            capabilities: { tools: {} }, serverInfo: { name: "GUI protocol receiver fixture", version: "1" } } }, { "mcp-session-id": session });
                        return;
                    }
                    if (!req.headers["mcp-session-id"]) { reply(400, { jsonrpc: "2.0", id: body.id, error: { code: -32000, message: "Initialize an MCP session first" } }); return; }
                    if (!sessions.has(req.headers["mcp-session-id"])) { reply(404, null); return; }
                    if (body.method === "notifications/initialized") { reply(202, null); return; }
                    if (body.method === "tools/list") { reply(200, { jsonrpc: "2.0", id: body.id, result: { tools } }); return; }
                    if (body.method !== "tools/call" || !tools.some(tool => tool.name === body.params?.name)) throw new Error("Unexpected fixture MCP method/tool");
                    const result = await call(body.params.name, body.params.arguments ?? {});
                    reply(200, { jsonrpc: "2.0", id: body.id, result: { content: [{ type: "text", text: JSON.stringify(result) }], structuredContent: result, isError: false } });
                } catch (error) { reply(400, { error: error.message }); }
            });
            server.on("connection", socket => { sockets.add(socket); socket.on("close", () => sockets.delete(socket)); });
            await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
            let closed = false;
            return { endpoint: `https://127.0.0.1:${server.address().port}/mcp`, calls: () => [...calls],
                async close() { if (closed) return; closed = true; const done = new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve())); for (const socket of sockets) socket.destroy(); await done; sessions.clear(); } };
        },
        async connectMcpPeer({ audienceDeviceId, profileId, rootTaskId, requestKey }) {
            if (!certificate) throw new Error("Participant must be approved before requesting an MCP grant");
            const grant = await request("grants", { audience_device_id: audienceDeviceId, profile_id: profileId,
                root_task_id: rootTaskId, request_key: requestKey, parent_grant_id: null, parent_job_id: null, action: "execute" });
            return openSenderMcpSession({ grant, target: { actorDeviceId: certificate.device_id, audienceDeviceId, profileId, rootTaskId, requestKey },
                ca: network.ca_certificate_pem, certificate: certificate.certificate_pem, key });
        },
    };
}
