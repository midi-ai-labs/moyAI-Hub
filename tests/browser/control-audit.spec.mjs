import { test, expect } from "./fixture.mjs";
import { freePort } from "./hub_server.mjs";
import { createMetadataProvider } from "./provider-fixture.mjs";
import { createDeviceParticipant } from "./device-fixture.mjs";

async function ready(page, hub) {
    const errors = [];
    page.on("pageerror", error => errors.push(error.message));
    await page.goto(hub.url);
    await expect(page.locator("#management-status")).toHaveText("Hub本体に接続中");
    return errors;
}
async function capture(page, testInfo, name) {
    const path = testInfo.outputPath(`${name}.png`);
    await page.screenshot({ path });
    await testInfo.attach(name, { path, contentType: "image/png" });
    // Read actual controls, including conditional/disabled ones. This attachment is
    // an observation inventory, not a substitute for the assertions or visual review.
    const controls = await page.locator("button,input,select,summary,a,pre[tabindex]").evaluateAll(nodes => nodes.map(node => ({
        tag: node.tagName, id: node.id, text: node.getAttribute("aria-label") || node.textContent.trim(),
        type: node.getAttribute("type"), href: node.getAttribute("href"), disabled: Boolean(node.disabled),
        readonly: Boolean(node.readOnly), checked: node.type === "checkbox" ? node.checked : null,
        value: node.id === "token" || node.id === "network-invite-code" ? "[credential omitted]" : node.value ?? null,
        visible: node.checkVisibility(), options: node.tagName === "SELECT"
            ? [...node.options].map(option => ({ value: option.value, label: option.text, selected: option.selected, disabled: option.disabled })) : null,
    })));
    await testInfo.attach(`${name}-controls`, { body: Buffer.from(JSON.stringify(controls, null, 2)), contentType: "application/json" });
}
async function startNetwork(page, hub) {
    await page.locator("#network-ip").fill("127.0.0.1");
    await page.locator("#network-port").fill(String(hub.networkPort));
    await page.locator("#network-start").click();
    await expect(page.locator("#network-stop")).toBeVisible();
    return hub.observeNetwork();
}
async function group(page, label) {
    await page.locator("#network-group-id").selectOption("");
    await page.locator("#network-group-label").fill(label);
    await page.locator("#network-group-form button").click();
    await expect(page.locator("#network-group-list")).toContainText(label);
}

test("Every page link and explanatory disclosure opens its own current target", async ({ page, hub }, testInfo) => {
    const errors = await ready(page, hub);
    for (const hash of ["#device-network", "#web-management", "#overview", "#models", "#clients", "#mcp-history", "#revisions"]) {
        await page.locator(`nav a[href="${hash}"]`).click();
        await expect(page).toHaveURL(new RegExp(`${hash}$`));
        await expect(page.locator(hash)).toBeInViewport();
    }
    for (const [selector, hash] of [["a.brand", "#overview"], ["#open-device-network", "#device-network"], ["#device-network a.primary-link", "#clients"]]) {
        await page.locator(selector).click();
        await expect(page).toHaveURL(new RegExp(`${hash}$`));
        await expect(page.locator(hash)).toBeInViewport();
    }
    for (const summary of [
        "中継経路の詳しい説明", "従来の手動接続（Hubと同じPC内）", "モデルの確認と同時接続数の詳しい説明",
        "モデル接続の生存確認（詳細）",
    ]) {
        const control = page.getByText(summary, { exact: true });
        const details = control.locator("..");
        await control.click(); await expect(details).toHaveAttribute("open", "");
        await control.click(); await expect(details).not.toHaveAttribute("open", "");
    }
    for (const selector of [".registration-options", 'details[data-details-key="network-connection-details"]', 'details[data-details-key="network-deleted-devices"]']) {
        const details = page.locator(selector);
        await details.locator("summary").click(); await expect(details).toHaveAttribute("open", "");
        await details.locator("summary").click(); await expect(details).not.toHaveAttribute("open", "");
    }
    await page.locator('nav a[href="#web-management"]').click();
    await expect(page.locator("#web-url")).toHaveJSProperty("readOnly", true);
    await expect(page.locator("#web-remote-address")).not.toBeVisible();
    await capture(page, testInfo, "all-navigation-web-status");
    expect(errors).toEqual([]);
});

