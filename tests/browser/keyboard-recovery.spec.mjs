import { readFile } from "node:fs/promises";
import { test, expect } from "./fixture.mjs";
import { createMetadataProvider } from "./provider-fixture.mjs";

async function ready(page, hub) {
    const errors = [];
    page.on("pageerror", error => errors.push(error.message));
    await page.goto(hub.url);
    await expect(page.locator("#management-status")).toHaveText("Hub本体に接続中");
    return errors;
}

// Use actual keyboard traversal; never focus an element with evaluate/Locator.focus.
async function tabTo(page, target, { dialog = null, backwards = false, limit = 40 } = {}) {
    await expect(target).toHaveCount(1);
    let browserBoundary = false;
    for (let step = 0; step <= limit; ++step) {
        if (dialog) {
            const focus = await modalFocus(dialog);
            expect(focus.inside || (!browserBoundary && focus.browserBoundary), "Tab must not focus background page controls or remain outside the document").toBe(true);
            browserBoundary = focus.browserBoundary;
        }
        if (await target.evaluate(node => node === document.activeElement)) return;
        if (step < limit) await page.keyboard.press(backwards ? "Shift+Tab" : "Tab");
    }
    throw new Error(`Keyboard did not reach ${await target.getAttribute("id") ?? await target.getAttribute("href")}`);
}

async function modalFocus(dialog) {
    return dialog.evaluate(node => ({
        inside: node.open && node.contains(document.activeElement),
        // Native Edge dialogs also visit browser chrome once at the sequence boundary.
        browserBoundary: node.open && !document.hasFocus() && document.activeElement === document.body,
        pageFocus: document.hasFocus(), tag: document.activeElement?.tagName, id: document.activeElement?.id,
    }));
}

async function replaceFocusedText(page, text) {
    await page.keyboard.press("ControlOrMeta+A");
    await page.keyboard.press("Backspace");
    if (text) await page.keyboard.type(text);
}

async function screenshot(page, testInfo, name) {
    const file = testInfo.outputPath(`${name}.png`);
    await page.screenshot({ path: file });
    await testInfo.attach(name, { path: file, contentType: "image/png" });
}

