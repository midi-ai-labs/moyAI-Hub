import { test, expect } from "./fixture.mjs";
import { createMetadataProvider } from "./provider-fixture.mjs";

async function ready(page, url) {
    await page.goto(url);
    await expect(page.locator("#management-status")).toHaveText("Hub本体に接続中");
}
async function polls(page, count = 2) {
    for (let index = 0; index < count; ++index) await page.waitForResponse(response => {
        if (!response.url().endsWith("/admin/command")) return false;
        return response.request().postDataJSON()?.command === "hub_snapshot";
    });
}
async function screenshot(page, testInfo, name) {
    const file = testInfo.outputPath(`${name}.png`);
    await page.screenshot({ path: file });
    await testInfo.attach(name, { path: file, contentType: "image/png" });
}

test("Model registration, profile discovery, edit and deletion use the real catalog", { tag: "@smoke" }, async ({ page, hub }, testInfo) => {
    const provider = await createMetadataProvider();
    const errors = []; page.on("pageerror", error => errors.push(error.message));
    try {
        await ready(page, hub.url);
        await page.locator('nav a[href="#models"]').click();
        await expect(page.locator("#model")).toBeDisabled();
        await expect(page.locator("#register")).toBeDisabled();
        await page.locator("#endpoint").fill("ftp://127.0.0.1");
        await page.locator("#discover").click();
        await expect(page.locator("#discovery-status")).toContainText("HTTP(S)");
        await page.locator("#endpoint").fill(provider.url);
        for (const profile of ["openai_compatible_chat", "lmstudio_chat", "openai_compatible_responses", "lmstudio_responses"]) {
            await page.locator("#profile").selectOption(profile);
            await page.locator("#discover").click();
            await expect(page.locator("#model")).toBeEnabled();
            await expect(page.locator("#model option")).toHaveText(profile.startsWith("lmstudio")
                ? ["モデルを選択", "fixture-native — fixture-native"]
                : ["モデルを選択", "fixture-alpha — fixture-alpha", "fixture-beta — fixture-beta"]);
        }
        await page.locator("#profile").selectOption("openai_compatible_chat");
        await page.locator("#discover").click();
        await expect(page.locator("#model option")).toHaveCount(3);
        await page.locator("#model").selectOption("fixture-alpha");
        await page.locator("#label").fill("Alpha <Hub> & tools");
        await page.locator("#allow-tools").check();
        await page.locator(".registration-options > summary").click();
        await page.locator("#capacity").fill("0");
        await page.locator("#register").click();
        expect((await hub.command("hub_snapshot")).deployments).toHaveLength(0);
        await page.locator("#capacity").fill("2");
        const before = await hub.command("hub_snapshot");
        await page.locator("#register").click();
        const row = page.locator("#model-rows tr");
        await expect(row).toHaveCount(1);
        await expect(row).toContainText("Alpha <Hub> & tools");
        await expect(row).toContainText("ツール利用");
        await expect(row).toContainText("一覧あり");
        const registered = await hub.command("hub_snapshot");
        expect(BigInt(registered.store.revision)).toBeGreaterThan(BigInt(before.store.revision));
        await expect(page.locator("#metric-revision")).toHaveText(registered.store.revision);
        await expect(page.locator("#revision-rows")).toContainText("モデル追加 1 件");

        await row.getByRole("button", { name: /を編集/ }).click();
        await expect(page.locator("#edit-dialog")).toBeVisible();
        await page.mouse.click(4, 4);
        await expect(page.locator("#edit-dialog")).toBeVisible();
        await page.locator("#edit-label").fill("Alpha maintained");
        await polls(page);
        await expect(page.locator("#edit-label")).toBeFocused();
        await expect(page.locator("#edit-label")).toHaveValue("Alpha maintained");
        await page.locator("#edit-capacity").fill("3");
        await page.locator("#edit-enabled").uncheck();
        await screenshot(page, testInfo, "model-edit-draft");
        await page.locator("#save-edit").click();
        await expect(page.locator("#edit-dialog")).not.toBeVisible();
        await expect(row).toContainText("Alpha maintained");
        await expect(row).toContainText("メンテナンス");
        provider.setModels([]);
        await row.getByRole("button", { name: /を編集/ }).click();
        await page.locator("#edit-enabled").check();
        await page.locator("#save-edit").click();
        await expect(page.locator("#edit-dialog")).not.toBeVisible();
        await page.locator("#refresh-models").click();
        await expect(row).toContainText("モデルなし");
        provider.setModels(["fixture-alpha"]);
        await page.locator("#refresh-models").click();
        await expect(row).toContainText("一覧あり");
        await screenshot(page, testInfo, "model-catalog");
        await row.getByRole("button", { name: /を編集/ }).click();
        await page.locator("#remove-model").click();
        await expect(page.locator("#remove-model")).toHaveText("削除を確定");
        await page.locator("#close-edit").click();
        expect((await hub.command("hub_snapshot")).deployments).toHaveLength(1);
        await row.getByRole("button", { name: /を編集/ }).click();
        await expect(page.locator("#remove-model")).toHaveText("配置を削除");
        await page.locator("#remove-model").click();
        await page.locator("#remove-model").click();
        await expect(row).toHaveCount(0);
        await expect(page.locator("#models-empty")).toBeVisible();
        await expect(page.locator("#revision-rows")).toContainText("モデル削除 1 件");
        await page.reload();
        await expect(page.locator("#model-rows tr")).toHaveCount(0);
        await expect(page.locator("#revision-rows")).toContainText("モデル削除 1 件");
        expect(errors).toEqual([]);
    } finally { await provider.close(); }
});

