import { readFile } from "node:fs/promises";
import { randomUUID } from "node:crypto";
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
    } else {
        await row.getByRole("button", { name: `${label}の許可`, exact: true }).click();
        await page.locator("#join-project-save").click();
        await expect(page.locator("#join-project-dialog")).not.toBeVisible();
    }
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

async function createProject(page, label) {
    await page.locator("#onboarding-new-project").click();
    await page.locator("#shared-admin-label").fill(label);
    const sent = page.waitForRequest(request => request.url().endsWith("/admin/command")
        && request.postDataJSON()?.command === "hub_shared_command");
    await page.locator("#shared-admin-save").click();
    const id = (await sent).postDataJSON().args.request.id;
    await expect(page.locator("#shared-admin-form")).toHaveCount(0);
    return id;
}

async function externalSharedCommand(page, hub, operation) {
    // An authenticated external writer changes the real persisted project. The
    // observed page receives no manual refresh, focus change, or UI mutation.
    const session = await (await page.request.get(new URL("/admin/session", hub.url).href)).json();
    const command = async (name, args = {}) => {
        const response = await page.request.post(new URL("/admin/command", hub.url).href, {
            headers: { Origin: new URL(hub.url).origin, "X-Moyai-Csrf": session.csrf,
                "X-Moyai-Session": session.authentication.session_id },
            data: { command: name, args },
        });
        expect(response.ok(), await response.text()).toBe(true);
        return (await response.json()).result;
    };
    const snapshot = await command("hub_shared_snapshot");
    return command("hub_shared_command", { request_id: randomUUID(), expected_revision: snapshot.revision,
        request: operation });
}

for (const selection of ["direct", "pointer-popup", "keyboard-popup"]) {
const keyboard = selection !== "direct";
test(selection === "keyboard-popup" ? "Automatic refresh continues after keyboard opening of the native project selector"
    : keyboard ? "Automatic refresh continues after native keyboard project selection"
    : "Automatic refresh updates approved PC names, purposes, presence and the next request without manual refresh", async ({ page, hub }, testInfo) => {
    const keyEvents = [];
    if (keyboard) {
        await page.exposeFunction("recordTestKeyboardEvent", value => { if (keyEvents.length < 100) keyEvents.push(value); });
        await page.addInitScript(() => {
            for (const type of ["keydown", "keyup"]) document.addEventListener(type, event => {
                void window.recordTestKeyboardEvent({ type, key: event.key, code: event.code,
                    tag: event.target?.tagName, id: event.target?.id });
            }, true);
        });
    }
    try {
    await startNetwork(page, hub);
    const projectId = await createProject(page, "自動更新の仕事");
    const first = await createDeviceParticipant(await hub.observeNetwork(), "DESKTOP-automatic");
    const pending = page.locator(`[data-id="request:${first.requestId}"]`);
    await expect(pending).toContainText("承認待ち");
    await pending.getByRole("button", { name: "DESKTOP-automaticの許可", exact: true }).click();
    await page.locator("#join-project-label").fill("自動更新 WinA");
    if (keyboard) {
        if (selection === "keyboard-popup") {
            // Tab is pressed on the input and released on the select. Its
            // tracked key must still be cleared across that focus transition.
            await page.locator("#join-project-label").press("Tab");
            await expect(page.locator("#join-project-project")).toBeFocused();
            await page.keyboard.press("Alt+ArrowDown");
        } else await page.locator("#join-project-project").click();
        await page.keyboard.press("ArrowDown");
        await page.keyboard.press("Enter");
        await expect(page.locator("#join-project-project")).toHaveValue(projectId);
    } else await page.locator("#join-project-project").selectOption(projectId);
    await page.locator("#join-project-control").check();
    await page.locator("#join-project-execute").check();
    await page.locator("#join-project-save").click();
    await expect(page.locator("#join-project-dialog")).not.toBeVisible();
    // No observed-page interaction follows approval: all changes below must
    // arrive through the ordinary automatic refresh owner.
    const approval = await first.collectApproval();
    expect(approval.status).toBe("approved");
    await first.presence();
    await expect(pending).toHaveCount(0);
    await expect(page.locator(`[data-id="device:${approval.deviceId}"]`)).toContainText("自動更新 WinA");
    await expect(page.locator("#network-clients-summary")).toHaveText("承認待ち 0 台 · 登録済み 1 台 · オンライン 1 台");
    await expect(page.locator("#shared-admin-content")).toContainText("操作に使うPC: 自動更新 WinA");
    await expect(page.locator("#shared-admin-content")).not.toContainText(approval.deviceId);
    await expect(page.locator("#onboarding-steps")).toContainText("操作PC 1 台・実行PC 1 台");
    const second = await createDeviceParticipant(await hub.observeNetwork(), "自動更新 WinB");
    await expect(page.locator(`[data-id="request:${second.requestId}"]`)).toContainText("自動更新 WinB");
    await expect(page.locator("#network-clients-summary")).toHaveText("承認待ち 1 台 · 登録済み 1 台 · オンライン 1 台");
    await expect(page.locator("#onboarding-steps")).toContainText("承認待ち 1 台");
    await capture(page, testInfo, "automatic-approval-and-next-request");
    } finally {
        if (keyboard) await testInfo.attach("native-project-selection-events", {
            contentType: "application/json", body: Buffer.from(JSON.stringify(keyEvents, null, 2)),
        });
    }
});
}

