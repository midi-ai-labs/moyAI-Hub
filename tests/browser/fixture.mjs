import { test as base, expect } from "@playwright/test";
import { startHubServer } from "./hub_server.mjs";

export const administrator = Object.freeze({ username: "browser-admin", display_name: "ブラウザー検証 管理者", password: "browser-fixture-password" });

export const test = base.extend({
    remoteWeb: [false, { option: true }],
    authenticatedAdmin: [true, { option: true }],
    adminAuthentication: [async ({ context, hub, authenticatedAdmin }, use) => {
        if (authenticatedAdmin) {
            const response = await context.request.get(new URL("/admin/session", hub.url).href);
            expect(response.ok()).toBe(true);
            const session = await response.json();
            const action = session.authentication.configured ? "login" : "bootstrap";
            const authenticated = await context.request.post(new URL(`/admin/${action}`, hub.url).href, {
                headers: { Origin: new URL(hub.url).origin, "X-Moyai-Csrf": session.csrf },
                data: action === "bootstrap" ? administrator : { username: administrator.username, password: administrator.password },
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
