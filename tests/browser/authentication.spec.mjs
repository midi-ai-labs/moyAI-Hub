import { test, expect, administrator } from "./fixture.mjs";
import { createMetadataProvider } from "./provider-fixture.mjs";

test.use({ authenticatedAdmin: false });

async function bootstrap(page, hub) {
    await page.goto(hub.url);
    await expect(page.getByRole("heading", { name: "初回の管理者設定", exact: true })).toBeVisible();
    await page.getByLabel("ユーザー名", { exact: true }).fill(administrator.username);
    await page.locator("#web-authentication").getByLabel("表示名", { exact: true }).fill(administrator.display_name);
    await page.getByLabel("パスワード", { exact: true }).fill(administrator.password);
    await page.getByLabel("パスワードの確認", { exact: true }).fill(administrator.password);
    await page.getByRole("button", { name: "管理者を設定してログイン", exact: true }).click();
    await expect(page.locator("#management-app")).toBeVisible();
    await expect(page.locator("#web-auth-user")).toHaveText(administrator.display_name);
}

test("administrator setup, failed login, logout and fresh login use the real human session", async ({ page, hub }, testInfo) => {
    const commands = [];
    page.on("request", request => { if (request.url().endsWith("/admin/command")) commands.push(request); });
    await page.goto(hub.url);
    await expect(page.getByRole("heading", { name: "初回の管理者設定", exact: true })).toBeVisible();
    await expect(page.locator("#management-app")).toBeHidden();
    expect(commands).toHaveLength(0);
    const sessionResponse = await page.request.get(new URL("/admin/session", hub.url).href);
    const session = await sessionResponse.json();
    const forbidden = await page.request.post(new URL("/admin/command", hub.url).href, {
        headers: { Origin: new URL(hub.url).origin, "X-Moyai-Csrf": session.csrf },
        data: { command: "hub_snapshot", args: {} },
    });
    expect(forbidden.status()).toBe(401);
    await testInfo.attach("initial-administrator-setup", { body: await page.screenshot(), contentType: "image/png" });
    await page.getByLabel("ユーザー名", { exact: true }).fill(administrator.username);
    await page.locator("#web-authentication").getByLabel("表示名", { exact: true }).fill(administrator.display_name);
    await page.getByLabel("パスワード", { exact: true }).fill(administrator.password);
    await page.getByLabel("パスワードの確認", { exact: true }).fill("different-password");
    await page.getByRole("button", { name: "管理者を設定してログイン", exact: true }).click();
    await expect(page.getByRole("alert")).toContainText("一致しません");
    await expect(page.getByLabel("パスワード", { exact: true })).toHaveValue("");
    await page.getByLabel("パスワード", { exact: true }).fill(administrator.password);
    await page.getByLabel("パスワードの確認", { exact: true }).fill(administrator.password);
    await page.getByRole("button", { name: "管理者を設定してログイン", exact: true }).click();
    await expect(page.locator("#management-app")).toBeVisible();
    await expect(page.locator("#web-auth-user")).toHaveText(administrator.display_name);
    const cookie = (await page.context().cookies(hub.url)).find(value => value.name === "moyai_admin");
    expect(cookie).toMatchObject({ httpOnly: true, sameSite: "Strict", path: "/admin" });
    expect(await page.evaluate(() => document.cookie)).toBe("");

    await page.getByRole("button", { name: "ログアウト", exact: true }).click();
    await expect(page.locator("#management-app")).toBeHidden();
    await expect(page.getByRole("heading", { name: "管理者ログイン", exact: true })).toBeVisible();
    await expect.poll(async () => (await page.context().cookies(hub.url)).length).toBe(0);
    await testInfo.attach("administrator-login", { body: await page.screenshot(), contentType: "image/png" });
    await page.getByLabel("ユーザー名", { exact: true }).fill(administrator.username);
    await page.getByLabel("パスワード", { exact: true }).fill("incorrect-password");
    await page.getByRole("button", { name: "ログイン", exact: true }).click();
    await expect(page.getByRole("alert")).toContainText("ユーザー名またはパスワード");
    await expect(page.locator("#management-app")).toBeHidden();
    await expect(page.getByLabel("パスワード", { exact: true })).toHaveValue("");
    await page.getByLabel("パスワード", { exact: true }).fill(administrator.password);
    await page.getByRole("button", { name: "ログイン", exact: true }).click();
    await expect(page.locator("#management-app")).toBeVisible();
    await expect(page.locator("#web-auth-user")).toHaveText(administrator.display_name);
    await expect(page.locator("#web-enabled-state")).toContainText("稼働中");
    await testInfo.attach("authenticated-management", { body: await page.screenshot(), contentType: "image/png" });
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
        await page.getByRole("button", { name: "ログアウト", exact: true }).click();
        await expect(page.locator("#management-app")).toBeHidden();
        release();
        await expect(other.locator("#management-app")).toBeHidden();
        await expect(other.locator("#edit-dialog")).toHaveJSProperty("open", false);
        await expect(other.getByRole("heading", { name: "管理者ログイン", exact: true })).toBeVisible();
        await expect(page.locator("#management-app")).toBeHidden();
    } finally { await other.close(); await provider.close(); }
});

test("a cookie session replaced in another tab cannot execute the old document's commands", async ({ page, context, hub }) => {
    await bootstrap(page, hub);
    const session = await (await context.request.get(new URL("/admin/session", hub.url).href)).json();
    const staleResponse = page.waitForResponse(response => response.url().endsWith("/admin/command") && response.status() === 401);
    const replacement = await context.request.post(new URL("/admin/login", hub.url).href, {
        headers: { Origin: new URL(hub.url).origin, "X-Moyai-Csrf": session.csrf },
        data: { username: administrator.username, password: administrator.password },
    });
    expect(replacement.ok()).toBe(true);
    const next = await replacement.json();
    expect(next.authentication.session_id).not.toBe(session.authentication.session_id);
    await staleResponse;
    await expect(page.locator("#management-app")).toBeHidden();
    await expect(page.getByRole("heading", { name: "管理者ログイン", exact: true })).toBeVisible();
    await page.getByRole("button", { name: "ログイン状態を再確認", exact: true }).click();
    await expect(page.locator("#management-app")).toBeVisible();
    const request = await page.waitForRequest(request => request.url().endsWith("/admin/command"));
    expect(request.headers()["x-moyai-session"]).toBe(next.authentication.session_id);
});

test.describe("remote first setup", () => {
    test.use({ remoteWeb: true });
    test("the remote listener guides first setup to the Hub PC", async ({ page, hub }, testInfo) => {
        await page.goto(hub.remoteUrl);
        await expect(page.locator("#web-auth-help")).toContainText("Hubと同じPC");
        await expect(page.locator("#web-auth-help")).toContainText("HTTPSの管理URL");
        await expect(page.locator("#management-app")).toBeHidden();
        await expect(page.locator('#web-authentication input[type="password"]')).toHaveCount(0);
        await expect(page.getByRole("button", { name: "管理者を設定してログイン", exact: true })).toHaveCount(0);
        await testInfo.attach("https-login-guidance", { body: await page.screenshot(), contentType: "image/png" });
    });
});
