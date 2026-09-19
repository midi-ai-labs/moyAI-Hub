import { readFile } from "node:fs/promises";
import { test, expect } from "./fixture.mjs";
import { createDeviceParticipant } from "./device-fixture.mjs";

async function startNetwork(page, hub) {
    await page.goto(hub.url);
    await expect(page.locator("#management-status")).toHaveText("Hub本体に接続中");
    await page.locator("#network-ip").fill("127.0.0.1");
    await page.locator("#network-port").fill(String(hub.networkPort));
    await page.locator("#network-start").click();
    await expect(page.locator("#network-stop")).toBeVisible();
    return hub.observeNetwork();
}
async function approve(page, participant, label, rename = null) {
    await page.locator('nav a[href="#clients"]').click();
    await page.locator("#network-clients-refresh").click();
    const row = page.locator(`[data-id="request:${participant.requestId}"]`);
    await expect(row).toContainText("承認待ち");
    if (rename) {
        await row.getByRole("button", { name: `${label}の名前を確認`, exact: true }).click();
        await page.locator("#network-device-label").fill(rename);
        await page.locator("#network-device-save").click();
        await expect(page.locator("#network-device-dialog")).not.toBeVisible();
    } else await row.getByRole("button", { name: `${label}の許可`, exact: true }).click();
    await expect(row).toHaveCount(0);
    const result = await participant.collectApproval();
    expect(result.status).toBe("approved");
    await participant.presence();
    await page.locator("#network-clients-refresh").click();
    return result.deviceId;
}
async function capture(page, testInfo, name) {
    const path = testInfo.outputPath(`${name}.png`);
    await page.screenshot({ path });
    await testInfo.attach(name, { path, contentType: "image/png" });
}

