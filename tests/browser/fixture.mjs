import { test as base, expect } from "@playwright/test";
import { startHubServer } from "./hub_server.mjs";

export const administrator = Object.freeze({ display_name: "Hub PC 管理" });

export const test = base.extend({
    remoteWeb: [false, { option: true }],
    authenticatedAdmin: [true, { option: true }],
    adminAuthentication: [async ({ context, hub, authenticatedAdmin }, use) => {
        if (authenticatedAdmin) {
            const response = await context.request.get(new URL("/admin/session", hub.url).href);
            expect(response.ok()).toBe(true);
            const session = await response.json();
            const grant = await hub.command("hub_web_open");
            const ticket = new URLSearchParams(new URL(grant.url).hash.slice(1)).get("access");
            const authenticated = await context.request.post(new URL("/admin/access", hub.url).href, {
                headers: { Origin: new URL(hub.url).origin, "X-Moyai-Csrf": session.csrf },
                data: { ticket },
            });
            expect(authenticated.ok(), await authenticated.text()).toBe(true);
            expect((await authenticated.json()).authentication.user.administrator).toBe(true);
        }
        await use();
    }, { auto: true }],
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
