import {test,expect} from "./fixture.mjs";
import {readFile} from "node:fs/promises";

test("Firewall preview refuses loopback without permitting an OS mutation",async({page,hub},info)=>{
  const mutations=[];
  page.on("request",request=>{if(request.url().endsWith("/admin/command")) { const value=request.postDataJSON(); if(["hub_firewall_apply","hub_firewall_remove"].includes(value?.command)) mutations.push(value.command); }});
  await page.goto(hub.url); await page.locator('nav a[href="#device-network"]').click();
  await page.locator("#network-ip").fill("127.0.0.1"); await page.locator("#network-port").fill(String(hub.networkPort)); await page.locator("#network-start").click();
  await expect(page.locator("#network-stop")).toBeVisible();
  await page.locator("#hub-firewall > summary").click();
  await page.locator("#hub-firewall-addresses").fill("192.168.10.0/24"); await page.locator("#hub-firewall-private").check();
  await page.locator("#hub-firewall-review").click(); await expect(page.locator("#hub-firewall-status")).toContainText("社内IPv4");
  await expect(page.locator("#hub-firewall-apply")).toBeDisabled(); await expect(page.locator("#hub-firewall-preview")).toBeHidden();
  await info.attach("firewall-loopback-refused",{body:await page.locator("#hub-firewall").screenshot(),contentType:"image/png"});
  expect(mutations).toEqual([]);
  await page.locator("#network-stop").click(); await expect(page.locator("#network-start")).toBeVisible();
});

test("Firewall real local-LAN preview preserves explicit scope and invalidates changed input without UAC",async({page,hub},info)=>{
  const options=await hub.command("hub_hosting_options");
  const privateCandidates=options.candidates.filter(row=>/^(10\.|192\.168\.|172\.(1[6-9]|2\d|3[01])\.)/.test(row.ip));
  const candidate=privateCandidates.find(row=>row.profile==="Private"||row.profile==="DomainAuthenticated") ?? privateCandidates[0];
  test.skip(!candidate,"No local RFC1918 interface: private-LAN read-only preview remains an environment acceptance item");
  await hub.allowPrivateNetworkAddress(candidate.ip);
  const mutations=[];
  page.on("request",request=>{if(request.url().endsWith("/admin/command")) { const value=request.postDataJSON(); if(["hub_firewall_apply","hub_firewall_remove"].includes(value?.command)) mutations.push(value.command); }});
  await page.goto(hub.url); await page.locator('nav a[href="#device-network"]').click();
  await page.locator("#network-ip").fill(candidate.ip); await page.locator("#network-port").fill(String(hub.networkPort)); await page.locator("#network-start").click();
  await expect(page.locator("#network-stop")).toBeVisible(); await hub.observeNetwork();
  const grant=await hub.command("hub_web_open");
  await page.goto(`${grant.url}&section=network-step-firewall`);
  await expect(page).toHaveURL(`${hub.url}#network-step-firewall`);
  await expect(page.locator("#network-step-firewall h3")).toBeInViewport();
  await expect(page.locator("#hub-firewall-health")).toContainText("未登録",{timeout:30000});
  await expect(page.locator("#network-next-title")).toContainText("受信許可");
  await expect(page.locator("#network-save-config")).toBeDisabled();
  await expect(page.locator("#network-copy-config")).toBeDisabled();
  await expect(page.locator("#hub-firewall")).toHaveAttribute("open","");
  await info.attach("firewall-startup-status",{body:await page.locator("#network-step-firewall").screenshot(),contentType:"image/png"});
  const suggestion = page.locator("#hub-firewall-suggestion");
  if (candidate.profile === "Private" || candidate.profile === "DomainAuthenticated") {
    await expect(suggestion).toBeVisible();
    await expect(page.locator("#hub-firewall-addresses")).not.toBeEmpty();
    await expect(page.locator("#hub-firewall-preview")).toBeVisible({timeout:25000});
    await expect(page.locator("#hub-firewall-apply")).toBeDisabled();
    await expect(page.locator("#hub-firewall-export")).toBeDisabled();
  }
  // Open only if the explicit suggestion did not already open the editor.
  if (!await page.locator("#hub-firewall").evaluate(node=>node.open)) await page.locator("#hub-firewall > summary").click();
  await page.locator("#hub-firewall-addresses").fill(candidate.ip); await page.locator("#hub-firewall-private").check();
  await page.locator("#hub-firewall-review").click();
  await expect(page.locator("#hub-firewall-preview")).toBeVisible({timeout:25000});
  await expect(page.locator("#hub-firewall-target")).toContainText(candidate.ip);
  await expect(page.locator("#hub-firewall-target")).toContainText(String(hub.networkPort));
  await expect(page.locator("#hub-firewall-target")).toContainText(String(hub.gatewayPort));
  expect(await page.locator("#hub-firewall-target").evaluate(node=>node.scrollWidth<=node.clientWidth+1)).toBe(true);
  await expect(page.locator("#hub-firewall-previous")).toHaveText("未登録");
  await page.locator("#network-refresh").click(); await expect(page.locator("#network-refresh")).toBeEnabled();
  await expect(page.locator("#hub-firewall-confirm")).not.toBeChecked();
  await expect(page.locator("#hub-firewall-apply")).toBeDisabled();
  await page.locator("#hub-firewall-confirm").check(); await expect(page.locator("#hub-firewall-apply")).toBeEnabled();
  const downloadPromise=page.waitForEvent("download");
  await page.locator("#hub-firewall-export").click();
  const download=await downloadPromise;
  expect(download.suggestedFilename()).toBe("Allow-moyAI-Hub.ps1");
  const script=await readFile(await download.path(),"utf8");
  expect(script).toContain(candidate.ip);expect(script).toContain(String(hub.networkPort));
  expect(script).toContain("CheckOnly");
  await expect(page.locator("#hub-firewall-status")).toContainText("保存だけでは受信許可は変わりません");
  await expect(page.locator("#network-save-config")).toBeDisabled();
  // Administrator-managed policies have an explicit, temporary distribution path.
  await page.locator("#network-external-firewall > summary").click();
  await page.locator("#network-firewall-external-confirm").check();
  await expect(page.locator("#network-save-config")).toBeEnabled();
  await expect(page.locator("#network-distribution-status")).toContainText("まだ確認していません");
  await info.attach("firewall-private-preview",{body:await page.locator("#hub-firewall").screenshot(),contentType:"image/png"});
  await page.locator("#hub-firewall-addresses").fill(`${candidate.ip},10.0.0.2`);
  await page.locator("#network-refresh").click(); await expect(page.locator("#network-refresh")).toBeEnabled();
  await expect(page.locator("#hub-firewall-preview")).toBeHidden(); await expect(page.locator("#hub-firewall-apply")).toBeDisabled();
  expect(mutations).toEqual([]);
  await page.locator("#network-stop").click(); await expect(page.locator("#network-start")).toBeVisible();
  await expect(page.locator("#hub-firewall-health")).toContainText("Hubを開始すると");
  await expect(page.locator("#network-firewall-external-confirm")).not.toBeChecked();
  await expect(page.locator("#network-save-config")).toBeDisabled();
});