test("Manual server validates inputs and explicitly starts with Gateway on and off", async ({ page, hub }, testInfo) => {
    const errors = await ready(page, hub);
    await page.locator("#manual-server-connection > summary").click();
    await page.locator("#bind").fill("");
    await page.locator("#start-server").click();
    expect(await page.locator("#bind").evaluate(node => node.validity.valueMissing)).toBe(true);
    await page.locator("#bind").fill(`127.0.0.1:${hub.networkPort}`);
    await page.locator("#start-server").click();
    expect(await page.locator("#token").evaluate(node => node.validity.valueMissing)).toBe(true);
    await page.locator("#token").fill("short");
    await page.locator("#start-server").click();
    expect((await hub.command("hub_snapshot")).server.running).toBe(false);
    await page.locator("#token").fill("!".repeat(32));
    await page.locator("#start-server").click();
    await expect(page.locator("#notice")).toContainText("トークンは英数字");
    for (const [bind, message] of [["not-an-address", "IP:port"], ["0.0.0.0:9470", "loopback"], ["127.0.0.1:0", "ポートとトークン"]]) {
        await page.locator("#bind").fill(bind);
        await page.locator("#token").fill("x".repeat(32));
        await page.locator("#start-server").click();
        await expect(page.locator("#notice")).toContainText(message);
        expect((await hub.command("hub_snapshot")).server.running).toBe(false);
    }
    await capture(page, testInfo, "manual-invalid-input");
    await page.locator("#bind").fill(`127.0.0.1:${hub.networkPort}`);
    for (const enabled of [true, false]) {
        await page.locator("#enable-gateway").setChecked(enabled);
        if (enabled) {
            await page.locator("#generate-token").click();
            const first = await page.locator("#token").inputValue();
            await page.locator("#generate-token").click();
            expect(await page.locator("#token").inputValue()).not.toBe(first);
            await expect(page.locator("#token")).toHaveAttribute("type", "text");
        } else await page.locator("#token").fill("a".repeat(256));
        await page.locator("#start-server").click();
        await expect(page.locator("#stop-server")).toBeVisible();
        const snapshot = await hub.command("hub_snapshot");
        expect(snapshot.server.running).toBe(true);
        expect(snapshot.gateway.enabled).toBe(enabled);
        await expect(page.locator("#token")).toHaveValue("");
        await expect(page.locator("#token")).toHaveAttribute("type", "password");
        for (const id of ["bind", "token", "generate-token", "enable-gateway"]) await expect(page.locator(`#${id}`)).toBeDisabled();
        await capture(page, testInfo, `manual-gateway-${enabled ? "on" : "off"}`);
        await page.locator("#stop-server").click();
        await expect(page.locator("#start-server")).toBeVisible();
        expect((await hub.command("hub_snapshot")).gateway.enabled).toBe(false);
    }
    await page.reload();
    expect((await hub.command("hub_snapshot")).server.running).toBe(false);
    expect(errors).toEqual([]);
});