test("Delayed discovery from a previous host cannot replace the selected host's models", async ({ page, hub }, testInfo) => {
    const older = await createMetadataProvider(), current = await createMetadataProvider();
    older.setModels(["old-host-model"]); current.setModels(["current-host-model"]);
    const hold = older.holdNext();
    try {
        await ready(page, hub.url);
        await page.locator("#endpoint").fill(older.url);
        await page.locator("#discover").click(); await hold.arrival;
        await page.locator("#endpoint").fill(current.url);
        await expect(page.locator("#model")).toBeDisabled();
        await page.locator("#discover").click();
        await expect(page.locator("#model option")).toHaveText(["モデルを選択", "current-host-model — current-host-model"]);
        await page.locator("#model").selectOption("current-host-model");
        hold.release(); await polls(page);
        await expect(page.locator("#model")).toHaveValue("current-host-model");
        await expect(page.locator("#model option")).toHaveCount(2);
        await page.locator("#label").fill("Current host registration");
        await page.locator("#register").click();
        await expect(page.locator("#model-rows")).toContainText("Current host registration");
        const catalog = await hub.command("hub_snapshot");
        expect(catalog.deployments[0].deployment.endpoint).toBe(current.url);
        expect(catalog.deployments[0].deployment.provider_model).toBe("current-host-model");
        await screenshot(page, testInfo, "discovery-current-host");
    } finally { hold.release(); await older.close(); await current.close(); }
});

test("Unavailable and empty providers clear candidates, and shared model capacity stays consistent", async ({ page, hub }, testInfo) => {
    const provider = await createMetadataProvider();
    try {
        await ready(page, hub.url);
        await page.locator("#endpoint").fill(`${provider.url}/unavailable`);
        await page.locator("#discover").click();
        await expect(page.locator("#discovery-status")).toHaveClass(/error/);
        await expect(page.locator("#model")).toBeDisabled();
        await expect(page.locator("#register")).toBeDisabled();
        provider.setModels([]);
        await page.locator("#endpoint").fill(provider.url);
        await page.locator("#discover").click();
        await expect(page.locator("#discovery-status")).toContainText("利用できるモデルがありません");
        await expect(page.locator("#model option")).toHaveCount(1);
        provider.setModels(["fixture-alpha", "fixture-beta"]);
        await page.locator("#discover").click();
        await expect(page.locator("#model option")).toHaveCount(3);
        await page.locator("#model").selectOption("fixture-alpha");
        await page.locator("#label").fill("Primary deployment");
        await page.locator(".registration-options > summary").click();
        await page.locator("#capacity").fill("2");
        await page.locator("#register").click();
        await expect(page.locator("#model-rows tr")).toHaveCount(1);
        const first = (await hub.command("hub_snapshot")).deployments[0].deployment;
        await page.locator("#model").selectOption("fixture-beta");
        await page.locator("#label").fill("Secondary deployment");
        await page.locator("#logical-model").selectOption(first.logical_model_id);
        await page.locator("#capacity-pool").selectOption(first.capacity_pool_id);
        await expect(page.locator("#capacity")).toBeDisabled();
        await page.locator("#register").click();
        await expect(page.locator("#model-rows tr")).toHaveCount(2);
        const state = await hub.command("hub_snapshot");
        expect(state.store.catalog.models).toHaveLength(1);
        expect(state.store.catalog.capacity_pools).toHaveLength(1);
        expect(state.deployments.every(row => row.deployment.capacity_pool_id === first.capacity_pool_id)).toBe(true);
        await page.getByRole("button", { name: "Secondary deployment を編集", exact: true }).click();
        await expect(page.locator("#edit-capacity")).toHaveValue("2");
        await page.locator("#edit-capacity").fill("4");
        await page.locator("#save-edit").click();
        await expect(page.locator("#edit-dialog")).not.toBeVisible();
        await page.getByRole("button", { name: "Primary deployment を編集", exact: true }).click();
        await expect(page.locator("#edit-capacity")).toHaveValue("4");
        await page.locator("#close-edit").click();
        await screenshot(page, testInfo, "model-shared-capacity");
        await page.reload();
        await expect(page.locator("#model-rows tr")).toHaveCount(2);
        await expect(page.locator("#metric-models")).toHaveText("1");
    } finally { await provider.close(); }
});

