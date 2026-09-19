import { test, expect, administrator } from "./fixture.mjs";
import { createDeviceParticipant } from "./device-fixture.mjs";

async function open(page, hub) {
  await page.goto(hub.url);
  await page.locator('nav a[href="#shared-administration"]').click();
  await expect(page.locator('#shared-admin-content')).toContainText("プロジェクトを追加");
}
async function operation(page, kind, id) {
  await page.locator(`button[data-sa-operation="${kind}"]${id ? `[data-sa-id="${id}"]` : ""}`).first().click();
}
async function save(page) {
  await page.locator("#shared-admin-save").click();
  await expect(page.locator("#shared-admin-notice")).toHaveText("設定を保存しました。");
  await expect(page.locator("#shared-admin-form")).toHaveCount(0);
}
async function enroll(page,hub,label) {
  const participant=await createDeviceParticipant(await hub.observeNetwork(),label);
  await page.locator("#network-refresh").click();
  await page.locator("#network-open-pending").click();
  await page.locator(`[data-id="request:${participant.requestId}"] button[data-network-action]`).click();
  const {deviceId}=await participant.collectApproval(); await participant.presence();
  await page.locator("#network-clients-refresh").click();
  return {participant,deviceId};
}

test("one project form assigns people and both PC purposes and automatically prepares a shared execution PC", async ({page,hub},info)=>{
  test.setTimeout(120_000);
  await open(page,hub);
  await expect(page.locator('[data-sa-tab="groups"], [data-sa-tab="environments"], [data-sa-tab="resources"]')).toHaveCount(0);
  await page.locator('[data-sa-tab="users"]').click(); await operation(page,"create_user");
  await page.locator("#shared-admin-username").fill("design-alice");
  await page.locator("#shared-admin-display_name").fill("設計担当 Alice");
  await page.locator("#shared-admin-password").fill("fixture-password-alice"); await save(page);
  await page.locator('nav a[href="#device-network"]').click();
  await page.locator("#network-ip").fill("127.0.0.1"); await page.locator("#network-port").fill(String(hub.networkPort));
  await page.locator("#network-start").click(); await page.locator("#network-stop").waitFor();
  await expect(page.locator("#network-group-form")).toBeHidden(); await expect(page.locator("#network-rule-form")).toBeHidden();
  const first=await enroll(page,hub,"操作用 PC"); const second=await enroll(page,hub,"解析用 PC");
  await first.participant.sharedLogin(administrator.username,administrator.password);
  await page.locator('nav a[href="#shared-administration"]').click(); await page.locator('[data-sa-tab="projects"]').click();
  await operation(page,"save_project");
  await page.locator("#shared-admin-label").fill("設計プロジェクト A");
  await page.getByRole("combobox",{name:"設計担当 Alice",exact:true}).selectOption("contributor");
  await page.getByLabel("操作用 PCを操作に使う",{exact:true}).check();
  await page.getByLabel("解析用 PCを操作に使う",{exact:true}).check();
  await page.getByLabel("解析用 PCで仕事を実行する",{exact:true}).check();
  await expect(page.locator('#shared-admin-id, #shared-admin-resource_id, #shared-admin-template_id')).toHaveCount(0);
  await info.attach("project-people-pc-single-form",{body:await page.locator("#shared-administration").screenshot(),contentType:"image/png"});
  const sent=[]; const capture=request=>{if(new URL(request.url()).pathname==="/admin/command"&&request.method()==="POST"){const body=request.postDataJSON();if(body.command==="hub_shared_command")sent.push(body.args);}};
  page.on("request",capture); await save(page); page.off("request",capture);
  expect(sent).toHaveLength(1); const projectId=sent[0].request.id;
  expect(sent[0].request).toMatchObject({kind:"save_project",label:"設計プロジェクト A",controller_device_ids:[first.deviceId,second.deviceId],runner_device_ids:[second.deviceId]});
  await expect(page.locator("#shared-admin-content")).toContainText("このPCの実行設定待ち");
  const selected=await second.participant.sharedDeviceProjects();
  expect(selected.projects.find(row=>row.id===projectId)).toMatchObject({can_control:true,can_execute:true,preparation_state:"waiting_setup",environment_id:null});
  // This authenticated fixture acknowledges provisioning; actual filesystem
  // creation and embedded Runner execution are covered by the Desktop scenario.
  await second.participant.sharedTemplates([{id:"desktop-default",label:"このPCの実行設定"}]);
  const [request]=await second.participant.sharedProvisioning();
  expect(request.template_id).toBe("desktop-default");
  const preparing=await first.participant.sharedCall("snapshot");
  expect(preparing.environments.find(row=>row.id===request.environment_id).enabled).toBe(false);
  await page.locator("#shared-admin-refresh").click(); await expect(page.locator("#shared-admin-content")).toContainText("作業フォルダを準備中");
  await second.participant.sharedProvisionReceipt({...request,success:true,error:null});
  await page.locator("#shared-admin-refresh").click(); await expect(page.locator("#shared-admin-content")).toContainText("利用可能");
  await info.attach("project-pc-preparation-ready",{body:await page.locator("#shared-administration").screenshot(),contentType:"image/png"});
  await operation(page,"save_project"); await page.locator("#shared-admin-label").fill("設計プロジェクト B");
  await page.getByRole("combobox",{name:"設計担当 Alice",exact:true}).selectOption("contributor");
  await page.getByLabel("操作用 PCを操作に使う",{exact:true}).check(); await page.getByLabel("解析用 PCで仕事を実行する",{exact:true}).check(); await save(page);
  const requests=await second.participant.sharedProvisioning(); expect(requests).toHaveLength(1);
  await second.participant.sharedProvisionReceipt({...requests[0],success:true,error:null});
  const shared=await first.participant.sharedCall("snapshot"); expect(shared.environments).toHaveLength(2);
  expect(shared.environments[0].resource_id).toBe(shared.environments[1].resource_id); expect(shared.resources).toHaveLength(1); expect(shared.resources[0].capacity).toBe(1);
  await page.locator("#shared-admin-refresh").click();
  await expect(page.locator("#shared-admin-content").getByText(/解析用 PC: 利用可能/)).toHaveCount(2);
  await operation(page,"save_project",projectId); await page.getByLabel("解析用 PCで仕事を実行する",{exact:true}).uncheck(); await save(page);
  const removed=await first.participant.sharedCall("snapshot");
  expect(removed.environments.find(row=>row.id===request.environment_id).enabled).toBe(false);
  expect(removed.environments.find(row=>row.id===requests[0].environment_id).enabled).toBe(true);
  await page.locator('[data-sa-tab="storage"]').click(); await operation(page,"prune_shared_data");
  await page.locator("#shared-admin-cutoff").fill("2020-01-01"); await page.locator("#shared-admin-preview").click();
  await expect(page.locator("#shared-admin-preview-result")).toContainText("0件 / 0 bytes");
  await page.getByLabel("表示された保存内容の削除を確認した（仕事の記録は保持）",{exact:true}).check(); await save(page);
  await info.attach("shared-storage-maintenance",{body:await page.locator("#shared-administration").screenshot(),contentType:"image/png"});
});