test("Model dropdown alternatives reset dependent values and register each provider profile", async ({ page, hub }, testInfo) => {
    const errors = await ready(page, hub), provider = await createMetadataProvider();
    try {
        await page.locator(".registration-options > summary").click();
        for (const profile of ["openai_compatible_chat", "lmstudio_chat", "openai_compatible_responses", "lmstudio_responses"]) {
            await page.locator("#profile").selectOption(profile);
            await page.locator("#endpoint").fill(provider.url.replace("http://", ""));
            await page.locator("#discover").click();
            await expect(page.locator("#model")).toBeEnabled();
            const model = profile.startsWith("lmstudio") ? "fixture-native" : "fixture-alpha";
            await page.locator("#model").selectOption(model);
            await page.locator("#allow-tools").check();
            await page.locator("#model").selectOption("");
            await expect(page.locator("#allow-tools")).not.toBeChecked();
            await expect(page.locator("#register")).toBeDisabled();
            await page.locator("#model").selectOption(model);
            const previous = (await hub.command("hub_snapshot")).deployments;
            if (previous.length) {
                await page.locator("#logical-model").selectOption(previous[0].deployment.logical_model_id);
                await page.locator("#capacity-pool").selectOption(previous[0].deployment.capacity_pool_id);
                await expect(page.locator("#capacity")).toBeDisabled();
                await page.locator("#logical-model").selectOption("");
                await page.locator("#capacity-pool").selectOption("");
                await expect(page.locator("#capacity")).toBeEnabled();
            }
            await page.locator("#label").fill("");
            await page.locator("#register").click();
            expect(await page.locator("#label").evaluate(node => node.validity.valueMissing)).toBe(true);
            await page.locator("#label").fill(`日本語 ${profile}`);
            await page.locator("#capacity").fill("1025");
            await page.locator("#register").click();
            expect(await page.locator("#capacity").evaluate(node => node.validity.rangeOverflow)).toBe(true);
            expect((await hub.command("hub_snapshot")).deployments).toHaveLength(previous.length);
            await page.locator("#capacity").fill("1024");
            await page.locator("#allow-tools").check();
            await page.locator("#allow-tools").uncheck();
            await page.locator("#register").click();
            await expect(page.locator("#model-rows tr")).toHaveCount(previous.length + 1);
            const state = await hub.command("hub_snapshot");
            const row = state.deployments.find(row => row.deployment.label === `日本語 ${profile}`);
            expect(row.deployment.provider_profile).toBe(profile);
            expect(state.store.catalog.models).toHaveLength(previous.length + 1);
            expect(state.store.catalog.capacity_pools).toHaveLength(previous.length + 1);
        }
        await capture(page, testInfo, "provider-dropdowns-registered");
        await page.reload(); await expect(page.locator("#model-rows tr")).toHaveCount(4);
        expect(errors).toEqual([]);
    } finally { await provider.close(); }
});

