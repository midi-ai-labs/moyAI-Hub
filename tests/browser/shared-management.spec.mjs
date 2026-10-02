import { test, expect } from "./fixture.mjs";
import { createDeviceParticipant } from "./device-fixture.mjs";
import { randomUUID } from "node:crypto";


async function sharedSnapshot(page,hub) {
  const session=await (await page.request.get(new URL("/admin/session",hub.url).href)).json();
  const response=await page.request.post(new URL("/admin/command",hub.url).href,{headers:{Origin:new URL(hub.url).origin,"X-Moyai-Csrf":session.csrf,"X-Moyai-Session":session.authentication.session_id},data:{command:"hub_shared_snapshot",args:{}}});
  expect(response.ok()).toBe(true); return (await response.json()).result;
}

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
  await page.locator("#join-project-save").click();
  await expect(page.locator("#join-project-dialog")).not.toBeVisible();
  const {deviceId}=await participant.collectApproval(); await participant.presence();
  await page.locator("#network-clients-refresh").click();
  return {participant,deviceId};
}

test("one project form assigns both PC purposes and waits for the execution PC to choose its local folder", async ({page,hub},info)=>{
  test.setTimeout(120_000);
  await open(page,hub);
  await expect(page.locator('[data-sa-tab="groups"], [data-sa-tab="environments"], [data-sa-tab="resources"]')).toHaveCount(0);
  await page.locator('nav a[href="#device-network"]').click();
  await page.locator("#network-ip").fill("127.0.0.1"); await page.locator("#network-port").fill(String(hub.networkPort));
  await page.locator("#network-start").click(); await page.locator("#network-stop").waitFor();
  await expect(page.locator("#network-group-form")).toBeHidden(); await expect(page.locator("#network-rule-form")).toBeHidden();
  const first=await enroll(page,hub,"操作用 PC"); const second=await enroll(page,hub,"解析用 PC");
  await first.participant.sharedDeviceSession();
  await page.locator('nav a[href="#shared-administration"]').click(); await page.locator('[data-sa-tab="projects"]').click();
  await operation(page,"save_project");
  await page.locator("#shared-admin-label").fill("設計プロジェクト A");
  await page.getByLabel("操作用 PCを操作に使う",{exact:true}).check();
  await page.getByLabel("解析用 PCを操作に使う",{exact:true}).check();
  await page.getByLabel("解析用 PCで仕事を実行する",{exact:true}).check();
  await expect(page.locator('#shared-admin-id, #shared-admin-resource_id, #shared-admin-template_id')).toHaveCount(0);
  await info.attach("project-pc-single-form",{body:await page.locator("#shared-administration").screenshot(),contentType:"image/png"});
  const sent=[]; const capture=request=>{if(new URL(request.url()).pathname==="/admin/command"&&request.method()==="POST"){const body=request.postDataJSON();if(body.command==="hub_shared_command")sent.push(body.args);}};
  page.on("request",capture); await save(page); page.off("request",capture);
  expect(sent).toHaveLength(1); const projectId=sent[0].request.id;
  expect(sent[0].request).not.toHaveProperty("members");
  expect(sent[0].request).toMatchObject({kind:"save_project",label:"設計プロジェクト A",controller_device_ids:[first.deviceId,second.deviceId],runner_device_ids:[second.deviceId]});
  await expect(page.locator("#shared-admin-content")).toContainText("このPCでプロジェクトの作業フォルダーを選んでください");
  await expect(page.locator("#shared-admin-content")).not.toContainText("waiting_local_folder");
  await info.attach("project-local-folder-selection",{body:await page.locator("#shared-administration").screenshot(),contentType:"image/png"});
  const selected=await second.participant.sharedDeviceProjects();
  expect(selected.projects.find(row=>row.id===projectId)).toMatchObject({can_control:true,can_execute:true,preparation_state:"waiting_local_folder",environment_id:expect.any(String)});
  // This authenticated fixture acknowledges local-folder selection; actual folder
  // access and embedded Runner execution are covered by the Desktop scenario.
  const [request]=await second.participant.sharedProvisioning();
  expect(request.template_id).toBe("local-folder");
  const preparing=await sharedSnapshot(page,hub);
  expect(preparing.environments.find(row=>row.id===request.environment_id).enabled).toBe(false);
  await page.locator("#shared-admin-refresh").click(); await expect(page.locator("#shared-admin-content")).toContainText("作業フォルダーを選んでください");
  await second.participant.sharedProvisionReceipt({environment_id:request.environment_id,template_id:request.template_id,generation:request.generation,success:true,error:null});
  await page.locator("#shared-admin-refresh").click(); await expect(page.locator("#shared-admin-content")).toContainText("作業フォルダーの準備完了");
  await operation(page,"set_environment_capabilities",request.environment_id);
  await expect(page.locator("#shared-admin-editor")).toContainText("実際に動くことの確認にはなりません");
  await page.locator("#shared-admin-names").fill("Flask\nGurobi"); await save(page);
  const configured=await sharedSnapshot(page,hub);
  expect(configured.environments.find(row=>row.id===request.environment_id).capabilities.map(item=>item.name)).toEqual(["Flask","Gurobi"]);
  await expect(page.locator("#shared-admin-content")).toContainText("このPCで使えるもの: Flask、Gurobi");
  await info.attach("project-pc-preparation-ready",{body:await page.locator("#shared-administration").screenshot(),contentType:"image/png"});
  await operation(page,"save_project"); await page.locator("#shared-admin-label").fill("設計プロジェクト B");
  await page.getByLabel("操作用 PCを操作に使う",{exact:true}).check(); await page.getByLabel("解析用 PCで仕事を実行する",{exact:true}).check(); await save(page);
  const requests=await second.participant.sharedProvisioning(); expect(requests).toHaveLength(1);
  await second.participant.sharedProvisionReceipt({environment_id:requests[0].environment_id,template_id:requests[0].template_id,generation:requests[0].generation,success:true,error:null});
  const shared=await sharedSnapshot(page,hub); expect(shared.environments).toHaveLength(2);
  expect(shared.environments[0].resource_id).toBe(shared.environments[1].resource_id); expect(shared.resources).toHaveLength(1); expect(shared.resources[0].capacity).toBe(1);
  await page.locator("#shared-admin-refresh").click();
  await expect(page.locator("#shared-admin-content").getByText(/解析用 PC: 作業フォルダーの準備完了/)).toHaveCount(2);
  await operation(page,"save_project",projectId); await page.getByLabel("解析用 PCで仕事を実行する",{exact:true}).uncheck(); await save(page);
  const removed=await sharedSnapshot(page,hub);
  // Retired project bindings remain historical records, but the management DTO
  // lists only current environments and provisioning requests.
  expect(removed.environments.some(row=>row.id===request.environment_id)).toBe(false);
  expect(removed.provisioning.some(row=>row.environment_id===request.environment_id)).toBe(false);
  expect(removed.project_devices.find(row=>row.project_id===projectId&&row.device_id===second.deviceId))
    .toMatchObject({can_control:true,can_execute:false,environment_id:null,preparation_state:"not_selected"});
  expect(removed.environments.find(row=>row.id===requests[0].environment_id).enabled).toBe(true);
  expect(removed.resources).toHaveLength(1);
  expect(removed.resources[0].capacity).toBe(1);
  expect(await second.participant.sharedProvisioning()).toEqual([]);
  const retiredProject=page.locator(".shared-admin-row").filter({has:page.getByRole("heading",{name:"設計プロジェクト A",exact:true})});
  await expect(retiredProject).toContainText("仕事を実行するPC未選択");
  // Existing groups remain an API/data contract even though their creation form
  // is absent. Exercise the surviving users UI against one such stored group.
  const groupId="existing-research", userId=removed.device_principals.find(row=>row.device_id===first.deviceId).user_id;
  const session=await (await page.request.get(new URL("/admin/session",hub.url).href)).json();
  const groupResponse=await page.request.post(new URL("/admin/command",hub.url).href,{
    headers:{Origin:new URL(hub.url).origin,"X-Moyai-Csrf":session.csrf,"X-Moyai-Session":session.authentication.session_id},
    data:{command:"hub_shared_command",args:{request_id:randomUUID(),expected_revision:removed.revision,request:{kind:"create_group",id:groupId,label:"既存の研究グループ"}}},
  });
  expect(groupResponse.ok()).toBe(true);
  await open(page,hub);
  await page.getByText("既存の利用者・権限（詳細）",{exact:true}).click();
  await page.locator('[data-sa-tab="users"]').click();
  await operation(page,"group_member");
  await page.locator("#shared-admin-group_id").selectOption(groupId);
  await page.locator("#shared-admin-user_id").selectOption(userId);
  await page.getByLabel("このグループへ所属させる",{exact:true}).check(); await save(page);
  await operation(page,"group_project_role");
  await page.locator("#shared-admin-project_id").selectOption(projectId);
  await page.locator("#shared-admin-group_id").selectOption(groupId);
  await page.locator("#shared-admin-role").selectOption("reader"); await save(page);
  await operation(page,"project_member");
  await page.locator("#shared-admin-project_id").selectOption(projectId);
  await page.locator("#shared-admin-user_id").selectOption(userId);
  await page.locator("#shared-admin-role").selectOption("manager"); await save(page);
  const permissions=await sharedSnapshot(page,hub);
  expect(permissions.group_memberships).toContainEqual({group_id:groupId,user_id:userId});
  expect(permissions.group_project_roles).toContainEqual({group_id:groupId,project_id:projectId,role:"reader"});
  expect(permissions.project_memberships).toContainEqual({user_id:userId,project_id:projectId,role:"manager"});
  expect(permissions.resources).toEqual(removed.resources);
  await info.attach("existing-users-permissions",{body:await page.locator("#shared-administration").screenshot(),contentType:"image/png"});
  await page.locator('[data-sa-tab="storage"]').click(); await operation(page,"prune_shared_data");
  await page.locator("#shared-admin-cutoff").fill("2020-01-01"); await page.locator("#shared-admin-preview").click();
  await expect(page.locator("#shared-admin-preview-result")).toContainText("0件 / 0 bytes");
  await page.getByLabel("表示された保存内容の削除を確認した（仕事の記録は保持）",{exact:true}).check(); await save(page);
  await info.attach("shared-storage-maintenance",{body:await page.locator("#shared-administration").screenshot(),contentType:"image/png"});
});

