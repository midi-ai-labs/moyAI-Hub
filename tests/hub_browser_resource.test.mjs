import assert from "node:assert/strict";
import test from "node:test";
import path from "node:path";
import { validateHubDescriptor, validateHubHealth } from "./browser/hub_server.mjs";

test("shared real Hub owner rejects foreign identity without exposing descriptor credentials", () => {
    const catalog = path.resolve("test-catalog.json");
    const descriptor = { schema_version: 1, pid: 123, catalog_path: catalog, port: 34567, token: "S".repeat(52), instance_id: "I".repeat(26) };
    assert.doesNotThrow(() => validateHubDescriptor(descriptor, { pid: 123, catalog }));
    for (const changed of [{ pid: 124 }, { catalog_path: path.resolve("foreign-catalog.json") }, { port: 0 }, { token: "secret-invalid" }, { instance_id: "foreign" }]) {
        assert.throws(() => validateHubDescriptor({ ...descriptor, ...changed }, { pid: 123, catalog }), error => !error.message.includes(descriptor.token) && !error.message.includes("secret-invalid"));
    }
    const health = { pid: 123, instance_id: descriptor.instance_id, catalog_path: catalog, closing: false };
    assert.doesNotThrow(() => validateHubHealth(health, descriptor, catalog));
    for (const changed of [{ pid: 124 }, { instance_id: "foreign" }, { catalog_path: path.resolve("foreign.json") }, { closing: true }]) {
        assert.throws(() => validateHubHealth({ ...health, ...changed }, descriptor, catalog));
    }
});