test("Device dialogs cancel drafts, review without losing edits, and update both membership values", async ({ page, hub }, testInfo) => {
    const errors = await ready(page, hub);
    const network = await startNetwork(page, hub);
    await group(page, "Clients"); await group(page, "Servers");
    const participant = await createDeviceParticipant(network, "Pending control fixture");
    await page.locator("#network-clients-refresh").click();
    await page.locator("#network-filter").selectOption("pending");
    const pending = page.locator(`[data-id="request:${participant.requestId}"]`);
    await expect(pending).toBeVisible();
    await pending.locator("summary").click(); await expect(pending.locator("details")).toHaveAttribute("open", "");
    await pending.locator("summary").click();
    await pending.locator("[data-network-request]").click();
    await page.locator("#network-device-label").fill("Cancelled request draft");
    await page.locator("#network-device-close").click();
    await pending.locator("[data-network-request]").click();
    await expect(page.locator("#network-device-label")).toHaveValue("Pending control fixture");
    for (const label of ["Clients", "Servers"]) await page.locator("#network-device-groups").getByLabel(label, { exact: true }).check();
    await page.locator("#network-device-save").click();
    await expect(page.locator("#network-device-dialog")).not.toBeVisible();
    const approval = await participant.collectApproval(); expect(approval.status).toBe("approved");
    await participant.presence(); await page.locator("#network-clients-refresh").click();
    await page.locator("#network-filter").selectOption("all");
    const row = page.locator(`[data-id="device:${approval.deviceId}"]`);
    await expect(row).toContainText("Clients"); await expect(row).toContainText("Servers");
    for (const query of ["Pending control", "127.0.0.1", approval.deviceId, "Servers"]) {
        await page.locator("#network-search").fill(query); await expect(row).toBeVisible();
    }
    await page.locator("#network-search").fill("not-a-device"); await expect(row).toHaveCount(0);
    await page.locator("#network-search").fill(""); await expect(row).toBeVisible();
    await row.locator("[data-network-edit]").click();
    await page.locator("#network-device-label").fill("");
    await page.locator("#network-device-save").click();
    expect(await page.locator("#network-device-label").evaluate(node => node.validity.valueMissing)).toBe(true);
    await page.locator("#network-device-label").fill("取消する名前");
    await page.locator("#network-device-groups").getByLabel("Servers", { exact: true }).uncheck();
    await page.keyboard.press("Escape");
    await expect(page.locator("#network-device-dialog")).not.toBeVisible();
    await row.locator("[data-network-edit]").click();
    await expect(page.locator("#network-device-label")).toHaveValue("Pending control fixture");
    await expect(page.locator("#network-device-groups").getByLabel("Servers", { exact: true })).toBeChecked();
    await page.locator("#network-device-label").fill("変更を保持する端末");
    await page.locator("#network-device-groups").getByLabel("Clients", { exact: true }).uncheck();
    await page.locator("#network-device-revoke-details > summary").click();
    await page.locator("#network-device-revoked").check(); await page.locator("#network-device-revoked").uncheck();
    await page.locator("#network-device-review").click();
    await expect(page.locator("#network-device-error")).toContainText("入力内容を確認して保存");
    await expect(page.locator("#network-device-label")).toHaveValue("変更を保持する端末");
    await expect(page.locator("#network-device-groups").getByLabel("Clients", { exact: true })).not.toBeChecked();
    await capture(page, testInfo, "device-draft-review-memberships");
    await page.locator("#network-device-save").click();
    await expect(page.locator("#network-device-dialog")).not.toBeVisible();
    const state = await hub.command("hub_network_snapshot"), device = state.devices.find(device => device.device_id === approval.deviceId);
    expect(device.label).toBe("変更を保持する端末"); expect(device.revoked).toBe(false);
    expect(device.groups).toEqual([state.groups.find(group => group.label === "Servers").id]);
    await row.locator("[data-network-delete-device]").click();
    await page.mouse.click(4, 4); await expect(page.locator("#network-delete-dialog")).toBeVisible();
    await page.keyboard.press("Escape"); await expect(page.locator("#network-delete-dialog")).not.toBeVisible();
    await expect(row).toBeVisible();
    await page.locator('nav a[href="#mcp-history"]').click();
    await page.locator("#history-device").selectOption(approval.deviceId); await expect(page.locator("#history-request")).toBeEnabled();
    await page.locator("#history-device").selectOption(""); await expect(page.locator("#history-request")).toBeDisabled();
    await page.reload(); await expect(row).toContainText("変更を保持する端末");
    expect(errors).toEqual([]);
});