test("Device admission, duplicate-name validation, retained legacy memberships, stop and deletion target the selected participant", { tag: "@smoke" }, async ({ page, hub }, testInfo) => {
    const network = await startNetwork(page, hub);
    // Legacy ACL data remains compatible; it is setup through the existing
    // administrative API, not through removed group/rule UI controls.
    let legacy=await hub.command("hub_network_snapshot");
    await hub.command("hub_network_group",{expectedRevision:legacy.revision,id:null,label:"Servers"});
    const first = await createDeviceParticipant(network, "WinB fixture");
    const firstId = await approve(page, first, "WinB fixture");
    legacy=await hub.command("hub_network_snapshot");
    await hub.command("hub_network_device",{expectedRevision:legacy.revision,deviceId:firstId,label:"WinB fixture",groups:[legacy.groups[0].id],revoked:false});
    await expect(createDeviceParticipant(network, "WinB fixture")).rejects.toThrow("device_name_conflict");
    const second = await createDeviceParticipant(network, "WinC pending");
    const secondId = await approve(page, second, "WinC pending", "WinC fixture");
    const firstRow = page.locator(`[data-id="device:${firstId}"]`);
    const secondRow = page.locator(`[data-id="device:${secondId}"]`);
    await expect(firstRow).toContainText("オンライン");
    const legacyStatus=firstRow.locator("details[data-network-legacy-mcp]");
    await expect(legacyStatus).not.toHaveAttribute("open", "");
    await expect(legacyStatus.getByText("受付の申告なし",{exact:true})).toBeHidden();
    await expect(firstRow).toContainText("使い方はプロジェクトで設定します。");
    await expect(firstRow).not.toContainText("接続ルールが必要");
    await first.publish({ profile_id: "fixture-project", name: "Fixture project", endpoint: "https://127.0.0.1:7332/mcp",
        mode: "agent", scope_id: "fixture-scope", enabled: true });
    await page.locator("#network-clients-refresh").click();
    await expect(legacyStatus.getByText("受付申告あり",{exact:true})).toBeHidden();
    await legacyStatus.getByText("以前のMCP連携",{exact:true}).click();
    await expect(legacyStatus.getByText("受付申告あり",{exact:true})).toBeVisible();
    await legacyStatus.getByText("以前のMCP連携",{exact:true}).click();
    await firstRow.locator("details[data-device-identity] > summary").click();
    await firstRow.getByRole("button", { name: "WinB fixtureを管理", exact: true }).click();
    await page.locator("#network-device-label").fill(" winc FIXTURE ");
    await page.locator("#network-device-save").click();
    await expect(page.locator("#network-device-error")).not.toBeEmpty();
    expect((await hub.command("hub_network_snapshot")).devices.find(device => device.device_id === firstId).label).toBe("WinB fixture");
    await page.locator("#network-device-label").fill("WinB renamed");
    await expect(page.locator("#network-device-groups")).toBeHidden();
    await page.mouse.click(4, 4);
    await expect(page.locator("#network-device-dialog")).toBeVisible();
    await capture(page, testInfo, "device-rename-membership");
    await page.locator("#network-device-save").click();
    await expect(page.locator("#network-device-dialog")).not.toBeVisible();
    await expect(firstRow).toContainText("WinB renamed");
    await expect(firstRow.locator("td").nth(3)).toBeHidden();
    expect((await hub.command("hub_network_snapshot")).devices.find(device=>device.device_id===firstId).groups).toEqual([legacy.groups[0].id]);
    await expect(firstRow.locator("details[data-device-identity]")).toHaveAttribute("open", "");
    await page.locator("#network-search").fill("WinB renamed");
    await expect(page.locator("#network-device-rows tr")).toHaveCount(1);
    await expect(firstRow).toBeVisible();
    await page.locator("#network-search").fill("");
    await page.locator('nav a[href="#device-network"]').click();
    legacy=await hub.command("hub_network_snapshot");
    await hub.command("hub_network_rule",{expectedRevision:legacy.revision,rule:{id:null,from:{kind:"device",id:secondId},to:{kind:"device",id:firstId},usage:"direct",effect:"allow",max_depth:1,profile_id:"fixture-project"}});
    const configuredLegacy = await hub.command("hub_network_snapshot");
    expect(configuredLegacy.rules[0].profile_id).toBe("fixture-project");
    await expect(page.locator("#network-rule-form")).toBeHidden();
    await page.locator('nav a[href="#clients"]').click();
    // API fixture setup is an external edit: observe its revision before the
    // next user command so this lifecycle test does not submit an old CAS.
    await page.locator("#network-clients-refresh").click();
    await expect(page.locator("#network-identity")).toContainText(`更新 ${configuredLegacy.revision}`);
    await firstRow.getByRole("button", { name: "WinB renamedの停止", exact: true }).click();
    await expect(firstRow).toContainText("新規利用を停止");
    await expect(secondRow.getByRole("button", { name: "WinC fixtureの停止", exact: true })).toBeEnabled();
    await firstRow.getByRole("button", { name: "WinB renamedの再許可", exact: true }).click();
    await expect(firstRow.getByRole("button", { name: "WinB renamedの停止", exact: true })).toBeEnabled();
    await secondRow.getByRole("button", { name: "WinC fixtureを管理", exact: true }).click();
    await page.locator("#network-device-revoke-details > summary").click();
    await page.locator("#network-device-revoked").check();
    await page.locator("#network-device-save").click();
    await expect(page.locator("#network-device-dialog")).not.toBeVisible();
    await expect(secondRow).toContainText("恒久失効");
    await expect(secondRow.getByRole("button", { name: "WinC fixtureの再許可", exact: true })).toHaveCount(0);
    await firstRow.getByRole("button", { name: "WinB renamedの登録を削除", exact: true }).click();
    await expect(page.locator("#network-delete-identity")).toContainText(firstId);
    await page.locator("#network-delete-cancel").click();
    await expect(firstRow).toBeVisible();
    await firstRow.getByRole("button", { name: "WinB renamedの登録を削除", exact: true }).click();
    await page.locator("#network-delete-confirm").click();
    await expect(firstRow).toHaveCount(0);
    await expect(secondRow).toBeVisible();
    const state = await hub.command("hub_network_snapshot");
    expect(state.devices.map(device => device.device_id)).toEqual([secondId]);
    expect(state.deleted_devices.some(device => device.device_id === firstId)).toBe(true);
    expect(state.rules).toHaveLength(0);
    await page.locator('details[data-details-key="network-deleted-devices"] > summary').click();
    await expect(page.locator("#network-deleted-list")).toContainText("WinB renamed");
    await capture(page, testInfo, "device-registry-deletion");
    await page.reload();
    await expect(firstRow).toHaveCount(0);
    await expect(secondRow).toContainText("恒久失効");
});

