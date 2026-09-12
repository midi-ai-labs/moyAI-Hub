import { test as base, expect } from "@playwright/test";
import { startHubServer } from "./hub_server.mjs";

export const test = base.extend({
    remoteWeb: [false, { option: true }],
    hub: [async ({ remoteWeb }, use, testInfo) => {
        const hub = await startHubServer({ dataDirectory: testInfo.outputPath("hub-data"),
            remoteWeb,
            record: (name, value) => testInfo.attach(name, {
                contentType: "application/json", body: Buffer.from(JSON.stringify(value, null, 2)),
            }),
        });
        try { await use(hub); }
        finally {
            const result = await hub.close();
            expect(result.pass, "The isolated Hub and every captured listener must shut down cleanly").toBe(true);
        }
    }, { timeout: 90_000 }],
});
export { expect };