test("Legacy invitation controls and rule depth three preserve exact selected groups", async ({ page, context, hub }, testInfo) => {
    const errors = await ready(page, hub);
    await context.grantPermissions(["clipboard-read", "clipboard-write"], { origin: new URL(hub.url).origin });
    await startNetwork(page, hub);
    await page.locator("#network-group-form button").click();
    expect(await page.locator("#network-group-label").evaluate(node => node.validity.valueMissing)).toBe(true);
    await group(page, "Client 日本語"); await group(page, "Server 日本語");
    const state = await hub.command("hub_network_snapshot"), [clients, servers] = ["Client 日本語", "Server 日本語"].map(label => state.groups.find(group => group.label === label));
    await page.locator("#network-group-id").selectOption(clients.id);
    await expect(page.locator("#network-group-label")).toHaveValue(clients.label);
    await page.locator("#network-group-id").selectOption(""); await expect(page.locator("#network-group-label")).toHaveValue("");
    await page.locator("#network-rule-form button").click();
    expect(await page.locator("#network-rule-from").evaluate(node => node.validity.valueMissing)).toBe(true);
    await page.locator("#network-rule-from").selectOption(`group:${clients.id}`);
    await page.locator("#network-rule-form button").click();
    expect(await page.locator("#network-rule-to").evaluate(node => node.validity.valueMissing)).toBe(true);
    await page.locator("#network-rule-to").selectOption(`group:${servers.id}`);
    await page.locator("#network-rule-usage").selectOption("delegated");
    await page.locator("#network-rule-effect").selectOption("deny");
    await page.locator("#network-rule-depth").selectOption("3");
    await page.locator("#network-rule-form button").click();
    await expect(page.locator("#network-rule-list")).toContainText("最大3段");
    const saved = (await hub.command("hub_network_snapshot")).rules[0];
    expect(saved).toMatchObject({ from: { kind: "group", id: clients.id }, to: { kind: "group", id: servers.id }, usage: "delegated", effect: "deny", max_depth: 3, profile_id: null });
    const details = page.locator('details[data-details-key="network-legacy-invite"]');
    await details.locator("summary").click();
    await page.locator("#network-invite-form button").click();
    expect(await page.locator("#network-invite-label").evaluate(node => node.validity.valueMissing)).toBe(true);
    await page.locator("#network-invite-label").fill("Legacy invited fixture");
    for (const label of [clients.label, servers.label]) await page.locator("#network-invite-groups").getByLabel(label, { exact: true }).check();
    await page.locator("#network-invite-groups").getByLabel(clients.label, { exact: true }).uncheck();
    const request = page.waitForRequest(request => request.url().endsWith("/admin/command") && request.postDataJSON()?.command === "hub_network_invite");
    await page.locator("#network-invite-form button").click();
    const payload = (await request).postDataJSON();
    expect(payload.args.groups).toEqual([servers.id]); expect(payload.args.label).toBe("Legacy invited fixture");
    await expect(page.locator("#network-invite-receipt")).toBeVisible();
    await expect(page.locator("#network-invite-expiry")).toContainText("1回限り");
    const code = await page.locator("#network-invite-code").inputValue(); expect(code.length).toBeGreaterThan(20);
    await expect(page.locator("#network-invite-code")).toHaveJSProperty("readOnly", true);
    await page.locator("#network-copy-invite").click();
    await expect(page.locator("#network-clients-notice")).toContainText("参加コードをコピーしました");
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(code);
    await details.locator("summary").click(); await details.locator("summary").click();
    await expect(page.locator("#network-invite-code")).toHaveValue(code);
    // Hide only by the user-facing disclosure before capture; invitation is a credential.
    await details.locator("summary").click();
    await capture(page, testInfo, "legacy-invite-and-depth-three");
    await page.reload(); await expect(page.locator("#network-rule-list")).toContainText("最大3段");
    expect(errors).toEqual([]);
});

test.describe("Configured external Web controls", () => {
    test.use({ remoteWeb: true });
    test("External URL is readonly and copies the server-owned URL from either actual listener", async ({ page, context, hub }, testInfo) => {
        const errors = await ready(page, hub);
        expect(hub.remoteUrl).toMatch(/^http:\/\/127\.0\.0\.1:\d+\/admin\/$/);
        expect(hub.remoteUrl).not.toBe(hub.url);
        for (const url of [hub.url, hub.remoteUrl]) {
            await context.grantPermissions(["clipboard-read", "clipboard-write"], { origin: new URL(url).origin });
            await page.goto(url);
            await expect(page.locator("#management-status")).toHaveText("Hub本体に接続中");
            await page.locator('nav a[href="#web-management"]').click();
            await expect(page.locator("#web-remote-url")).toHaveValue(hub.remoteUrl);
            await expect(page.locator("#web-remote-url")).toHaveJSProperty("readOnly", true);
            await expect(page.locator("#web-url")).toHaveValue(hub.url);
            await page.locator("#web-copy-remote-url").click();
            await expect(page.locator("#web-status")).toHaveText("URLをコピーしました。");
            expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(hub.remoteUrl);
            await page.locator("#web-copy-url").click();
            expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(hub.url);
        }
        await capture(page, testInfo, "external-web-addresses");
        expect(errors).toEqual([]);
    });
});

