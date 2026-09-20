import assert from "node:assert/strict";
import test from "node:test";
import path from "node:path";
import { classifyWindowsListener, validateHubDescriptor, validateHubHealth } from "./browser/hub_server.mjs";

test("LAN cleanup requires a valid OS listener table and detects exact or wildcard listeners", () => {
    const ip = "192.168.10.106", port = 40000;
    assert.equal(classifyWindowsListener({listeners:[]},ip,port),"closed");
    for (const address of [ip,"0.0.0.0","::"]) {
        assert.equal(classifyWindowsListener({listeners:[{address,port}]},ip,port),"open");
    }
    assert.equal(classifyWindowsListener({listeners:[{address:ip,port:40001},{address:"192.168.10.107",port}]},ip,port),"closed");
    for (const invalid of [null,{}, {listeners:null},{listeners:[{address:ip,port:"40000"}]}]) {
        assert.throws(()=>classifyWindowsListener(invalid,ip,port));
    }
});

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
