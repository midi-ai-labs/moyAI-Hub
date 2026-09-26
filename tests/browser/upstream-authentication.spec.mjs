import { test, expect } from "./fixture.mjs";
import { createMetadataProvider } from "./provider-fixture.mjs";

const originalKey = "fixture-upstream-key-original";
const replacementKey = "fixture-upstream-key-replacement";

async function ready(page, hub) {
    await page.goto(hub.url);
    await expect(page.locator("#management-status")).toHaveText("Hub本体に接続中");
    await page.locator('nav a[href="#models"]').click();
}

async function noDisplayedOrBrowserStoredKey(page, hub) {
    for (const key of [originalKey, replacementKey]) {
        await expect(page.locator("body")).not.toContainText(key);
        expect(JSON.stringify(await hub.command("hub_snapshot"))).not.toContain(key);
        expect(await page.evaluate(() => JSON.stringify({ local: { ...localStorage }, session: { ...sessionStorage } }))).not.toContain(key);
    }
}

async function refreshAuthenticatedModel(page, provider) {
    const before = provider.requests.length;
    const completed = page.waitForResponse(response => response.url().endsWith("/admin/command")
        && response.request().postDataJSON()?.command === "hub_refresh");
    await page.locator("#refresh-models").click();
    await completed;
    expect(provider.requests.slice(before).some(row => row.path === "/v1/models" && row.authorized)).toBe(true);
    await expect(page.locator("#model-rows tr")).toContainText("一覧あり");
}

test("Upstream API key is saved at registration and can be kept, replaced, cancelled or explicitly cleared", async ({ page, hub }) => {
    const provider = await createMetadataProvider();
    provider.setApiKey(originalKey);
    try {
        await ready(page, hub);
        await page.locator("#endpoint").fill(provider.url);
        await page.locator("#discover").click();
        await expect(page.locator("#discovery-status")).toHaveClass(/error/);
        await expect(page.locator("#register")).toBeDisabled();
        await page.locator("#api-key").fill(originalKey);
        await expect(page.locator("#api-key")).toHaveAttribute("type", "password");
        await expect(page.locator("#api-key")).toHaveAttribute("autocomplete", "off");
        await page.locator("#discover").click();
        await expect(page.locator("#model")).toBeEnabled();
        await noDisplayedOrBrowserStoredKey(page, hub);
        await page.locator("#model").selectOption("fixture-alpha");
        await page.locator("#label").fill("Authenticated model");
        await page.locator("#register").click();
        const row = page.locator("#model-rows tr");
        await expect(row).toContainText("Authenticated model");
        await expect(page.locator("#api-key")).toHaveValue("");
        await expect(page.locator("#model")).toBeDisabled();
        const savedId = (await hub.command("hub_snapshot")).deployments[0].deployment.upstream_credential_id;
        expect(savedId).toBeTruthy();

        await row.getByRole("button", { name: /を編集/ }).click();
        await expect(page.locator("#edit-api-key-status")).toContainText("設定済み");
        await expect(page.locator("#edit-api-key")).toHaveValue("");
        await page.locator("#save-edit").click();
        await expect(page.locator("#edit-dialog")).not.toBeVisible();
        expect((await hub.command("hub_snapshot")).deployments[0].deployment.upstream_credential_id).toBe(savedId);
        await refreshAuthenticatedModel(page, provider);

        await row.getByRole("button", { name: /を編集/ }).click();
        await page.locator("#edit-api-key").fill("fixture-cancelled-key");
        await page.locator("#close-edit").click();
        await expect(page.locator("#edit-api-key")).toHaveValue("");
        await row.getByRole("button", { name: /を編集/ }).click();
        await expect(page.locator("#edit-api-key")).toHaveValue("");
        provider.setApiKey(replacementKey);
        await page.locator("#edit-api-key").fill(replacementKey);
        await page.locator("#save-edit").click();
        await expect(page.locator("#edit-dialog")).not.toBeVisible();
        await expect(page.locator("#edit-api-key")).toHaveValue("");
        await refreshAuthenticatedModel(page, provider);
        await noDisplayedOrBrowserStoredKey(page, hub);

        await row.getByRole("button", { name: /を編集/ }).click();
        await page.locator("#edit-api-key-clear").check();
        await expect(page.locator("#edit-api-key")).toBeDisabled();
        await page.locator("#edit-api-key-clear").uncheck();
        await expect(page.locator("#edit-api-key")).toBeEnabled();
        await page.locator("#save-edit").click();
        await expect(page.locator("#edit-dialog")).not.toBeVisible();
        await refreshAuthenticatedModel(page, provider);

        await row.getByRole("button", { name: /を編集/ }).click();
        await page.locator("#edit-api-key-clear").check();
        provider.setApiKey(null);
        await page.locator("#save-edit").click();
        await expect(page.locator("#edit-dialog")).not.toBeVisible();
        expect((await hub.command("hub_snapshot")).deployments[0].deployment.upstream_credential_id ?? null).toBeNull();
        await refreshAuthenticatedModel(page, provider);
        await page.reload();
        await row.getByRole("button", { name: /を編集/ }).click();
        await expect(page.locator("#edit-api-key-status")).toContainText("未設定");
        await expect(page.locator("#edit-api-key-clear-label")).toBeHidden();
        await noDisplayedOrBrowserStoredKey(page, hub);
    } finally { await provider.close(); }
});