test("Network bind validation and explicit automatic Gateway setting preserve stopped state", async ({ page, hub }, testInfo) => {
    const errors = await ready(page, hub);
    await page.locator("#network-ip").fill("");
    await page.locator("#network-start").click();
    expect(await page.locator("#network-ip").evaluate(node => node.validity.valueMissing)).toBe(true);
    for (const [ip, port] of [["not-an-ip", "9471"], ["0.0.0.0", "9471"], ["127.0.0.1", "0"], ["127.0.0.1", "65536"], ["127.0.0.1", "abc"]]) {
        await page.locator("#network-ip").fill(ip); await page.locator("#network-port").fill(port);
        await page.locator("#network-start").click();
        await expect(page.locator("#network-notice")).toHaveClass(/error/);
        await expect(page.locator("#network-start")).toBeEnabled();
        expect((await hub.observeNetwork()).server.running).toBe(false);
        await expect(page.locator("#network-ip")).toHaveValue(ip); await expect(page.locator("#network-port")).toHaveValue(port);
    }
    await page.locator('details[data-details-key="network-connection-details"] > summary').click();
    for (const input of ["-1", "1.5", "abc"]) {
        await page.locator("#network-gateway-port").fill(input);
        await page.locator("#network-settings-form button").click();
        await expect(page.locator("#network-notice")).toContainText("1〜65535");
        expect((await hub.command("hub_network_snapshot")).gateway_port).toBe(0);
    }
    await page.locator("#network-gateway-port").fill("");
    await page.locator("#network-settings-form button").click();
    await expect(page.locator("#network-notice")).toContainText("ポート設定を保存しました");
    await startNetwork(page, hub);
    await expect(page.locator("#network-copy-config")).toBeEnabled();
    await capture(page, testInfo, "network-invalid-recovered");
    await page.locator("#network-stop").click();
    await expect(page.locator("#network-start")).toBeVisible();
    let gatewayPort = await freePort();
    while (gatewayPort === hub.networkPort) gatewayPort = await freePort();
    await page.locator("#network-gateway-port").fill(String(gatewayPort));
    await page.locator("#network-settings-form button").click();
    await expect(page.locator("#network-notice")).toContainText("ポート設定を保存しました");
    expect((await hub.observeNetwork()).gateway_port).toBe(gatewayPort);
    for (const pass of ["saved", "reloaded"]) {
        if (pass === "reloaded") await page.reload();
        await expect(page.locator("#network-gateway-port")).toHaveValue(String(gatewayPort));
        await startNetwork(page, hub);
        const fixed = await hub.observeNetwork();
        const endpoint = new URL(fixed.server.gateway_bind);
        expect(endpoint.hostname).toBe("127.0.0.1");
        expect(Number(endpoint.port)).toBe(gatewayPort);
        const details = page.locator('details[data-details-key="network-connection-details"] > summary');
        if (!(await details.evaluate(node => node.parentElement.open))) await details.click();
        await capture(page, testInfo, `network-fixed-gateway-${pass}`);
        await page.locator("#network-stop").click();
        await expect(page.locator("#network-start")).toBeVisible();
    }
    expect(errors).toEqual([]);
});