test("a concurrent project edit keeps its draft through comparison and repeated conflicts",async({page,context,hub},info)=>{
  await open(page,hub); await operation(page,"save_project"); await page.locator("#shared-admin-label").fill("編集中の内容");
  const other=await context.newPage(); await open(other,hub); await operation(other,"save_project");
  await other.locator("#shared-admin-label").fill("別画面の保存"); await save(other);
  await page.locator("#shared-admin-save").click(); await expect(page.locator("#shared-admin-form-error")).toContainText("入力は残っています");
  await expect(page.locator("#shared-admin-label")).toHaveValue("編集中の内容"); await expect(page.locator("#shared-admin-save")).toBeDisabled();
  await page.locator("#shared-admin-review-conflict").click(); await expect(page.locator("#shared-admin-comparison")).toContainText("編集中の内容");
  await expect(page.locator("#shared-admin-save")).toBeDisabled();
  await page.locator("#shared-admin-label").fill("保持した入力を修正"); await expect(page.locator("#shared-admin-comparison")).toBeHidden();
  await expect(page.locator("#shared-admin-save")).toBeDisabled();
  await page.locator("#shared-admin-review-conflict").click(); await page.locator("#shared-admin-accept-comparison").click();
  await expect(page.locator("#shared-admin-label")).toHaveValue("保持した入力を修正");
  await operation(other,"save_project"); await other.locator("#shared-admin-label").fill("比較後の別保存"); await save(other);
  await page.locator("#shared-admin-save").click(); await expect(page.locator("#shared-admin-form-error")).toContainText("入力は残っています");
  await page.locator("#shared-admin-review-conflict").click();
  await info.attach("conflict-comparison-keeps-input",{body:await page.locator("#shared-administration").screenshot(),contentType:"image/png"});
  await page.locator("#shared-admin-accept-comparison").click(); await save(page);
  await expect(page.locator("#shared-admin-content")).toContainText("別画面の保存");
  await expect(page.locator("#shared-admin-content")).toContainText("保持した入力を修正");
});

