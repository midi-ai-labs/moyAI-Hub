import test from "node:test";
import assert from "node:assert/strict";
import { onboardingNextStep, projectPreparation } from "../src/onboarding_state.ts";
import type { SharedAdminSnapshot } from "../src/shared_admin_state.ts";
import type { NetworkSnapshot } from "../src/network_state.ts";

test("preparation uses the selected project's current people and allowed PCs, without treating folders as AI success", () => {
  const shared = { projects:[{id:"a",label:"A",archived:false},{id:"b",label:"B",archived:false}],
    users:[{user_id:"reader",disabled:false},{user_id:"disabled",disabled:true},{user_id:"writer",disabled:false}],
    project_memberships:[{project_id:"a",user_id:"reader",role:"reader"},{project_id:"a",user_id:"disabled",role:"contributor"},{project_id:"b",user_id:"writer",role:"contributor"}],
    project_devices:[{project_id:"a",device_id:"runner",can_execute:true,can_control:false,preparation_state:"ready"},{project_id:"b",device_id:"controller",can_execute:false,can_control:true,preparation_state:"not_selected"}],
  } as SharedAdminSnapshot;
  const network = { devices:[{device_id:"runner",revoked:false,admission:"allowed"},{device_id:"controller",revoked:false,admission:"allowed"}],join_requests:[] } as unknown as NetworkSnapshot;
  const a = projectPreparation(shared,network,"a");
  assert.equal(a.canSubmit,false); assert.equal(a.controllers.length,0); assert.equal(a.foldersPrepared,1);
  assert.equal("executionSucceeded" in a,false);
  network.devices[0].admission="stopped";
  assert.equal(projectPreparation(shared,network,"a").executors.length,0);
  assert.equal(projectPreparation(shared,network,"b").canSubmit,true);
  shared.users.find(user=>user.user_id==="writer")!.setup_pending=true;
  assert.equal(projectPreparation(shared,network,"b").canSubmit,false);
});

function setup() {
  const shared: SharedAdminSnapshot = {
    revision: "1", users: [{ user_id: "writer", username: "requester", display_name: "依頼する人", administrator: false, disabled: false }],
    groups: [], group_memberships: [], group_project_roles: [],
    projects: [{ id: "project", label: "集計", archived: false }],
    project_memberships: [{ project_id: "project", user_id: "writer", role: "contributor" }],
    project_devices: [
      { project_id: "project", device_id: "a", can_control: true, can_execute: false, environment_id: null, preparation_state: "not_selected", error: null },
      { project_id: "project", device_id: "b", can_control: false, can_execute: true, environment_id: "env", preparation_state: "ready", error: null },
    ],
    environments: [], resources: [], templates: [], provisioning: [],
    storage: { retained_bytes: 0, asset_count: 0, purged_asset_count: 0, limit_bytes: 1024 },
    maintenance: [], maintenance_next_before: null, maintenance_total: 0,
  };
  const network: NetworkSnapshot = {
    revision: "1", hub_id: "hub", ca_certificate_pem: "", ca_sha256: "", gateway_port: 0,
    hosting: { configured: true, bind: "127.0.0.1:9081", advertise_ip: "127.0.0.1", error: null },
    server: { running: true, bind: "https://127.0.0.1:9081", gateway_bind: null, certificate: null },
    devices: ["a", "b"].map(id => ({ device_id: id, label: id === "a" ? "WinA" : "WinB", groups: [], revoked: false, admission: "allowed", certificate_sha256: "", expires_at_ms: 0, last_seen_ms: null, observed_ip: null, online: true, publications: [] })),
    join_requests: [], groups: [], rules: [],
  };
  return { shared, network, next: () => onboardingNextStep(shared, network, "project") };
}