test("Rule destination dropdown replaces publication options without retaining the old target", async ({ page, hub }, testInfo) => {
    const errors = await ready(page, hub), network = await startNetwork(page, hub);
    await group(page, "Destination group");
    const groupId = (await hub.command("hub_network_snapshot")).groups[0].id;
    const ids = [];
    for (const [label, profileId] of [["Publication one", "profile-one"], ["Publication two", "profile-two"]]) {
        const participant = await createDeviceParticipant(network, label);
        await page.locator("#network-clients-refresh").click();
        await page.locator(`[data-id="request:${participant.requestId}"] [data-network-request]`).click();
        await page.locator("#network-device-groups").getByLabel("Destination group", { exact: true }).check();
        await page.locator("#network-device-save").click();
        await expect(page.locator("#network-device-dialog")).not.toBeVisible();
        const approval = await participant.collectApproval(); expect(approval.status).toBe("approved");
        ids.push(approval.deviceId); await participant.presence();
        await participant.publish({ profile_id: profileId, name: `${label} project`, endpoint: "https://127.0.0.1:7332/mcp", mode: "agent", scope_id: profileId, enabled: true });
    }
    await page.locator("#network-refresh").click();
    await expect(page.locator("#network-rule-to option")).toHaveCount(4);
    await page.locator("#network-rule-from").selectOption(`device:${ids[0]}`);
    await page.locator("#network-rule-to").selectOption(`device:${ids[0]}`);
    await page.locator("#network-rule-profile").selectOption("profile-one");
    await page.locator("#network-rule-to").selectOption(`device:${ids[1]}`);
    await expect(page.locator("#network-rule-profile")).toHaveValue("");
    await expect(page.locator("#network-rule-profile")).toHaveJSProperty("selectedIndex", 0);
    await expect(page.locator("#network-rule-profile option:checked")).toHaveText("この宛先の公開対象すべて");
    await expect(page.locator('#network-rule-profile option[value="profile-one"]')).toHaveCount(0);
    await page.locator("#network-rule-profile").selectOption("profile-two");
    await page.locator("#network-rule-to").selectOption(`group:${groupId}`);
    await expect(page.locator("#network-rule-profile option")).toHaveCount(3);
    await expect(page.locator("#network-rule-profile")).toHaveValue("profile-two");
    await page.locator("#network-rule-form button").click();
    await expect(page.locator("#network-rule-list")).toContainText("Publication two project");
    expect((await hub.command("hub_network_snapshot")).rules[0]).toMatchObject({ to: { kind: "group", id: groupId }, profile_id: "profile-two" });
    await page.locator("#network-rule-to").selectOption("");
    await expect(page.locator("#network-rule-profile option")).toHaveCount(1);
    await expect(page.locator("#network-rule-profile")).toHaveValue("");
    await expect(page.locator("#network-rule-profile")).toHaveJSProperty("selectedIndex", 0);
    await expect(page.locator("#network-rule-profile option:checked")).toHaveText("この宛先の公開対象すべて");
    await capture(page, testInfo, "rule-destination-options");
    expect(errors).toEqual([]);
});