const historyRow = (id, direction, title) => ({
    id, direction, created_at_ms: 1700000000000, updated_at_ms: 1700000001000,
    title, peer_label: "WinA fixture", target_label: "temp", state: "completed", stop_status: "none",
    state_source: direction === "instruction" ? "last_observed" : "local_runtime",
    result_received: true, session_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV", profile_id: "fixture-project",
    job_id: null, root_task_id: "fixture-root", device_path: ["fixture-origin"], can_stop: false,
});
async function answer(participant, response, predicate = () => true, error = null) {
    let pending;
    await expect.poll(async () => {
        const requests = await participant.historyRequests();
        pending = requests.find(predicate);
        return !!pending;
    }).toBe(true);
    await participant.submitHistory(pending.request_id, response, error);
    return pending;
}

test("History list, pagination, detail and Markdown export use the authenticated participant response", async ({ page, hub }, testInfo) => {
    const participant = await createDeviceParticipant(await startNetwork(page, hub), "History fixture");
    const deviceId = await approve(page, participant, "History fixture");
    await page.locator(`[data-id="device:${deviceId}"]`).getByRole("button", { name: "History fixtureのMCP履歴を見る", exact: true }).click();
    await expect(page.locator("#history-device")).toHaveValue(deviceId);
    // The row shortcut selects this device and immediately requests its history.
    await expect(page.locator("#history-request")).toBeDisabled();
    await expect(page.locator("#history-notice")).toContainText("端末から履歴を取得しています");
    await answer(participant, null, () => true, "fixture_read_failure");
    await expect(page.locator("#history-notice")).toContainText("端末で履歴を取得できませんでした");
    const first = historyRow("01ARZ3NDEKTSV4RRFFQ69G5FAV", "instruction", "First <instruction>");
    const second = historyRow("01ARZ3NDEKTSV4RRFFQ69G5FAW", "instruction", "Second instruction");
    await page.locator("#history-request").click();
    await answer(participant, { rows: [first], next_offset: 1, anchor: "instruction:2" });
    await expect(page.locator("#history-rows")).toContainText(first.title);
    await expect(page.locator("#history-rows")).toContainText("指示側に保存された最終確認");
    await expect(page.locator("#history-next")).toBeEnabled();
    await page.locator("#history-next").click();
    const next = await answer(participant, { rows: [second], next_offset: null, anchor: "instruction:2" });
    expect(next.offset).toBe(1); expect(next.anchor).toBe("instruction:2");
    await expect(page.locator("#history-rows")).toContainText(second.title);
    await expect(page.locator("#history-next")).toBeDisabled();
    await page.locator("#history-previous").click();
    await answer(participant, { rows: [first], next_offset: 1, anchor: "instruction:2" });
    await expect(page.locator("#history-rows")).toContainText(first.title);
    await page.getByRole("button", { name: `${first.title}の詳細を見る`, exact: true }).click();
    const markdown = "# Fixture instruction\n\n<script>window.fixtureInjected = true</script>\n\n保存された取得結果です。\n\n"
        + Array.from({ length: 160 }, (_, index) => `取得済み本文の行 ${index + 1}。この本文は読み取り専用です。`).join("\n") + "\nHUB_HISTORY_END_MARKER";
    const detail = await answer(participant, { row: first, markdown, truncated: true });
    expect(detail.id).toBe(first.id);
    await expect(page.locator("#history-markdown")).toHaveText(markdown);
    expect(await page.evaluate(() => window.fixtureInjected)).toBeUndefined();
    await expect(page.locator("#history-truncated")).toBeVisible();
    await capture(page, testInfo, "history-detail-truncated");
    const pendingDownload = page.waitForEvent("download");
    await page.locator("#history-export").click();
    const download = await pendingDownload;
    expect(download.suggestedFilename()).toMatch(/\.md$/);
    const path = testInfo.outputPath(download.suggestedFilename());
    await download.saveAs(path);
    expect(await readFile(path, "utf8")).toContain(markdown);
    const body = page.locator("#history-markdown");
    // Export temporarily disables its button, so explicitly acquire the body after download.
    await body.click();
    await expect(body).toBeFocused();
    await page.keyboard.press("Shift+Tab");
    await expect(page.locator("#history-export")).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(body).toBeFocused();
    const geometry = () => body.evaluate(node => ({ top: node.scrollTop, max: node.scrollHeight - node.clientHeight, text: node.textContent }));
    expect((await geometry()).max).toBeGreaterThan(200);
    await page.keyboard.press("PageDown");
    await expect.poll(async () => (await geometry()).top).toBeGreaterThan(0);
    await page.keyboard.press("Control+End");
    await expect.poll(async () => { const value = await geometry(); return Math.abs(value.max - value.top); }).toBeLessThanOrEqual(2);
    await capture(page, testInfo, "history-keyboard-bottom");
    const bottom = await geometry();
    await page.waitForResponse(response => response.url().endsWith("/admin/command")
        && response.request().postDataJSON()?.command === "hub_snapshot");
    await expect(body).toBeFocused();
    expect((await geometry()).top).toBe(bottom.top);
    await expect(body).toHaveText(markdown);
    await page.keyboard.press("Control+Home");
    await expect.poll(async () => (await geometry()).top).toBe(0);
    await capture(page, testInfo, "history-keyboard-top");
    await testInfo.attach("history-keyboard-scroll", { body: Buffer.from(JSON.stringify({ bottom, returned: await geometry() })), contentType: "application/json" });
    await participant.presence();
    await page.locator("#history-direction").selectOption("execution");
    await expect(page.locator("#history-detail")).not.toBeVisible();
    await expect(page.locator("#history-rows tr")).toHaveCount(0);
    await page.locator("#history-request").click();
    const execution = await answer(participant, { rows: [], next_offset: null, anchor: null });
    expect(execution.direction).toBe("execution");
    await expect(page.locator("#history-empty")).toBeVisible();
    await page.locator("#history-direction").selectOption("instruction");
    await expect(page.locator("#history-rows")).toContainText(first.title);
    await expect(page.locator("#history-notice")).toContainText("以前に取得した履歴");
    // A late instruction response must not replace the execution selection.
    await page.locator("#history-request").click();
    await expect(page.locator("#history-notice")).toContainText("端末から履歴を取得しています");
    await page.locator("#history-direction").selectOption("execution");
    await answer(participant, { rows: [second], next_offset: null, anchor: "instruction:2" });
    await page.waitForResponse(response => response.url().endsWith("/admin/command")
        && response.request().postDataJSON()?.command === "hub_snapshot");
    await expect(page.locator("#history-direction")).toHaveValue("execution");
    await expect(page.locator("#history-empty")).toBeVisible();
    await expect(page.locator("#history-rows tr")).toHaveCount(0);
    await page.locator("#history-direction").selectOption("instruction");
    await expect(page.locator("#history-rows")).toContainText(first.title);
    await page.locator('nav a[href="#clients"]').click();
    const deviceRow = page.locator(`[data-id="device:${deviceId}"]`);
    await deviceRow.getByRole("button", { name: "History fixtureの停止", exact: true }).click();
    await expect(deviceRow).toContainText("新規利用を停止");
    await deviceRow.getByRole("button", { name: "History fixtureのMCP履歴を見る", exact: true }).click();
    await answer(participant, { rows: [first], next_offset: null, anchor: "instruction:2" });
    await expect(page.locator("#history-notice")).toContainText("履歴を取得しました");
    await expect(page.locator("#history-connection")).toContainText("履歴は確認可能");
    await page.locator('nav a[href="#clients"]').click();
    await deviceRow.getByRole("button", { name: "History fixtureの登録を削除", exact: true }).click();
    await page.locator("#network-delete-confirm").click();
    await expect(deviceRow).toHaveCount(0);
    await page.locator('nav a[href="#mcp-history"]').click();
    await expect(page.locator("#history-connection")).toContainText("登録を削除済み");
    await expect(page.locator("#history-request")).toBeDisabled();
    await page.getByRole("button", { name: `${first.title}の詳細を見る`, exact: true }).click();
    await expect(page.locator("#history-notice")).toContainText("以前に取得した詳細");
    await expect(page.locator("#history-markdown")).toHaveText(markdown);
    await expect(page.locator("#history-export")).toBeEnabled();
    await capture(page, testInfo, "history-deleted-device-cache");
});