test("a concurrent project edit retains the stale people and PC draft and requires an explicit refresh",async({page,context,hub})=>{
  await open(page,hub); await operation(page,"save_project"); await page.locator("#shared-admin-label").fill("編集中の内容");
  const other=await context.newPage(); await open(other,hub); await operation(other,"save_project");
  await other.locator("#shared-admin-label").fill("別画面の保存"); await save(other);
  await page.locator("#shared-admin-save").click(); await expect(page.locator("#shared-admin-form-error")).toContainText("設定が変更されています");
  await expect(page.locator("#shared-admin-label")).toHaveValue("編集中の内容"); await expect(page.locator("#shared-admin-save")).toBeDisabled();
  await page.locator("#shared-admin-close").click(); await page.locator("#shared-admin-refresh").click();
  await expect(page.locator("#shared-admin-content")).toContainText("別画面の保存"); await expect(page.locator("#shared-admin-content")).not.toContainText("編集中の内容");
});

test("a deleted final PC can be removed from both project purposes even when the editor has no PC checkboxes",async({page,hub},info)=>{
  await open(page,hub);
  await page.locator('nav a[href="#device-network"]').click();
  await page.locator("#network-ip").fill("127.0.0.1"); await page.locator("#network-port").fill(String(hub.networkPort));
  await page.locator("#network-start").click(); await page.locator("#network-stop").waitFor();
  const {deviceId}=await enroll(page,hub,"廃止予定 PC");
  await page.locator('nav a[href="#shared-administration"]').click();
  await operation(page,"save_project"); await page.locator("#shared-admin-label").fill("PC入替えプロジェクト");
  await page.getByLabel("廃止予定 PCを操作に使う",{exact:true}).check();
  await page.getByLabel("廃止予定 PCで仕事を実行する",{exact:true}).check(); await save(page);
  const card=page.locator(".shared-admin-row").filter({has:page.getByRole("heading",{name:"PC入替えプロジェクト",exact:true})});
  const projectId=await card.locator('[data-sa-operation="save_project"]').getAttribute("data-sa-id");
  await page.locator('nav a[href="#clients"]').click();
  await page.locator("#network-filter").selectOption("all");
  const pc=page.locator(`[data-id="device:${deviceId}"]`);
  await pc.getByRole("button",{name:"廃止予定 PCの登録を削除",exact:true}).click();
  await page.locator("#network-delete-confirm").click(); await expect(pc).toHaveCount(0);
  await page.locator('nav a[href="#shared-administration"]').click(); await page.locator("#shared-admin-refresh").click();
  await operation(page,"save_project",projectId);
  await expect(page.locator('[name="controller_device_ids"], [name="runner_device_ids"]')).toHaveCount(0);
  await page.locator("#shared-admin-label").fill("PCの再登録を待つプロジェクト");
  await info.attach("project-no-registered-pcs",{body:await page.locator("#shared-administration").screenshot(),contentType:"image/png"});
  await save(page);
  await expect(page.locator("#shared-admin-content")).toContainText("PCの再登録を待つプロジェクト");
  await operation(page,"save_project",projectId);
  await expect(page.locator('[name="controller_device_ids"], [name="runner_device_ids"]')).toHaveCount(0);
});