test("Keyboard model dialog keeps focus, validates edits and cancels with Escape", async ({ page, hub }, testInfo) => {
    const provider = await createMetadataProvider();
    const editRequests = [];
    page.on("request", request => {
        if (request.url().endsWith("/admin/command") && request.method() === "POST") {
            const body = request.postDataJSON();
            if (body.command === "hub_edit") editRequests.push(body.args);
        }
    });
    try {
        const errors = await ready(page, hub);
        const modelLink = page.locator('nav a[href="#models"]');
        await tabTo(page, modelLink);
        await page.keyboard.press("Enter");
        await expect(page).toHaveURL(`${hub.url}#models`);
        // Model registration is setup through the existing GUI, not private catalog mutation.
        await page.locator("#endpoint").fill(provider.url);
        await page.locator("#discover").click();
        await expect(page.locator("#model option")).toHaveCount(3);
        await page.locator("#model").selectOption("fixture-alpha");
        await page.locator("#label").fill("Keyboard model");
        await page.locator("#register").click();
        const editButton = page.getByRole("button", { name: "Keyboard model を編集", exact: true });
        await expect(editButton).toBeVisible();
        const before = await hub.command("hub_snapshot");
        const deployment = before.deployments[0].deployment;
        await tabTo(page, editButton);
        await page.keyboard.press("Enter");
        const dialog = page.locator("#edit-dialog");
        await expect(dialog).toBeVisible();
        const visited = new Set();
        const traversal = [];
        let browserBoundary = false;
        for (const key of ["Tab", "Shift+Tab"]) {
            for (let step = 0; step < 12; ++step) {
                const focus = await modalFocus(dialog);
                traversal.push({ key, step, ...focus });
                expect(focus.inside || (!browserBoundary && focus.browserBoundary), "Modal traversal must not visit background page controls").toBe(true);
                browserBoundary = focus.browserBoundary;
                if (focus.inside) visited.add(focus.id);
                await page.keyboard.press(key);
            }
        }
        await testInfo.attach("keyboard-modal-traversal", { contentType: "application/json", body: Buffer.from(JSON.stringify(traversal, null, 2)) });
        for (const id of ["close-edit", "edit-label", "edit-capacity", "edit-enabled", "remove-model", "save-edit"]) expect(visited.has(id), `${id} is keyboard reachable`).toBe(true);
        await tabTo(page, page.locator("#edit-label"), { dialog });
        await replaceFocusedText(page, "Unsaved keyboard draft");
        await screenshot(page, testInfo, "keyboard-dialog-draft");
        await page.keyboard.press("Escape");
        await expect(dialog).not.toBeVisible();
        await expect(editButton).toBeFocused();
        expect((await hub.command("hub_snapshot")).deployments[0].deployment.label).toBe("Keyboard model");
        expect(editRequests).toHaveLength(0);
        await page.keyboard.press("Enter");
        await expect(dialog).toBeVisible();
        await expect(page.locator("#edit-label")).toHaveValue("Keyboard model");
        await tabTo(page, page.locator("#edit-label"), { dialog });
        await replaceFocusedText(page, "");
        await page.keyboard.press("Enter");
        await expect(page.locator("#edit-label")).toBeFocused();
        expect(await page.locator("#edit-label").evaluate(node => node.validity.valueMissing)).toBe(true);
        await expect(dialog).toBeVisible();
        expect(editRequests).toHaveLength(0);
        await page.keyboard.type("Keyboard model saved");
        await tabTo(page, page.locator("#edit-capacity"), { dialog });
        await replaceFocusedText(page, "0");
        await page.keyboard.press("Enter");
        await expect(page.locator("#edit-capacity")).toBeFocused();
        expect(await page.locator("#edit-capacity").evaluate(node => node.validity.rangeUnderflow)).toBe(true);
        await expect(dialog).toBeVisible();
        expect(editRequests).toHaveLength(0);
        await screenshot(page, testInfo, "keyboard-dialog-validation");
        await replaceFocusedText(page, "3");
        await tabTo(page, page.locator("#edit-enabled"), { dialog });
        await page.keyboard.press("Space");
        await expect(page.locator("#edit-enabled")).not.toBeChecked();
        await tabTo(page, page.locator("#save-edit"), { dialog });
        await page.keyboard.press("Enter");
        await expect(dialog).not.toBeVisible();
        await expect(page.locator("#model-rows")).toContainText("Keyboard model saved");
        await expect(page.locator("#model-rows")).toContainText("メンテナンス");
        expect(editRequests).toHaveLength(1);
        expect(editRequests[0].request).toMatchObject({ deployment_id: deployment.id, revision: before.store.revision,
            label: "Keyboard model saved", capacity: 3, enabled: false, remove: false });
        const saved = await hub.command("hub_snapshot");
        expect(saved.deployments[0].deployment).toMatchObject({ id: deployment.id, label: "Keyboard model saved", enabled: false });
        expect(saved.store.catalog.capacity_pools.find(pool => pool.id === deployment.capacity_pool_id).max_concurrent).toBe(3);
        await screenshot(page, testInfo, "keyboard-model-saved");
        expect(errors).toEqual([]);
    } finally { await provider.close(); }
});