test("Fixed control reverse directions retain disclosure drafts and explicitly restore rule choices", async ({ page, hub }, testInfo) => {
    const errors = await ready(page, hub), network = await startNetwork(page, hub);
    await group(page, "Reverse control sender");
    const sender = (await hub.command("hub_network_snapshot")).groups.find(item => item.label === "Reverse control sender");
    const participant = await createDeviceParticipant(network, "Reverse control receiver");
    await page.locator('nav a[href="#clients"]').click();
    await page.locator("#network-clients-refresh").click();
    const requestRow = page.locator(`[data-id="request:${participant.requestId}"]`);
    await requestRow.locator("button[data-network-action]").click();
    await expect(requestRow).toHaveCount(0);
    const approval = await participant.collectApproval(); expect(approval.status).toBe("approved");
    await participant.presence();
    await participant.publish({ profile_id: "reverse-control-project", name: "Reverse control project",
        endpoint: "https://127.0.0.1:7332/mcp", mode: "agent", scope_id: "reverse-control-scope", enabled: true });
    await page.locator("#network-clients-refresh").click();
    const row = page.locator(`[data-id="device:${approval.deviceId}"]`), identity = row.locator("details[data-device-identity]");
    await expect(row).toContainText("受付申告あり");

    // C083: the same current device identity survives an explicit close, a normal poll and reopen.
    await identity.locator("summary").click(); await expect(identity).toHaveAttribute("open", "");
    const identityText = await identity.innerText();
    expect(identityText).toContain(approval.deviceId);
    await identity.locator("summary").click(); await expect(identity).not.toHaveAttribute("open", "");
    await page.waitForResponse(response => response.url().endsWith("/admin/command")
        && response.request().postDataJSON()?.command === "hub_snapshot");
    await expect(identity).not.toHaveAttribute("open", "");
    await identity.locator("summary").click(); await expect(identity).toHaveAttribute("open", "");
    expect(await identity.innerText()).toBe(identityText);
    await capture(page, testInfo, "device-identity-reopened");

    // C072: disclosure closure must not discard either editable draft; no revocation is saved.
    await row.locator("button[data-network-edit]").click();
    const revoked = page.locator("#network-device-revoked"), details = page.locator("#network-device-revoke-details");
    await page.locator("#network-device-label").fill("Unsaved disclosure draft");
    await details.locator("summary").click(); await revoked.check();
    await details.locator("summary").click(); await expect(details).not.toHaveAttribute("open", "");
    await page.waitForResponse(response => response.url().endsWith("/admin/command")
        && response.request().postDataJSON()?.command === "hub_snapshot");
    await expect(details).not.toHaveAttribute("open", "");
    await expect(page.locator("#network-device-label")).toHaveValue("Unsaved disclosure draft");
    await details.locator("summary").click(); await expect(details).toHaveAttribute("open", "");
    await expect(revoked).toBeChecked();
    await capture(page, testInfo, "device-revocation-reopened-draft");
    await page.locator("#network-device-close").click();
    await expect(page.locator("#network-device-dialog")).not.toBeVisible();
    const unchanged = (await hub.command("hub_network_snapshot")).devices.find(item => item.device_id === approval.deviceId);
    expect(unchanged).toMatchObject({ label: "Reverse control receiver", revoked: false });

    await page.locator('nav a[href="#device-network"]').click();
    await page.locator("#network-rule-from").selectOption(`group:${sender.id}`);
    await page.locator("#network-rule-to").selectOption(`device:${approval.deviceId}`);
    // C055: explicitly select the blank option after a real sender selection.
    await page.locator("#network-rule-from").selectOption("");
    await expect(page.locator("#network-rule-from")).toHaveJSProperty("selectedIndex", 0);
    await expect(page.locator("#network-rule-from option:checked")).toHaveText("選択してください");
    await page.locator("#network-rule-form button").click();
    expect(await page.locator("#network-rule-from").evaluate(node => node.validity.valueMissing)).toBe(true);
    expect((await hub.command("hub_network_snapshot")).rules).toHaveLength(0);
    await page.locator("#network-rule-from").selectOption(`group:${sender.id}`);
    await page.locator("#network-rule-profile").selectOption("reverse-control-project");
    await page.locator("#network-rule-effect").selectOption("deny");
    await page.locator("#network-rule-form button").click();
    await expect(page.locator("#network-rule-list .network-rule")).toHaveCount(1);
    const denied = (await hub.command("hub_network_snapshot")).rules[0];
    expect(denied).toMatchObject({ from: { kind: "group", id: sender.id }, to: { kind: "device", id: approval.deviceId },
        effect: "deny", profile_id: "reverse-control-project" });
    // C058/C059: explicit selections rather than initial defaults or destination-triggered reset.
    await page.locator("#network-rule-effect").selectOption("allow");
    await page.locator("#network-rule-profile").selectOption("");
    await expect(page.locator("#network-rule-effect")).toHaveValue("allow");
    await expect(page.locator("#network-rule-profile")).toHaveJSProperty("selectedIndex", 0);
    await expect(page.locator("#network-rule-profile option:checked")).toHaveText("この宛先の公開対象すべて");
    await page.locator("#network-rule-form button").click();
    await expect(page.locator("#network-rule-list .network-rule")).toHaveCount(2);
    const rules = (await hub.command("hub_network_snapshot")).rules;
    expect(rules.filter(item => item.id === denied.id)).toEqual([denied]);
    expect(rules.filter(item => item.id !== denied.id)).toEqual([expect.objectContaining({
        from: { kind: "group", id: sender.id }, to: { kind: "device", id: approval.deviceId }, effect: "allow", profile_id: null,
    })]);
    await capture(page, testInfo, "rule-choices-explicitly-restored");
    await testInfo.attach("fixed-reverse-direction-identity", { contentType: "application/json", body: Buffer.from(JSON.stringify({
        controls: ["C055", "C058", "C059", "C072", "C083"], deviceId: approval.deviceId, senderId: sender.id, rules,
        publication: "Participant public-protocol fixture; no actual model or remote task execution.",
    }, null, 2)) });
    expect(errors).toEqual([]);
});
