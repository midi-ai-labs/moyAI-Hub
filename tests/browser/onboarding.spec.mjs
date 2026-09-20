import { test, expect, administrator } from "./fixture.mjs";
import { createDeviceParticipant } from "./device-fixture.mjs";

test("first-person code is displayed once and does not return after refresh",async({page,hub},info)=>{
  await page.goto(hub.url);
  await expect(page.locator("#onboarding-steps")).toContainText("最初のプロジェクト");
  await page.locator("#onboarding-new-user").click();
  await page.locator("#shared-admin-username").fill("first-person");
  await page.locator("#shared-admin-display_name").fill("初回の利用者");
  await expect(page.locator("#shared-admin-editor input[type=password]")).toHaveCount(0);
  await page.locator("#shared-admin-save").click();
  await expect(page.locator("#shared-admin-setup-code")).toBeVisible();
  await expect(page.locator("#shared-admin-setup-code-value")).toHaveValue(/^[0-9a-f]{64}$/);
  await page.locator("#shared-admin-setup-code-close").click();
  await expect(page.locator("#shared-admin-setup-code-value")).toHaveValue("");
  await page.reload(); await page.locator('[data-sa-tab="users"]').click();
  await expect(page.locator("#shared-admin-content")).toContainText("本人の初回設定待ち");
  await expect(page.locator("#shared-admin-setup-code")).not.toBeVisible();
  await page.locator(".shared-admin-row").filter({has:page.getByRole("heading",{name:"初回の利用者",exact:true})}).locator('[data-sa-operation="update_user"]').click();
  await expect(page.locator('#shared-admin-editor input[name="administrator"]')).toBeDisabled();
  await expect(page.locator("#shared-admin-editor")).toContainText("初回パスワード設定が済んでから");
  await info.attach("person-waiting-for-setup",{body:await page.locator("#shared-administration").screenshot(),contentType:"image/png"});
  await page.locator("#hub-autostart-refresh").evaluate(el=>el.closest("details").open=true);
  await page.locator("#hub-autostart-refresh").click();
  await expect(page.locator("#hub-autostart-status")).not.toContainText("確認しています");
  // Read only: never alter the test operator's real logon registration.
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

test("connection setup guides the first PC through start permission file and approval without claiming reachability",async({page,hub},info)=>{
  await page.goto(hub.url); await page.locator('nav a[href="#device-network"]').click();
  await expect(page.locator("#network-next-title")).toContainText("1.");
  await expect(page.locator("#network-save-config")).toBeDisabled();
  await expect(page.locator("#network-settings-form")).toBeHidden();
  await expect(page.locator("#network-gateway-settings > summary")).toBeVisible();
  await expect(page.locator("#network-settings-form")).toContainText("通常は変更・保存せず");
  const order=await page.locator("#device-network .network-setup-grid > section").evaluateAll(rows=>rows.map(row=>row.id));
  expect(order).toEqual(["network-step-start","network-step-firewall","network-step-file","network-step-confirm"]);
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
  await expect(page.locator("#network-next-title")).toContainText("2.");
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
  await expect(page.locator("#network-step-confirm")).toContainText("HubのAIも使う場合");
  await info.attach("network-next-after-first-approval",{body:await page.locator(".network-next-action").screenshot(),contentType:"image/png"});
  await page.locator("#network-stop").click(); await expect(page.locator("#network-next-title")).toContainText("1.");
});

test("first-person creation keeps its inputs after conflict and issues a code only on explicit save",async({page,context,hub},info)=>{
  await page.goto(hub.url); await page.locator("#onboarding-new-user").click();
  await page.locator("#shared-admin-username").fill("retained-first-person"); await page.locator("#shared-admin-display_name").fill("入力を保持する利用者");
  const other=await context.newPage(); await other.goto(hub.url); await project(other,"並行登録されたプロジェクト");
  await page.locator("#shared-admin-save").click(); await expect(page.locator("#shared-admin-form-error")).toContainText("入力は残っています");
  await page.locator("#shared-admin-review-conflict").click(); await expect(page.locator("#shared-admin-comparison")).toContainText("retained-first-person");
  await expect(page.locator("#shared-admin-setup-code")).toBeHidden();
  await expect(page.locator("#shared-admin-content")).not.toContainText("入力を保持する利用者");
  await info.attach("person-conflict-comparison-before-code",{body:await page.locator("#shared-admin-editor").screenshot(),contentType:"image/png"});
  await page.locator("#shared-admin-accept-comparison").click();
  await expect(page.locator("#shared-admin-username")).toHaveValue("retained-first-person");
  await expect(page.locator("#shared-admin-display_name")).toHaveValue("入力を保持する利用者");
  await expect(page.locator("#shared-admin-setup-code")).toBeHidden();
  await page.locator("#shared-admin-save").click(); await expect(page.locator("#shared-admin-setup-code-value")).toHaveValue(/^[0-9a-f]{64}$/);
  await page.locator("#shared-admin-setup-code-close").click(); await expect(page.locator("#shared-admin-setup-code-value")).toHaveValue("");
});
test("team preparation joins and assigns one exact PC without granting business access to the administrator",async({page,hub},info)=>{
  await start(page,hub);
  const id=await project(page,"初回の業務");
  const participant=await createDeviceParticipant(await hub.observeNetwork(),"準備する PC");
  await page.locator("#network-clients-refresh").click();
  await page.locator(`[data-id="request:${participant.requestId}"] button[data-network-action]`).click();
  await page.locator("#join-project-project").selectOption(id);
  await page.locator("#join-project-control").check(); await page.locator("#join-project-execute").check();
  await info.attach("join-and-purpose",{body:await page.locator("#join-project-dialog").screenshot(),contentType:"image/png"});
  await page.locator("#join-project-save").click(); await expect(page.locator("#join-project-dialog")).not.toBeVisible();
  const approval=await participant.collectApproval(); expect(approval.status).toBe("approved");
  expect((await participant.sharedDeviceProjects()).projects.find(p=>p.id===id)).toMatchObject({can_control:true,can_execute:true,preparation_state:"waiting_setup"});
  await participant.sharedLogin(administrator.username,administrator.password);
  expect(await participant.sharedCall("projects")).toEqual([]);
  await page.locator("#shared-admin-refresh").click();
  await expect(page.locator("#onboarding-steps")).toContainText("実行PCの保存先・権限の確認待ち");
  await page.reload(); await expect(page.locator("#onboarding-project")).toHaveValue(id);
  await expect(page.locator("#onboarding-steps")).toContainText("Desktopからサンプルを依頼し、回答と成果ファイルを開いてください。");
  await page.locator("#onboarding-first-job > summary").click();
  await expect(page.locator("#onboarding-first-job")).toContainText("Desktopの「接続状況とサンプル」");
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
