import { test, expect } from "./fixture.mjs";
import { createDeviceParticipant } from "./device-fixture.mjs";

test("first setup starts with PC and project choices without user or password chores",async({page,hub},info)=>{
  await page.goto(hub.url);
  await expect(page.locator("#onboarding-steps")).toContainText("最初のプロジェクト");
  await expect(page.locator("#onboarding-new-user")).toHaveCount(0);
  await expect(page.locator("#onboarding-steps").getByRole("link",{name:"プロジェクトのPC用途を確認",exact:true})).toHaveCount(0);
  await expect(page.locator("#onboarding-steps")).toContainText("チームの仕事にはHubに登録したAIが必要です");
  await expect(page.locator("#team-onboarding")).not.toContainText("PCに直接設定したAIなら不要");
  await page.locator("#onboarding-new-project").click();
  await expect(page.locator('#shared-admin-editor input[type=password], #shared-admin-editor [name^="participant:"]')).toHaveCount(0);
  await info.attach("project-without-person-setup",{body:await page.locator("#shared-administration").screenshot(),contentType:"image/png"});
});

async function project(page,label) {
  await page.locator("#onboarding-new-project").click();
  await page.locator("#shared-admin-label").fill(label);
  await page.locator("#shared-admin-save").click();
  await expect(page.locator("#shared-admin-form")).toHaveCount(0);
  return page.locator(".shared-admin-row").filter({has:page.getByRole("heading",{name:label,exact:true})}).locator('[data-sa-operation="save_project"]').getAttribute("data-sa-id");
}
async function start(page,hub) {
  await page.goto(hub.url);
  await expect(page.locator("#onboarding-steps")).toContainText("最初のプロジェクト");
  await page.locator("#network-ip").fill("127.0.0.1"); await page.locator("#network-port").fill(String(hub.networkPort));
  await page.locator("#network-start").click(); await expect(page.locator("#network-stop")).toBeVisible();
}

