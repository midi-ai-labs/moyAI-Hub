import {test,expect} from "./fixture.mjs";

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
  const candidate=options.candidates.find(row=>/^(10\.|192\.168\.|172\.(1[6-9]|2\d|3[01])\.)/.test(row.ip));
  test.skip(!candidate,"No local RFC1918 interface: private-LAN read-only preview remains an environment acceptance item");
  await hub.allowPrivateNetworkAddress(candidate.ip);
  const mutations=[];
  page.on("request",request=>{if(request.url().endsWith("/admin/command")) { const value=request.postDataJSON(); if(["hub_firewall_apply","hub_firewall_remove"].includes(value?.command)) mutations.push(value.command); }});
  await page.goto(hub.url); await page.locator('nav a[href="#device-network"]').click();
  await page.locator("#network-ip").fill(candidate.ip); await page.locator("#network-port").fill(String(hub.networkPort)); await page.locator("#network-start").click();
  await expect(page.locator("#network-stop")).toBeVisible(); await hub.observeNetwork();
  await page.locator("#hub-firewall > summary").click();
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
  await info.attach("firewall-private-preview",{body:await page.locator("#hub-firewall").screenshot(),contentType:"image/png"});
  await page.locator("#hub-firewall-addresses").fill(`${candidate.ip},10.0.0.2`);
  await page.locator("#network-refresh").click(); await expect(page.locator("#network-refresh")).toBeEnabled();
  await expect(page.locator("#hub-firewall-preview")).toBeHidden(); await expect(page.locator("#hub-firewall-apply")).toBeDisabled();
  expect(mutations).toEqual([]);
  await page.locator("#network-stop").click(); await expect(page.locator("#network-start")).toBeVisible();
});
