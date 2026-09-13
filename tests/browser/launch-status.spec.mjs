import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { test, expect } from "./fixture.mjs";

const execute = promisify(execFile);

test("Launch shows Hub lifetime independently of the model listener and detects shutdown", async ({ page, hub }, testInfo) => {
    await page.goto(`${hub.url}#hub-started`);
    await expect(page.locator("#header-status")).toHaveText("Hub本体は稼働中");
    await expect(page.locator("#notice")).toContainText("moyAI Hub を起動しました");
    expect((await hub.command("hub_snapshot")).server.running).toBe(false);
    await expect(page).toHaveURL(hub.url);
    await testInfo.attach("started-status", { body: await page.screenshot(), contentType: "image/png" });
    await page.reload();
    await expect(page.locator("#header-status")).toHaveText("Hub本体は稼働中");
    await expect(page.locator("#notice")).toBeHidden();
    expect((await hub.close()).pass).toBe(true);
    await expect(page.locator("#header-status")).toHaveText("Hub本体の応答を確認できません");
    await testInfo.attach("stopped-status", { body: await page.screenshot(), contentType: "image/png" });
});

test("Repeated launches retain the same Hub and explain the existing instance in the browser", async ({ page, hub }, testInfo) => {
    const replies = await Promise.all([0, 1].map(async () => {
        const { stdout, stderr } = await execute(hub.binary,
            ["--launch", "--no-browser", "--data-dir", hub.dataDirectory],
            { windowsHide: true, timeout: 25_000 });
        expect(stderr).toContain("起動済み");
        return JSON.parse(stdout);
    }));
    for (const reply of replies) {
        expect(reply.pid).toBe(hub.pid);
        expect(reply.already_running).toBe(true);
        expect(reply.web_url).toBe(hub.url);
    }
    await page.goto(`${replies[0].web_url}#hub-already-running`);
    await expect(page.locator("#notice")).toHaveText("moyAI Hub は起動済みです。稼働中の Hub の管理画面を開きました。");
    await expect(page.locator("#header-status")).toHaveText("Hub本体は稼働中");
    await expect(page).toHaveURL(hub.url);
    await testInfo.attach("existing-hub-status", { body: await page.screenshot(), contentType: "image/png" });
});
