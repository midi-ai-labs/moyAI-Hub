import { test, expect } from "./fixture.mjs";
import { createDeviceParticipant } from "./device-fixture.mjs";

async function open(page, hub) {
  await page.goto(hub.url);
  await page.locator('nav a[href="#shared-administration"]').click();
  await expect(page.locator('#shared-admin-content')).toContainText("利用者を追加");
}
async function tab(page, name) {
  const id = { "利用者": "users", "利用者グループ": "groups", "プロジェクト": "projects", "仕事の実行先": "environments", "同時利用の枠": "resources", "停止状態の確認": "maintenance", "保存と保守": "storage" }[name];
  await page.locator(`[data-sa-tab="${id}"]`).click();
}
async function operation(page, kind, id) {
  const selector = `button[data-sa-operation="${kind}"]${id ? `[data-sa-id="${id}"]` : ""}`;
  await page.locator(selector).first().click();
}
async function save(page) {
  await page.locator("#shared-admin-save").click();
  await expect(page.locator("#shared-admin-notice")).toHaveText("設定を保存しました。");
  await expect(page.locator("#shared-admin-form")).toHaveCount(0);
}

test("administrators configure people, groups, project roles and capacity through real forms", async ({ page, hub }, info) => {
  test.setTimeout(120_000);
  await open(page, hub);
  await operation(page, "create_user");
  await page.locator("#shared-admin-username").fill("design-alice");
  await page.locator("#shared-admin-display_name").fill("設計担当 Alice");
  await page.locator("#shared-admin-password").fill("fixture-password-alice");
  await save(page);
  await expect(page.locator("#shared-admin-content")).toContainText("設計担当 Alice");
  await tab(page, "利用者グループ"); await operation(page, "create_group");
  await page.locator("#shared-admin-id").fill("engineering"); await page.locator("#shared-admin-label").fill("設計グループ"); await save(page);
  await operation(page, "group_member", "engineering");
  await page.locator("#shared-admin-user_id").selectOption({ label: "設計担当 Alice (design-alice)" });
  await save(page);
  await expect(page.locator("#shared-admin-content")).toContainText("所属: 設計担当 Alice");
  await tab(page, "プロジェクト"); await operation(page, "create_project");
  await page.locator("#shared-admin-id").fill("design-a"); await page.locator("#shared-admin-label").fill("設計プロジェクト A"); await save(page);
  await operation(page, "group_project_role", "design-a");
  await page.locator("#shared-admin-group_id").selectOption("engineering");
  await page.locator("#shared-admin-role").selectOption("contributor"); await save(page);
  await expect(page.locator("#shared-admin-content")).toContainText("グループ 設計グループ: 仕事の投入・実行");
  await info.attach("shared-project-group-role", { body: await page.locator("#shared-administration").screenshot(), contentType: "image/png" });
  await tab(page, "同時利用の枠"); await operation(page, "set_resource");
  await page.locator("#shared-admin-id").fill("engineering-pc"); await page.locator("#shared-admin-capacity").fill("1"); await save(page);
  await expect(page.locator("#shared-admin-content")).toContainText("同時利用上限: 1");
  await page.locator('nav a[href="#device-network"]').click();
  await page.locator("#network-ip").fill("127.0.0.1"); await page.locator("#network-port").fill(String(hub.networkPort));
  await page.locator("#network-start").click(); await page.locator("#network-stop").waitFor();
  const participant = await createDeviceParticipant(await hub.observeNetwork(), "設計用 PC");
  await page.locator("#network-refresh").click();
  await page.locator("#network-open-pending").click();
  await expect(page.locator("#network-filter")).toHaveValue("pending");
  await page.locator(`[data-id="request:${participant.requestId}"] button[data-network-action]`).click();
  await participant.collectApproval(); await participant.presence();
  await page.locator("#network-clients-refresh").click();
  await page.locator('nav a[href="#shared-administration"]').click(); await tab(page, "仕事の実行先");
  await operation(page, "create_environment");
  await page.locator("#shared-admin-id").fill("engineering-analysis"); await page.locator("#shared-admin-label").fill("設計の解析環境");
  await page.locator("#shared-admin-runner_id").selectOption({ label: "設計用 PC" });
  await page.locator("#shared-admin-resource_id").fill("engineering-pc"); await expect(page.locator("#shared-admin-capacity")).toHaveValue("1");
  await page.locator('input[name="project_ids"][value="design-a"]').check(); await save(page);
  await expect(page.locator("#shared-admin-content")).toContainText("共有枠: engineering-pc / 公開先: 設計プロジェクト A");
  await operation(page, "update_environment", "engineering-analysis"); await page.locator('input[name="enabled"]').uncheck(); await save(page);
  await expect(page.locator("#shared-admin-content")).toContainText("新規受付停止");
  await info.attach("shared-environment-project-resource", { body: await page.locator("#shared-administration").screenshot(), contentType: "image/png" });
  await participant.sharedTemplates([{ id: "analysis-template", label: "解析用フォルダ" }]);
  await page.locator("#shared-admin-refresh").click();
  await page.locator('[data-sa-template="analysis-template"]').click();
  const generatedId = await page.locator("#shared-admin-id").inputValue();
  expect(generatedId).toMatch(/^environment-/);
  await expect(page.locator("#shared-admin-label")).toHaveValue("解析用フォルダ");
  await expect(page.locator("#shared-admin-template_id")).toHaveValue("analysis-template");
  await expect(page.locator("#shared-admin-resource_id")).toHaveValue("engineering-pc");
  await expect(page.locator("#shared-admin-capacity")).toHaveValue("1");
  expect(await page.locator("#shared-admin-capacity").evaluate(node => node.readOnly)).toBe(true);
  await expect(page.locator('input[name="project_ids"]:checked')).toHaveCount(0);
  await page.locator("#shared-admin-refresh").click();
  await expect(page.locator("#shared-admin-id")).toHaveValue(generatedId);
  await page.locator('input[name="project_ids"][value="design-a"]').check();
  await info.attach("shared-template-ready-to-save", { body: await page.locator("#shared-administration").screenshot(), contentType: "image/png" });
  const sent = [];
  const capture = request => {
    if (new URL(request.url()).pathname !== "/admin/command" || request.method() !== "POST") return;
    const body = request.postDataJSON();
    if (body.command === "hub_shared_command") sent.push(body.args);
  };
  page.on("request", capture);
  await save(page);
  page.off("request", capture);
  expect(sent).toHaveLength(1);
  expect(sent[0].request).toMatchObject({ kind: "create_environment", id: generatedId, template_id: "analysis-template", resource_id: "engineering-pc", capacity: 1, project_ids: ["design-a"] });
  expect(await participant.sharedProvisioning()).toEqual([{ environment_id: generatedId, template_id: "analysis-template", generation: 1 }]);
  await expect(page.locator("#shared-admin-content")).toContainText("フォルダ作成待ち");
  await info.attach("shared-template-single-save", { body: await page.locator("#shared-administration").screenshot(), contentType: "image/png" });
  await tab(page, "保存と保守"); await operation(page, "prune_shared_data");
  await page.locator("#shared-admin-cutoff").fill("2020-01-01");
  await page.locator("#shared-admin-preview").click();
  await expect(page.locator("#shared-admin-preview-result")).toContainText("0件 / 0 bytes");
  await page.getByLabel("表示された保存内容の削除を確認した（仕事の記録は保持）", { exact: true }).check();
  await save(page);
  await info.attach("shared-storage-maintenance", { body: await page.locator("#shared-administration").screenshot(), contentType: "image/png" });
});

test("a concurrent administrative edit retains the stale draft and requires an explicit refresh", async ({ page, context, hub }) => {
  await open(page, hub);
  await tab(page, "利用者グループ"); await operation(page, "create_group");
  await page.locator("#shared-admin-id").fill("left-draft"); await page.locator("#shared-admin-label").fill("編集中の内容");
  const other = await context.newPage(); await open(other, hub);
  await tab(other, "利用者グループ"); await operation(other, "create_group");
  await other.locator("#shared-admin-id").fill("right-saved"); await other.locator("#shared-admin-label").fill("別画面の保存"); await save(other);
  await page.locator("#shared-admin-save").click();
  await expect(page.locator("#shared-admin-form-error")).toContainText("設定が変更されています");
  await expect(page.locator("#shared-admin-label")).toHaveValue("編集中の内容");
  await expect(page.locator("#shared-admin-save")).toBeDisabled();
  await page.locator("#shared-admin-close").click(); await page.locator("#shared-admin-refresh").click();
  await expect(page.locator("#shared-admin-content")).toContainText("別画面の保存");
  await expect(page.locator("#shared-admin-content")).not.toContainText("編集中の内容");
});