test("Groups and directed rules preserve drafts and reject stale edits across two browsers", async ({ page, context, hub }, testInfo) => {
    await ready(page, hub.url);
    await page.locator('nav a[href="#device-network"]').click();
    await page.locator("#network-group-label").fill("Clients");
    await page.getByRole("button", { name: "グループを保存", exact: true }).click();
    await expect(page.locator("#network-group-list")).toContainText("Clients");
    const other = await context.newPage();
    try {
        await ready(other, hub.url);
        const clients = (await hub.command("hub_network_snapshot")).groups[0];
        await page.locator("#network-group-id").selectOption(clients.id);
        await page.locator("#network-group-label").fill("Clients revised");
        await other.locator("#network-group-label").fill("Servers");
        await other.getByRole("button", { name: "グループを保存", exact: true }).click();
        await expect(other.locator("#network-group-list")).toContainText("Servers");
        await polls(page);
        await expect(page.locator("#network-group-label")).toHaveValue("Clients revised");
        await expect(page.locator("#network-group-label")).toBeFocused();
        await page.getByRole("button", { name: "グループを保存", exact: true }).click();
        await expect(page.locator("#network-notice")).toContainText("端末設定が変更されました");
        await expect(page.locator("#network-group-label")).toHaveValue("Clients revised");
        await screenshot(page, testInfo, "group-stale-revision");
        await page.locator("#network-refresh").click();
        await expect(page.locator("#network-notice")).toContainText("入力中の内容は保持");
        await page.getByRole("button", { name: "グループを保存", exact: true }).click();
        await expect(page.locator("#network-group-list")).toContainText("Clients revised");
        const snapshot = await hub.command("hub_network_snapshot");
        const servers = snapshot.groups.find(group => group.label === "Servers");
        await page.locator("#network-rule-from").selectOption(`group:${clients.id}`);
        await page.locator("#network-rule-to").selectOption(`group:${servers.id}`);
        await page.locator("#network-rule-usage").selectOption("delegated");
        await page.locator("#network-rule-depth").selectOption("2");
        await page.getByRole("button", { name: "接続ルールを追加", exact: true }).click();
        await expect(page.locator("#network-rule-list")).toContainText("Clients revised → グループ: Servers");
        await expect(page.locator("#network-rule-list")).toContainText("再委任の宛先");
        await expect(page.locator("#network-rule-list")).toContainText("最大2段");
        const rule = (await hub.command("hub_network_snapshot")).rules[0];
        expect(rule.from).toEqual({ kind: "group", id: clients.id });
        expect(rule.to).toEqual({ kind: "group", id: servers.id });
        expect(rule.usage).toBe("delegated"); expect(rule.effect).toBe("allow");
        await page.locator("#network-rule-effect").selectOption("deny");
        await page.locator("#network-rule-usage").selectOption("direct");
        await page.locator("#network-rule-depth").selectOption("1");
        await page.getByRole("button", { name: "接続ルールを追加", exact: true }).click();
        await expect(page.locator("#network-rule-list .network-rule")).toHaveCount(2);
        await screenshot(page, testInfo, "directed-rules");
        await page.locator(`#network-rule-list button[data-network-delete-rule="${rule.id}"]`).click();
        await expect(page.locator("#network-rule-list .network-rule")).toHaveCount(1);
        await page.reload();
        await expect(page.locator("#network-rule-list .network-rule")).toHaveCount(1);
        await expect(page.locator("#network-rule-list")).toContainText("拒否");
        await page.getByRole("link", { name: "接続端末", exact: false }).first().click();
        await page.locator("#network-filter").selectOption("pending");
        await expect(page.locator("#network-devices-empty")).toContainText("条件に一致する承認待ち");
        await page.locator("#network-search").fill("no-such-device");
        await polls(page);
        await expect(page.locator("#network-search")).toHaveValue("no-such-device");
        await expect(page.locator("#network-search")).toBeFocused();
        await page.locator('nav a[href="#mcp-history"]').click();
        await expect(page.locator("#history-request")).toBeDisabled();
    } finally { await other.close(); }
});

