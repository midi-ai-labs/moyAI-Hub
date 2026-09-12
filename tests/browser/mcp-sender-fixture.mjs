import { createHash, X509Certificate } from "node:crypto";
import { request } from "node:https";
import { checkServerIdentity } from "node:tls";

export const MCP_PROTOCOL = "2025-11-25";
const id = value => typeof value === "string" && /^[A-Za-z0-9_.:-]{1,128}$/.test(value);
export function validateSenderGrant(grant, target) {
    const claims = grant?.claims, peer = grant?.peer;
    if (!claims || !peer || !id(target.actorDeviceId) || !id(target.audienceDeviceId)
        || !id(target.profileId) || !id(target.rootTaskId) || !id(target.requestKey)
        || claims.actor_device_id !== target.actorDeviceId || claims.audience_device_id !== target.audienceDeviceId
        || claims.profile_id !== target.profileId || claims.root_task_id !== target.rootTaskId
        || claims.request_key !== target.requestKey || claims.mode !== "agent"
        || peer.device_id !== target.audienceDeviceId || peer.profile_id !== target.profileId
        || peer.mode !== "agent" || !/^[A-Za-z0-9._~-]{32,256}$/.test(grant.token ?? "")) {
        throw new Error("MCP fixture grant does not match the requested participant, profile and task");
    }
    const url = new URL(peer.endpoint);
    if (url.protocol !== "https:" || url.pathname !== "/mcp" || url.username || url.password || url.search || url.hash) {
        throw new Error("MCP fixture peer endpoint is invalid");
    }
    const fingerprint = createHash("sha256").update(new X509Certificate(peer.certificate_pem).raw).digest("hex");
    if (fingerprint !== peer.certificate_sha256) throw new Error("MCP fixture peer certificate fingerprint mismatch");
    return fingerprint;
}
export function decodeMcpToolResult(value) {
    if (!value || value.error || value.result?.isError) throw new Error("MCP fixture received a protocol/tool error");
    const result = value.result;
    const structured = result?.structuredContent ?? result?.content?.filter(part => part.type === "text")
        .map(part => { try { return JSON.parse(part.text); } catch { return null; } }).find(Boolean);
    if (!structured || typeof structured !== "object" || Array.isArray(structured)) throw new Error("MCP fixture received no structured task result");
    return structured;
}

/** Ordinary mTLS MCP caller. Credentials remain in this closure; no effectful retry. */
export async function openSenderMcpSession({ grant, target, ca, certificate, key }) {
    const fingerprint = validateSenderGrant(grant, target);
    let session = null, sequence = 0, closed = false;
    const calls = [];
    async function send(method, params, notification = false, remove = false) {
        if (closed) throw new Error("MCP fixture session is closed");
        const payload = remove ? null : Buffer.from(JSON.stringify({ jsonrpc: "2.0", ...(!notification ? { id: ++sequence } : {}), method, params }));
        calls.push(remove ? "DELETE" : method === "tools/call" ? params.name : method);
        return new Promise((resolve, reject) => {
            const req = request(grant.peer.endpoint, {
                method: remove ? "DELETE" : "POST", ca, cert: certificate, key, agent: false, timeout: 15000,
                checkServerIdentity(host, cert) {
                    const error = checkServerIdentity(host, cert);
                    if (error) return error;
                    if (createHash("sha256").update(cert.raw).digest("hex") !== fingerprint) return new Error("MCP fixture peer identity changed");
                },
                headers: { authorization: `Bearer ${grant.token}`, accept: "application/json, text/event-stream",
                    "mcp-protocol-version": MCP_PROTOCOL, ...(session ? { "mcp-session-id": session } : {}),
                    ...(payload ? { "content-type": "application/json", "content-length": payload.length } : {}) },
            }, response => {
                const chunks = []; let bytes = 0;
                response.on("data", chunk => { bytes += chunk.length; if (bytes > 512 * 1024) response.destroy(new Error("MCP fixture response too large")); else chunks.push(chunk); });
                response.on("error", reject);
                response.on("end", () => {
                    if (![200, 202, 204].includes(response.statusCode)) { reject(new Error(`MCP fixture request failed: HTTP ${response.statusCode}`)); return; }
                    if (!session && response.headers["mcp-session-id"]) session = response.headers["mcp-session-id"];
                    try { resolve(bytes ? JSON.parse(Buffer.concat(chunks).toString("utf8")) : null); }
                    catch { reject(new Error("MCP fixture expected a JSON response")); }
                });
            });
            req.on("error", reject); req.on("timeout", () => req.destroy(new Error("MCP fixture request timed out")));
            req.end(payload);
        });
    }
    const initialized = await send("initialize", { protocolVersion: MCP_PROTOCOL, capabilities: {}, clientInfo: { name: "Desktop GUI sender fixture", version: "1" } });
    if (!session || initialized?.result?.protocolVersion !== MCP_PROTOCOL) throw new Error("MCP fixture initialization did not establish the expected session");
    await send("notifications/initialized", {}, true);
    const tool = async (name, arguments_) => decodeMcpToolResult(await send("tools/call", { name, arguments: arguments_ }));
    return {
        delegate: prompt => tool("delegate_task", { request_key: target.requestKey,
            parent: { peer_id: target.actorDeviceId, task_id: target.rootTaskId, turn_id: target.rootTaskId }, prompt }),
        status: jobId => tool("task_status", { job_id: jobId }),
        cancel: jobId => tool("cancel_task", { job_id: jobId }),
        calls: () => [...calls],
        async close() { if (!closed) { try { await send(null, null, false, true); } finally { closed = true; } } },
    };
}