test("a deleted project stays blocked during comparison and is never recreated from the draft",async({page,context,hub})=>{
  await open(page,hub); await operation(page,"save_project"); await page.locator("#shared-admin-label").fill("削除される対象"); await save(page);
  const row=page.locator(".shared-admin-row").filter({has:page.getByRole("heading",{name:"削除される対象",exact:true})});
  const id=await row.locator('[data-sa-operation="save_project"]').getAttribute("data-sa-id");
  await operation(page,"save_project",id); await page.locator("#shared-admin-label").fill("保持中の名前");
  const other=await context.newPage(); await open(other,hub); await operation(other,"delete_project",id); await save(other);
  await page.locator("#shared-admin-save").click(); await expect(page.locator("#shared-admin-form-error")).toContainText("入力は残っています");
  await page.locator("#shared-admin-review-conflict").click(); await expect(page.locator("#shared-admin-comparison")).toContainText("削除されました");
  await expect(page.locator("#shared-admin-accept-comparison")).toHaveCount(0); await expect(page.locator("#shared-admin-save")).toBeDisabled();
  await expect(page.locator("#shared-admin-label")).toHaveValue("保持中の名前");
  await page.locator('[data-sa-tab="storage"]').click(); await expect(page.locator("#shared-admin-form")).toHaveCount(0);
  await page.locator('[data-sa-tab="projects"]').click(); await expect(page.locator("#shared-admin-content")).not.toContainText("保持中の名前");
});

