import assert from "node:assert/strict";
import test from "node:test";
import { validateSenderGrant, decodeMcpToolResult } from "./browser/mcp-sender-fixture.mjs";

test("MCP caller rejects grants for a different participant, profile or durable task before opening a connection", () => {
    const target = { actorDeviceId: "sender", audienceDeviceId: "receiver", profileId: "temp", rootTaskId: "root", requestKey: "request" };
    const grant = { token: "x".repeat(64), claims: { actor_device_id: "sender", audience_device_id: "receiver", profile_id: "temp", root_task_id: "root", request_key: "request", mode: "agent" },
        peer: { device_id: "receiver", profile_id: "temp", mode: "agent", endpoint: "http://127.0.0.1:7332/mcp" } };
    for (const field of ["actor_device_id", "audience_device_id", "profile_id", "root_task_id", "request_key", "mode"]) {
        const wrong = structuredClone(grant); wrong.claims[field] = "other";
        assert.throws(() => validateSenderGrant(wrong, target), /does not match/);
    }
    for (const endpoint of ["http://127.0.0.1:7332/mcp", "https://127.0.0.1:7332/other", "https://user:pass@127.0.0.1:7332/mcp", "https://127.0.0.1:7332/mcp?token=x"]) {
        assert.throws(() => validateSenderGrant({ ...grant, peer: { ...grant.peer, endpoint } }, target), /endpoint is invalid/);
    }
});

test("MCP task result decoding accepts structured/text payloads and never treats protocol or tool errors as success", () => {
    const result = { job_id: "job", state: "completed", result: "fixture result" };
    assert.deepEqual(decodeMcpToolResult({ result: { structuredContent: result } }), result);
    assert.deepEqual(decodeMcpToolResult({ result: { content: [{ type: "text", text: JSON.stringify(result) }] } }), result);
    for (const value of [null, { error: { code: -32602 } }, { result: { isError: true, structuredContent: result } },
        { result: { structuredContent: [] } }, { result: { content: [{ type: "text", text: "no result" }] } }]) {
        assert.throws(() => decodeMcpToolResult(value));
    }
});