test("Changing or clearing an API key invalidates old model candidates including delayed discovery", async ({ page, hub }) => {
    const provider = await createMetadataProvider();
    provider.setApiKey(originalKey);
    provider.setModels(["old-key-model"]);
    const hold = provider.holdNext();
    try {
        await ready(page, hub);
        await page.locator("#api-key").fill(originalKey);
        await page.locator("#endpoint").fill(provider.url);
        await page.locator("#discover").click();
        await hold.arrival;
        provider.setApiKey(replacementKey);
        provider.setModels(["new-key-model"]);
        await page.locator("#api-key").fill(replacementKey);
        await expect(page.locator("#model")).toBeDisabled();
        await expect(page.locator("#register")).toBeDisabled();
        await page.locator("#discover").click();
        await expect(page.locator("#model option")).toHaveText(["モデルを選択", "new-key-model — new-key-model"]);
        await page.locator("#model").selectOption("new-key-model");
        const oldResponse = page.waitForResponse(response => response.url().endsWith("/admin/command")
            && response.request().postDataJSON()?.command === "hub_discover"
            && response.request().postDataJSON()?.args?.apiKey === originalKey);
        hold.release();
        await oldResponse;
        await expect(page.locator("#model")).toHaveValue("new-key-model");
        await page.locator("#api-key").fill("");
        await expect(page.locator("#model")).toBeDisabled();
        await expect(page.locator("#register")).toBeDisabled();
        await page.locator("#discover").click();
        await expect(page.locator("#discovery-status")).toHaveClass(/error/);
        await expect(page.locator("#model")).toBeDisabled();
        await expect(page.locator("#register")).toBeDisabled();
        expect((await hub.command("hub_snapshot")).deployments).toHaveLength(0);
    } finally { hold.release(); await provider.close(); }
});

test("An entered API key reaches a changed host or profile only after explicit discovery", async ({ page, hub }) => {
    const first = await createMetadataProvider(), next = await createMetadataProvider();
    first.setApiKey(originalKey);
    next.setApiKey(originalKey);
    next.setModels(["next-host-model"]);
    next.setNativeModels(["next-profile-model"]);
    async function waitForBackgroundUpdates() {
        // Two actual polling cycles span the discovery debounce without relying
        // on a sleep or inspecting the application's private timers.
        for (let count = 0; count < 2; count++) {
            await page.waitForResponse(response => response.url().endsWith("/admin/command")
                && response.request().postDataJSON()?.command === "hub_snapshot");
        }
    }
    try {
        await ready(page, hub);
        await page.locator("#api-key").fill(originalKey);
        await page.locator("#endpoint").fill(first.url);
        await page.locator("#discover").click();
        await expect(page.locator("#model")).toBeEnabled();

        await page.locator("#endpoint").fill(next.url);
        await page.locator("#label").focus();
        await waitForBackgroundUpdates();
        expect(next.requests).toHaveLength(0);
        await expect(page.locator("#model")).toBeDisabled();
        await page.locator("#discover").click();
        await expect(page.locator("#model option")).toHaveText(["モデルを選択", "next-host-model — next-host-model"]);
        expect(next.requests).toEqual([{ method: "GET", path: "/v1/models", authorized: true }]);

        await page.locator("#profile").selectOption("lmstudio_chat");
        await waitForBackgroundUpdates();
        expect(next.requests).toHaveLength(1);
        await expect(page.locator("#model")).toBeDisabled();
        await page.locator("#discover").click();
        await expect(page.locator("#model option")).toHaveText(["モデルを選択", "next-profile-model — next-profile-model"]);
        expect(next.requests.at(-1)).toEqual({ method: "GET", path: "/api/v1/models", authorized: true });

        const before = first.requests.length;
        await page.locator("#endpoint").fill(first.url);
        await waitForBackgroundUpdates();
        expect(first.requests).toHaveLength(before);
        await expect(page.locator("#model")).toBeDisabled();
        await page.locator("#discover").click();
        await expect(page.locator("#model option")).toHaveText(["モデルを選択", "fixture-native — fixture-native"]);
        expect(first.requests.at(-1)).toEqual({ method: "GET", path: "/api/v1/models", authorized: true });

        next.setApiKey(null);
        await page.locator("#api-key").fill("");
        await page.locator("#endpoint").fill(next.url);
        await expect(page.locator("#model option")).toHaveText(["モデルを選択", "next-profile-model — next-profile-model"]);
        await expect(page.locator("#model")).toBeEnabled();
    } finally { await first.close(); await next.close(); }
});
