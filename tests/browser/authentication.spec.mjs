import { test, expect, administrator } from "./fixture.mjs";
import { createMetadataProvider } from "./provider-fixture.mjs";

test.use({ authenticatedAdmin: false });

async function bootstrap(page, hub) {
    const grant = await hub.command("hub_web_open");
    await page.goto(grant.url);
    await expect(page.locator("#management-app")).toBeVisible();
    await expect(page.locator("#web-auth-user")).toHaveText(administrator.display_name);
    await expect(page).toHaveURL(hub.url);
}

test("Hub launch, close and reopen require no password and reject a used ticket", async ({ page, hub }, testInfo) => {
    const commands = [];
    page.on("request", request => { if (request.url().endsWith("/admin/command")) commands.push(request); });
    await page.goto(hub.url);
    await expect(page.locator("#web-auth-title")).toContainText("管理画面を開いてください");
    await expect(page.locator("#management-app")).toBeHidden();
    await expect(page.locator('#web-authentication input[type="password"]')).toHaveCount(0);
    expect(commands).toHaveLength(0);
    const session = await (await page.request.get(new URL("/admin/session", hub.url).href)).json();
    const forbidden = await page.request.post(new URL("/admin/command", hub.url).href, {
        headers: { Origin: new URL(hub.url).origin, "X-Moyai-Csrf": session.csrf },
        data: { command: "hub_snapshot", args: {} },
    });
    expect(forbidden.status()).toBe(401);
    const grant = await hub.command("hub_web_open");
    const ticket = new URLSearchParams(new URL(grant.url).hash.slice(1)).get("access");
    await page.goto(grant.url);
    await expect(page.locator("#management-app")).toBeVisible();
    await expect(page).toHaveURL(hub.url);
    const cookie = (await page.context().cookies(hub.url)).find(value => value.name === "moyai_admin");
    expect(cookie).toMatchObject({ httpOnly:true, sameSite:"Strict", path:"/admin" });
    expect(await page.evaluate(() => document.cookie)).toBe("");
    await testInfo.attach("passwordless-management", { body:await page.screenshot(), contentType:"image/png" });
    await page.locator("#web-auth-logout").click();
    await expect(page.locator("#management-app")).toBeHidden();
    await expect.poll(async () => (await page.context().cookies(hub.url)).length).toBe(0);
    const used=await page.request.post(new URL("/admin/access",hub.url).href, {
        headers:{Origin:new URL(hub.url).origin,"X-Moyai-Csrf":session.csrf},data:{ticket},
    });
    expect(used.status()).toBe(401);
    await bootstrap(page,hub);
});
test("logout hides a delayed management result and revokes another open tab", async ({ page, context, hub }) => {
    await bootstrap(page, hub);
    const other = await context.newPage();
    const provider = await createMetadataProvider();
    try {
        await other.goto(hub.url);
        await expect(other.locator("#management-app")).toBeVisible();
        await other.locator("#endpoint").fill(provider.url);
        await other.locator("#discover").click();
        await expect(other.locator("#model option")).toHaveCount(3);
        await other.locator("#model").selectOption("fixture-alpha");
        await other.locator("#label").fill("Authentication modal");
        await other.locator("#register").click();
        await other.getByRole("button", { name: "Authentication modal を編集", exact: true }).click();
        await expect(other.locator("#edit-dialog")).toBeVisible();
        let release;
        const delayed = new Promise(resolve => { release = resolve; });
        let observed;
        const requested = new Promise(resolve => { observed = resolve; });
        let held = false;
        await page.route("**/admin/command", async route => {
            if (!held && route.request().postDataJSON().command === "hub_network_snapshot") {
                held = true;
                const response = await route.fetch();
                observed();
                await delayed;
                await route.fulfill({ response });
            } else await route.continue();
        });
        await requested;
        await page.locator("#web-auth-logout").click();
        await expect(page.locator("#management-app")).toBeHidden();
        release();
        await expect(other.locator("#management-app")).toBeHidden();
        await expect(other.locator("#edit-dialog")).toHaveJSProperty("open", false);
        await expect(other.locator("#web-auth-title")).toBeVisible();
        await expect(page.locator("#management-app")).toBeHidden();
    } finally { await other.close(); await provider.close(); }
});

test("a cookie session replaced in another tab cannot execute the old document's commands", async ({ page, context, hub }) => {
    await bootstrap(page, hub);
    const session = await (await context.request.get(new URL("/admin/session", hub.url).href)).json();
    const staleResponse = page.waitForResponse(response => response.url().endsWith("/admin/command") && response.status() === 401);
    const grant = await hub.command("hub_web_open");
    const replacement = await context.request.post(new URL("/admin/access", hub.url).href, {
        headers: { Origin: new URL(hub.url).origin, "X-Moyai-Csrf": session.csrf },
        data: { ticket: new URLSearchParams(new URL(grant.url).hash.slice(1)).get("access") },
    });
    expect(replacement.ok()).toBe(true);
    const next = await replacement.json();
    expect(next.authentication.session_id).not.toBe(session.authentication.session_id);
    await staleResponse;
    await expect(page.locator("#management-app")).toBeHidden();
    await expect(page.locator("#web-auth-title")).toBeVisible();
    await page.locator("#web-auth-refresh").click();
    await expect(page.locator("#management-app")).toBeVisible();
    const request = await page.waitForRequest(request => request.url().endsWith("/admin/command"));
    expect(request.headers()["x-moyai-session"]).toBe(next.authentication.session_id);
});

test.describe("remote first setup", () => {
    test.use({ remoteWeb: true });
    test("the remote listener guides first setup to the Hub PC", async ({ page, hub }, testInfo) => {
        await page.goto(hub.remoteUrl);
        await expect(page.locator("#web-auth-help")).toContainText("Hubを設置したPC");
        await expect(page.locator("#web-auth-help")).toContainText("管理を許可されたPC");
        await expect(page.locator("#management-app")).toBeHidden();
        await expect(page.locator('#web-authentication input[type="password"]')).toHaveCount(0);
        await expect(page.getByRole("button", { name: "管理者を設定してログイン", exact: true })).toHaveCount(0);
        await testInfo.attach("https-login-guidance", { body: await page.screenshot(), contentType: "image/png" });
    });
});