for (const region of ["project list", "onboarding steps", "next guidance"]) {
    const title = region === "next guidance" ? "Automatic refresh keeps the project creation action distinct when next guidance changes"
        : `Automatic refresh updates focused ${region} facts without changing an action target`;
    test(title, async ({ page, hub }, testInfo) => {
        await startNetwork(page, hub);
        const initial = "更新前の仕事", updated = "更新後の仕事";
        const projectId = region === "next guidance" ? `project-${randomUUID()}` : await createProject(page, initial);
        const target = region === "project list"
            ? page.locator(`#shared-admin-content [data-sa-operation="save_project"][data-sa-id="${projectId}"]`)
            : page.locator(region === "onboarding steps" ? "#onboarding-steps [data-onboarding-edit]" : "#onboarding-new-project");
        await target.focus();
        await expect(target).toBeFocused();
        const focused = await target.elementHandle();
        await externalSharedCommand(page, hub, { kind: "save_project", id: projectId, label: updated,
            archived: false, controller_device_ids: [], runner_device_ids: [] });
        const regionNode = page.locator(region === "project list" ? "#shared-admin-content"
            : region === "onboarding steps" ? "#onboarding-steps" : "#onboarding-next");
        await expect(regionNode).toContainText(region === "next guidance" ? "仕事を依頼するPCを指定してください" : updated);
        expect(await focused.evaluate(node => node.isConnected && node === document.activeElement)).toBe(true);
        await capture(page, testInfo, `automatic-focused-${region.replaceAll(" ", "-")}`);
        if (region === "next guidance") {
            await target.click();
            await expect(page.locator("#shared-admin-label")).toHaveValue("");
        }
    });
}

test("Automatic refresh retires a removed focused project action without reusing its target", async ({ page, hub }) => {
    await startNetwork(page, hub);
    const removedId = await createProject(page, "離脱する仕事");
    const retainedId = await createProject(page, "残す仕事");
    const removed = page.locator(`#shared-admin-content [data-sa-operation="save_project"][data-sa-id="${removedId}"]`);
    await removed.focus();
    const original = await removed.elementHandle();
    await externalSharedCommand(page, hub, { kind: "delete_project", id: removedId });
    await expect(removed).toHaveCount(0);
    expect(await original.evaluate(node => node.isConnected)).toBe(false);
    await expect(page.locator("#shared-admin-content")).toBeFocused();
    await expect(page.locator("#shared-admin-form")).toHaveCount(0);
    const retained = page.locator(`#shared-admin-content [data-sa-operation="save_project"][data-sa-id="${retainedId}"]`);
    await retained.click();
    await expect(page.locator("#shared-admin-label")).toHaveValue("残す仕事");
});

for (const region of ["onboarding-next", "onboarding-steps"]) {
    test(`Automatic refresh retires ${region} action when the selected project disappears`, async ({ page, hub }) => {
        await startNetwork(page, hub);
        const removedId = await createProject(page, "元の案内対象");
        const retainedId = await createProject(page, "次の案内対象");
        await page.locator("#onboarding-project").selectOption(removedId);
        const action = page.locator(`#${region} [data-onboarding-edit]`);
        await action.focus();
        const original = await action.elementHandle();
        await externalSharedCommand(page, hub, { kind: "delete_project", id: removedId });
        await expect(page.locator("#onboarding-project")).toHaveValue(retainedId);
        expect(await original.evaluate(node => node.isConnected)).toBe(false);
        await expect(page.locator(`#${region}`)).toBeFocused();
        await expect(page.locator("#shared-admin-form")).toHaveCount(0);
        await action.click();
        await expect(page.locator("#shared-admin-label")).toHaveValue("次の案内対象");
    });
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
    await expect(firstRow).toContainText("このPCはHubに接続しています。依頼や実行に使うプロジェクトを設定してください。");
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
    await expect(page.locator("#history-rows")).toContainText("依頼元に保存された最終確認");
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