test("a delayed comparison cannot return a previous session's draft after access closes",async({page,context,hub})=>{
  await open(page,hub); await operation(page,"save_project"); await page.locator("#shared-admin-label").fill("前の管理画面の入力");
  const other=await context.newPage(); await open(other,hub); await operation(other,"save_project"); await other.locator("#shared-admin-label").fill("別の保存"); await save(other);
  await page.locator("#shared-admin-save").click(); await expect(page.locator("#shared-admin-form-error")).toContainText("入力は残っています");
  let release; const gate=new Promise(resolve=>{release=resolve;}); let held=false;
  await page.route("**/admin/command",async route=>{
    if(route.request().postDataJSON()?.command!=="hub_shared_snapshot") return route.continue();
    const response=await route.fetch(); held=true; await gate; await route.fulfill({response}).catch(()=>{});
  });
  try {
    await page.locator("#shared-admin-review-conflict").click(); await expect.poll(()=>held).toBe(true);
    await expect(page.locator('[data-sa-tab="projects"]')).toBeDisabled();
    await page.locator("#web-auth-logout").click(); await expect(page.locator("#management-app")).toBeHidden(); release();
    await page.unroute("**/admin/command");
    await page.goto((await hub.command("hub_web_open")).url);
    await expect(page.locator("#management-app")).toBeVisible();
    await expect(page.locator("#shared-admin-content")).toContainText("別の保存");
    await expect(page.locator("#shared-admin-form")).toHaveCount(0); await expect(page.locator("#shared-administration")).not.toContainText("前の管理画面の入力");
  } finally { release(); await page.unroute("**/admin/command"); }
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
