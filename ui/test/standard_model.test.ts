import test from "node:test";
import assert from "node:assert/strict";
import { standardModelOptions } from "../src/standard_model.ts";
import type { Snapshot } from "../src/state.ts";

test("Main requires tools while Sub and Approve share ordinary chat candidates", () => {
    const snapshot = {
        store: { catalog: { models: [
            { id: "agent", label: "Agent", capabilities: ["tools"] },
            { id: "chat", label: "Chat", capabilities: [] },
            { id: "disabled", label: "Disabled", capabilities: [] },
        ] } },
        deployments: [
            { deployment: { logical_model_id: "agent", enabled: true } },
            { deployment: { logical_model_id: "chat", enabled: true } },
            { deployment: { logical_model_id: "disabled", enabled: false } },
        ],
    } as unknown as Snapshot;
    assert.deepEqual(standardModelOptions(snapshot, "main"), [{ id: "agent", label: "Agent", enabled: true }]);
    for (const role of ["side_chat", "approve"] as const) {
        assert.deepEqual(standardModelOptions(snapshot, role), [
            { id: "agent", label: "Agent", enabled: true }, { id: "chat", label: "Chat", enabled: true },
            { id: "disabled", label: "Disabled", enabled: false },
        ]);
    }
});
