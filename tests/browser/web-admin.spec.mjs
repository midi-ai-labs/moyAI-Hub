import { readFile } from "node:fs/promises";
import { test, expect } from "./fixture.mjs";

test("Browser manages the real Hub without a native app or login", { tag: "@smoke" }, async ({ page, hub }, testInfo) => {
    const pageErrors = [];
    page.on("pageerror", error => pageErrors.push(error.message));
    await page.goto(hub.url);
    await expect(page).toHaveURL(hub.url);
    await expect(page.getByRole("button", { name: "ログイン", exact: true })).toHaveCount(0);
    await expect(page.getByLabel("管理キー", { exact: true })).toHaveCount(0);
    expect(await page.context().cookies(hub.url)).toEqual([]);

    await page.getByRole("link", { name: "ネットワーク設定", exact: false }).click();
    await expect(page.getByRole("button", { name: "ネットワークを開始", exact: true })).toBeVisible();
    await page.getByLabel("このHubのIP", { exact: true }).fill("127.0.0.1");
    await page.locator("#network-port").fill(String(hub.networkPort));
    await page.getByRole("button", { name: "ネットワークを開始", exact: true }).click();
    await expect(page.getByRole("button", { name: "ネットワークを停止", exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "設定ファイルを保存", exact: true })).toBeEnabled();
    const network = await hub.observeNetwork();
    expect(network.server.running).toBe(true);
    expect(network.server.bind).toBe(`https://127.0.0.1:${hub.networkPort}`);
    expect(typeof network.server.gateway_bind).toBe("string");
    const group = "Playwright browser verification";
    await page.getByLabel("グループ名", { exact: true }).fill(group);
    await page.getByRole("button", { name: "グループを保存", exact: true }).click();
    await expect(page.locator("#network-group-list")).toContainText(group);
    // Read-only independent consumer confirms the real Admin owner received the browser mutation.
    const snapshot = await hub.command("hub_network_snapshot");
    expect(snapshot.groups.some(item => item.label === group)).toBe(true);
    await page.reload();
    await expect(page.locator("#network-group-list")).toContainText(group);

    const downloadPending = page.waitForEvent("download");
    await page.getByRole("button", { name: "設定ファイルを保存", exact: true }).click();
    const download = await downloadPending;
    expect(download.suggestedFilename()).toBe("hub-participation.toml");
    const downloaded = testInfo.outputPath(download.suggestedFilename());
    await download.saveAs(downloaded);
    const config = await readFile(downloaded, "utf8");
    expect(config).toContain(`https://127.0.0.1:${hub.networkPort}`);
    expect(config).toContain("BEGIN CERTIFICATE");
    expect(config).not.toContain("PRIVATE KEY");
    await testInfo.attach("web-network", { body: await page.screenshot({ fullPage: true }), contentType: "image/png" });

    await page.getByRole("button", { name: "ネットワークを停止", exact: true }).click();
    await expect(page.getByRole("button", { name: "ネットワークを開始", exact: true })).toBeVisible();
    expect((await hub.observeNetwork()).server.running).toBe(false);
    await page.reload();
    await expect(page.locator("#network-group-list")).toContainText(group);
    await expect(page.getByRole("button", { name: "ネットワークを開始", exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "設定ファイルを保存", exact: true })).toBeDisabled();
    expect((await hub.command("hub_web_status")).enabled).toBe(true);

    await page.getByRole("button", { name: "Hub本体へ再接続", exact: true }).click();
    await expect(page.locator("#network-group-list")).toContainText(group);
    await page.locator("#manual-server-connection > summary").click();
    await page.locator("#bind").fill(`127.0.0.1:${hub.networkPort}`);
    await page.locator("#generate-token").click();
    await page.locator("#start-server").click();
    await expect(page.locator("#stop-server")).toBeVisible();
    expect((await hub.command("hub_snapshot")).server.running).toBe(true);
    await page.locator("#stop-server").click();
    await expect(page.locator("#start-server")).toBeEnabled();
    expect((await hub.command("hub_snapshot")).server.running).toBe(false);
    await page.getByRole("link", { name: "ネットワーク設定", exact: false }).click();
    await testInfo.attach("web-network-viewport", { body: await page.screenshot(), contentType: "image/png" });
    await page.setViewportSize({ width: 760, height: 1000 });
    await page.getByRole("link", { name: "Web管理", exact: false }).click();
    await expect(page.locator("#web-enabled-state")).toContainText("稼働中");
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await testInfo.attach("web-narrow-viewport", { body: await page.screenshot(), contentType: "image/png" });
    expect(await page.context().cookies(hub.url)).toEqual([]);
    expect(pageErrors).toEqual([]);
});