test("next action waits for observed network and directs the Hub administrator to the existing network owner", () => {
  const { shared, network, next } = setup();
  assert.equal(onboardingNextStep(shared, null, "project").kind, "checking");
  network.server.running = false;
  const step = next();
  assert.equal(step.kind, "network");
  assert.equal(step.owner, "Hub管理者");
  assert.deepEqual(step.action, { kind: "link", href: "#network-step-start", label: "接続先を設定する" });
});

test("next action assigns project creation and missing writable membership to the administrator", () => {
  const { shared, network, next } = setup();
  assert.equal(onboardingNextStep(shared, network, "missing").kind, "project");
  shared.project_memberships[0].role = "reader";
  assert.equal(next().kind, "participants");
  shared.project_memberships[0].role = "manager";
  shared.users[0].disabled = true;
  assert.equal(next().kind, "participants");
  shared.users = [];
  assert.equal(next().kind, "participants");
  shared.projects[0].archived = true;
  assert.equal(next().kind, "project");
});

test("person setup names the person and their assigned control PC without requiring every participant to finish", () => {
  const { shared, next } = setup();
  shared.users[0].setup_pending = true;
  const step = next();
  assert.equal(step.kind, "person_setup");
  assert.equal(step.owner, "依頼する人");
  assert.equal(step.location, "WinA のDesktop");
  shared.users.push({ user_id: "second", username: "second", display_name: "設定済みの人", administrator: false, disabled: false });
  shared.project_memberships.push({ project_id: "project", user_id: "second", role: "manager" });
  assert.equal(next().kind, "sample");
  assert.equal(next().owner, "設定済みの人（仕事を依頼する人）");
});

test("stopped, revoked, and missing PCs cannot advance the handoff to the sample", () => {
  const { shared, network, next } = setup();
  network.devices[0].admission = "stopped";
  assert.equal(next().kind, "controllers");
  network.devices[0].admission = "allowed";
  network.devices[1].revoked = true;
  assert.equal(next().kind, "executors");
  network.devices[1].revoked = false;
  network.devices[1].admission = "revoked";
  assert.equal(next().kind, "executors");
  shared.project_devices![1].device_id = "missing";
  assert.equal(next().kind, "executors");
});

test("executor preparation names the PC and distinguishes local setup, in-progress work, and failed creation", () => {
  const { shared, next } = setup();
  shared.project_devices![1].preparation_state = "waiting_setup";
  assert.equal(next().kind, "execution_setup");
  assert.equal(next().location, "WinB のDesktop");
  assert.equal(next().owner, "実行PCの担当者");
  shared.project_devices![1].preparation_state = "pending";
  assert.equal(next().kind, "preparing");
  shared.project_devices![1].preparation_state = "failed";
  assert.equal(next().kind, "preparation_failed");
  assert.equal(next().owner, "実行PCの担当者 → Hub管理者");
});

test("one prepared executor enables a real sample while another incomplete executor is reported separately", () => {
  const { shared, network, next } = setup();
  shared.project_devices!.push({ ...shared.project_devices![1], device_id: "c", preparation_state: "failed" });
  network.devices.push({ ...network.devices[1], device_id: "c", label: "WinC" });
  const step = next();
  assert.equal(step.kind, "sample");
  assert.equal(step.location, "WinA のDesktop");
  assert.equal(step.action?.kind, "first");
  assert.match(step.detail, /実行先に WinB/);
  assert.match(step.detail, /WinC は準備が未完了/);
  assert.match(step.detail, /AIの接続・実行結果はこのサンプルで確認/);
  assert.equal("executionSucceeded" in step, false);
  assert.equal(shared.project_devices![2].preparation_state, "failed");
});

test("legacy unrestricted projects ask for review without presenting missing explicit mappings as inability to use", () => {
  const { shared, next } = setup();
  shared.projects[0].legacy_devices_unrestricted = true;
  shared.project_devices = [];
  shared.project_memberships = [];
  assert.equal(next().kind, "legacy");
  assert.equal(next().action?.kind, "project");
  assert.match(next().detail, /利用可否を判定していません/);
});