test("Network diagnostics and certificate renewal preserve expanded details and settings drafts", async ({ page, hub }, testInfo) => {
    await ready(page, hub.url);
    await page.locator('nav a[href="#device-network"]').click();
    await expect(page.locator("#network-diagnose")).toBeDisabled();
    const details = page.locator('details[data-details-key="network-connection-details"]');
    await details.locator("summary").click();
    await page.locator("#network-gateway-port").fill("65536");
    await page.getByRole("button", { name: "ポート設定を保存", exact: true }).click();
    await expect(page.locator("#network-notice")).toContainText("1〜65535");
    expect((await hub.command("hub_network_snapshot")).gateway_port).toBe(0);
    await page.locator("#network-gateway-port").fill("0");
    await page.getByRole("button", { name: "ポート設定を保存", exact: true }).click();
    await expect(page.locator("#network-notice")).toContainText("ポート設定を保存しました");
    await page.locator("#network-ip").fill("127.0.0.1");
    await page.locator("#network-port").fill(String(hub.networkPort));
    await page.locator("#network-start").click();
    await expect(page.locator("#network-stop")).toBeVisible();
    const before = await hub.observeNetwork();
    await expect(page.locator("#network-certificate-status")).toContainText("証明書は有効です");
    await page.locator("#network-diagnose").click();
    await expect(page.locator("#network-diagnostic-result")).toBeVisible();
    await expect(page.locator("#network-diagnostic-result .badge")).toHaveText(["確認済み", "確認済み", "未実施"]);
    await page.locator("#network-gateway-port").fill("12345");
    await polls(page, 3);
    await expect(details).toHaveAttribute("open", "");
    await expect(page.locator("#network-gateway-port")).toBeFocused();
    await expect(page.locator("#network-gateway-port")).toHaveValue("12345");
    await expect(page.locator("#network-diagnostic-result")).toBeVisible();
    await screenshot(page, testInfo, "network-diagnostics-draft");
    await page.locator("#network-renew-certificate").click();
    await expect(page.locator("#network-notice")).toContainText("証明書を更新しました");
    const after = await hub.observeNetwork();
    expect(after.server.certificate.sha256).not.toBe(before.server.certificate.sha256);
    await expect(page.locator("#network-gateway-port")).toHaveValue("12345");
    await expect(page.locator("#network-diagnostic-result")).not.toBeVisible();
    await expect(details).toHaveAttribute("open", "");
    await page.locator("#network-diagnose").click();
    await expect(page.locator("#network-diagnostic-result .badge")).toHaveText(["確認済み", "確認済み", "未実施"]);
    await page.setViewportSize({ width: 760, height: 1000 });
    await page.locator("#network-diagnose").scrollIntoViewIfNeeded();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await screenshot(page, testInfo, "network-narrow-diagnostics");
    await page.locator("#network-stop").click();
    await expect(page.locator("#network-start")).toBeVisible();
    await expect(page.locator("#network-diagnostic-result")).not.toBeVisible();
    await expect(page.locator("#network-renew-certificate")).toBeDisabled();
    await page.reload();
    await expect(page.locator("#network-gateway-port")).toHaveValue("");
    expect((await hub.command("hub_web_status")).enabled).toBe(true);
});