test("Public config and Web URL copy through the real browser clipboard", async ({ page, context, hub }, testInfo) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"], { origin: new URL(hub.url).origin });
    const errors = await ready(page, hub);
    await page.locator('nav a[href="#device-network"]').click();
    await expect(page.locator("#network-copy-config")).toBeDisabled();
    await page.locator("#network-ip").fill("127.0.0.1");
    await page.locator("#network-port").fill(String(hub.networkPort));
    await page.locator("#network-start").click();
    await expect(page.locator("#network-stop")).toBeVisible();
    expect((await hub.observeNetwork()).server.running).toBe(true);
    await page.locator("#network-copy-config").click();
    await expect(page.locator("#network-notice")).toContainText("接続設定をコピーしました。");
    // Read only after this test has successfully written its own public fixture config.
    const copied = await page.evaluate(() => navigator.clipboard.readText());
    expect(copied).toContain(`https://127.0.0.1:${hub.networkPort}`);
    expect(copied).toContain("BEGIN CERTIFICATE");
    expect(copied).not.toContain("PRIVATE KEY");
    await page.evaluate(() => Object.defineProperty(navigator, "clipboard", { configurable: true, value: undefined }));
    await page.locator("#network-copy-config").click();
    await expect(page.locator("#network-config-text")).toBeVisible();
    const textareaConfig = copied.replace(/\r\n/g, "\n");
    await expect(page.locator("#network-config-text")).toHaveValue(textareaConfig);
    expect(await page.locator("#network-config-text").evaluate(node => [node.selectionStart, node.selectionEnd])).toEqual([0, textareaConfig.length]);
    await testInfo.attach("public-config-manual-copy", { body: await page.locator("#device-network").screenshot(), contentType: "image/png" });
    await page.evaluate(() => delete navigator.clipboard);
    const downloadPending = page.waitForEvent("download");
    await page.locator("#network-save-config").click();
    const download = await downloadPending;
    const file = testInfo.outputPath("copied-hub-config.toml");
    await download.saveAs(file);
    // Windows' text clipboard normalizes line endings; TOML data and escaped CA bytes must remain identical.
    expect((await readFile(file, "utf8")).replaceAll("\r\n", "\n")).toBe(copied.replaceAll("\r\n", "\n"));
    await screenshot(page, testInfo, "clipboard-public-config");
    await page.locator('nav a[href="#web-management"]').click();
    await expect(page.locator("#web-copy-url")).toBeEnabled();
    const url = await page.locator("#web-url").inputValue();
    await page.locator("#web-copy-url").click();
    await expect(page.locator("#web-status")).toHaveText("URLをコピーしました。");
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(url);
    expect(url).toBe(hub.url);
    await screenshot(page, testInfo, "clipboard-web-url");
    await page.locator("#network-stop").click();
    await expect(page.locator("#network-copy-config")).toBeDisabled();
    expect((await hub.observeNetwork()).server.running).toBe(false);
    expect(errors).toEqual([]);
});

test("An occupied network port shows an actionable error and allows GUI recovery", async ({ page, hub }, testInfo) => {
    const occupied = await createMetadataProvider();
    try {
        const errors = await ready(page, hub);
        await page.locator('nav a[href="#device-network"]').click();
        const port = new URL(occupied.url).port;
        await page.locator("#network-ip").fill("127.0.0.1");
        await page.locator("#network-port").fill(port);
        await page.locator("#network-start").click();
        await expect(page.locator("#network-notice")).toContainText("TLS の待受を開始できません。IP とポートを確認してください。");
        await expect(page.locator("#network-notice")).toHaveClass(/error/);
        await expect(page.locator("#network-start")).toBeEnabled();
        await expect(page.locator("#network-stop")).not.toBeVisible();
        await expect(page.locator("#network-port")).toHaveValue(port);
        await expect(page.locator("#network-copy-config")).toBeDisabled();
        expect((await hub.observeNetwork()).server.running).toBe(false);
        expect((await hub.command("hub_web_status")).enabled).toBe(true);
        expect((await fetch(`${occupied.url}/v1/models`)).status).toBe(200);
        await screenshot(page, testInfo, "network-occupied-port");
        await page.locator("#network-port").fill(String(hub.networkPort));
        await page.locator("#network-start").click();
        await expect(page.locator("#network-stop")).toBeVisible();
        await expect(page.locator("#network-copy-config")).toBeEnabled();
        const recovered = await hub.observeNetwork();
        expect(recovered.server.running).toBe(true);
        expect(recovered.server.bind).toBe(`https://127.0.0.1:${hub.networkPort}`);
        await screenshot(page, testInfo, "network-port-recovered");
        await page.locator("#network-stop").click();
        await expect(page.locator("#network-start")).toBeEnabled();
        expect((await hub.observeNetwork()).server.running).toBe(false);
        expect(errors).toEqual([]);
    } finally { await occupied.close(); }
});