test("PC-only admission enables project purposes only while a project is selected",async({page,hub},info)=>{
  await start(page,hub);
  await expect(page.locator("#onboarding-next-title")).toHaveText("最初のプロジェクトを作ってください");
  await expect(page.locator("#team-onboarding").getByRole("button",{name:"プロジェクトを作る",exact:true})).toHaveCount(1);
  await expect(page.locator("#onboarding-steps [data-onboarding-edit]")).toHaveCount(0);
  const purposes=page.locator("#onboarding-steps").getByRole("link",{name:"プロジェクトのPC用途を確認",exact:true});
  await expect(purposes).toHaveCount(0);
  await info.attach("first-project-single-entry",{body:await page.locator("#team-onboarding").screenshot(),contentType:"image/png"});
  const projectId=await project(page,"用途を選ぶプロジェクト");
  await expect(purposes).toBeVisible();
  await purposes.click();
  await expect(page).toHaveURL(/#shared-administration$/);
  await expect(page.locator("#shared-admin-content").getByRole("heading",{name:"用途を選ぶプロジェクト",exact:true})).toBeVisible();
  const participant=await createDeviceParticipant(await hub.observeNetwork(),"用途を確認するPC");
  await page.locator("#network-clients-refresh").click();
  await page.locator(`[data-id="request:${participant.requestId}"] button[data-network-action]`).click();
  const control=page.locator("#join-project-control"),execute=page.locator("#join-project-execute");
  await expect(control).toBeDisabled(); await expect(execute).toBeDisabled();
  await expect(page.locator("#join-project-save")).toHaveText("参加を許可");
  await page.locator("#join-project-project").selectOption(projectId);
  await expect(control).toBeEnabled(); await expect(execute).toBeEnabled();
  await control.check(); await execute.check();
  await expect(page.locator("#join-project-save")).toHaveText("参加と用途を許可");
  await page.locator("#join-project-project").selectOption("");
  await expect(control).toBeDisabled(); await expect(execute).toBeDisabled();
  await expect(control).not.toBeChecked(); await expect(execute).not.toBeChecked();
  await info.attach("pc-only-admission",{body:await page.locator("#join-project-dialog").screenshot(),contentType:"image/png"});
  await page.locator("#join-project-save").click();
  await expect(page.locator("#join-project-dialog")).not.toBeVisible();
  expect((await participant.collectApproval()).status).toBe("approved");
  const session=await (await page.request.get(new URL("/admin/session",hub.url).href)).json();
  const response=await page.request.post(new URL("/admin/command",hub.url).href,{headers:{Origin:new URL(hub.url).origin,"X-Moyai-Csrf":session.csrf,"X-Moyai-Session":session.authentication.session_id},data:{command:"hub_shared_snapshot",args:{}}});
  expect(response.ok()).toBe(true);
  const shared=(await response.json()).result;
  expect(shared.project_devices.filter(row=>row.project_id===projectId)).toEqual([]);
});

test("connection setup guides the first PC through start permission file and approval without claiming reachability",async({page,hub},info)=>{
  await page.goto(hub.url); await page.locator('nav a[href="#device-network"]').click();
  await expect(page.locator("#network-next-title")).toContainText("1.");
  await expect(page.locator("#network-save-config")).toBeDisabled();
  await expect(page.locator("#network-settings-form")).toBeHidden();
  await expect(page.locator("#network-gateway-settings > summary")).toBeVisible();
  await expect(page.locator("#network-settings-form")).toContainText("通常は変更・保存せず");
  const order=await page.locator("#device-network .network-setup-grid > section").evaluateAll(rows=>rows.map(row=>row.id));
  expect(order).toEqual(["network-step-start","network-step-firewall","network-step-file","network-step-confirm"]);
  await expect(page.locator("#network-step-file h3")).toHaveText("3. 接続ファイルの配布");
  expect(await page.locator("#network-gateway-port").evaluate(field=>Boolean(field.compareDocumentPosition(document.querySelector("#network-start"))&Node.DOCUMENT_POSITION_FOLLOWING))).toBe(true);
  // Read geometry before fill/click/element screenshots can scroll to the start control.
  await expect.poll(()=>page.locator("#network-start").evaluate(button=>{
    const box=button.getBoundingClientRect();
    return box.width>0 && box.height>0 && box.top>=0 && box.bottom<=innerHeight && box.left>=0 && box.right<=innerWidth;
  })).toBe(true);
  await info.attach("network-start-without-scrolling",{body:await page.screenshot(),contentType:"image/png"});
  await info.attach("network-first-connection-steps",{body:await page.locator("#network-step-start").screenshot(),contentType:"image/png"});
  // Loopback is an isolated protocol fixture: no Windows rule, UAC, or LAN reachability is exercised.
  await page.locator("#network-ip").fill("127.0.0.1"); await page.locator("#network-port").fill(String(hub.networkPort));
  await page.locator("#network-start").click(); await expect(page.locator("#network-stop")).toBeVisible();
  await expect(page.locator("#network-next-title")).toHaveText("3. 接続ファイルの配布");
  await expect(page.locator("#network-firewall-prerequisite")).toContainText("現在のHubとGatewayのポート");
  await expect(page.locator("#network-save-config")).toBeEnabled();
  const downloadEvent=page.waitForEvent("download"); await page.locator("#network-save-config").click();
  expect((await downloadEvent).suggestedFilename()).toBe("hub-config.moyai-join");
  const participant=await createDeviceParticipant(await hub.observeNetwork(),"最初に試す PC");
  await page.locator("#network-refresh").click(); await expect(page.locator("#network-next-title")).toContainText("参加申請が1台");
  await page.locator("#network-next-link").click();
  await page.locator(`[data-id="request:${participant.requestId}"] button[data-network-action]`).click();
  await page.locator("#join-project-save").click(); await expect(page.locator("#join-project-dialog")).not.toBeVisible();
  expect((await participant.collectApproval()).status).toBe("approved"); await participant.presence();
  await page.locator("#network-clients-refresh").click();
  await expect(page.locator("#network-next-title")).toHaveText("4. 参加したPCで接続を診断");
  await expect(page.locator("#network-next-detail")).toContainText("Desktopで接続を診断");
  await expect(page.locator("#network-step-confirm")).toContainText("仕事を実行するPCでは");
  await info.attach("network-next-after-first-approval",{body:await page.locator(".network-next-action").screenshot(),contentType:"image/png"});
  await page.locator("#network-stop").click(); await expect(page.locator("#network-next-title")).toContainText("1.");
});

test("team preparation registers and names an exact PC and enables project access without a human login",async({page,hub},info)=>{
  await start(page,hub);
  const id=await project(page,"初回の業務");
  const participant=await createDeviceParticipant(await hub.observeNetwork(),"準備する PC");
  await page.locator("#network-clients-refresh").click();
  await page.locator(`[data-id="request:${participant.requestId}"] button[data-network-action]`).click();
  await page.locator("#join-project-project").selectOption(id);
  await page.locator("#join-project-control").check(); await page.locator("#join-project-execute").check();
  await expect(page.locator("#join-project-dialog")).toContainText("fixture\\operator");
  await info.attach("join-and-purpose",{body:await page.locator("#join-project-dialog").screenshot(),contentType:"image/png"});
  await page.locator("#join-project-save").click(); await expect(page.locator("#join-project-dialog")).not.toBeVisible();
  const approval=await participant.collectApproval(); expect(approval.status).toBe("approved");
  expect((await participant.sharedDeviceProjects()).projects.find(p=>p.id===id)).toMatchObject({can_control:true,can_execute:true,preparation_state:"waiting_local_folder",environment_id:expect.any(String)});
  const session=await participant.sharedDeviceSession();
  expect(session.principal.administrator).toBe(false);
  expect((await participant.sharedCall("projects")).some(project=>project.id===id)).toBe(true);
  await page.locator("#shared-admin-refresh").click();
  await expect(page.locator("#onboarding-steps")).toContainText("このPCでプロジェクトの作業フォルダーを選んでください");
  await expect(page.locator("#onboarding-steps")).not.toContainText("waiting_local_folder");
  await expect(page.locator("#onboarding-next-title")).toHaveText("実行PCでプロジェクトの作業フォルダーを選んでください");
  await expect(page.locator("#onboarding-next")).toContainText("2. プロジェクトの作業フォルダー");
  await info.attach("onboarding-local-folder-selection",{body:await page.locator("#team-onboarding").screenshot(),contentType:"image/png"});
  await page.reload(); await expect(page.locator("#onboarding-project")).toHaveValue(id);
  await expect(page.locator("#onboarding-steps")).toContainText("Desktopからサンプルを依頼し、回答と成果ファイルを開いてください。");
  // A prepared folder is not enough to run a shared job without a Hub model.
  // The participant simulates only the authenticated local-folder receipt.
  const [preparation]=await participant.sharedProvisioning();
  expect(preparation.template_id).toBe("local-folder");
  await participant.sharedProvisionReceipt({environment_id:preparation.environment_id,template_id:preparation.template_id,generation:preparation.generation,success:true,error:null});
  await page.locator("#shared-admin-refresh").click();
  await expect(page.locator("#onboarding-next-title")).toHaveText("チームで使うAIをHubに登録してください");
  await expect(page.locator('#onboarding-next a[href="#models"]')).toHaveText("共有AIを登録する");
  await page.locator("#onboarding-first-job > summary").click();
  await expect(page.locator("#onboarding-first-job")).toContainText("通常のチャット欄");
  await expect(page.locator("#onboarding-first-job")).toContainText("10・20・30の件数と合計");
  await expect(page.locator("#onboarding-first-job")).not.toContainText("サンプルの依頼を入力");
  await info.attach("team-preparation",{body:await page.locator("#team-onboarding").screenshot(),contentType:"image/png"});
});
test("approval survives a concurrent project edit and retries only project purposes for the same registered PC",async({page,context,hub})=>{
  await start(page,hub); const id=await project(page,"途中再開");
  const participant=await createDeviceParticipant(await hub.observeNetwork(),"部分保存 PC");
  await page.locator("#network-clients-refresh").click();
  await page.locator(`[data-id="request:${participant.requestId}"] button[data-network-action]`).click();
  await page.locator("#join-project-project").selectOption(id); await page.locator("#join-project-execute").check();
  const other=await context.newPage(); await other.goto(hub.url);
  await expect(other.locator("#onboarding-steps")).toContainText("途中再開");
  await project(other,"並行した準備");
  await page.locator("#join-project-save").click(); await expect(page.locator("#join-project-error")).toContainText("PCは登録済み");
  expect((await hub.observeNetwork()).devices).toHaveLength(1);
  await page.locator("#join-project-save").click(); await expect(page.locator("#join-project-dialog")).not.toBeVisible();
  await participant.collectApproval();
  expect((await participant.sharedDeviceProjects()).projects.find(p=>p.id===id)).toMatchObject({can_control:false,can_execute:true});
  expect((await hub.observeNetwork()).devices).toHaveLength(1);
  await other.close();
});
