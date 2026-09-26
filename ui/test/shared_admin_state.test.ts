import assert from "node:assert/strict";
import test from "node:test";
import { adminRequest, adminTabs, environmentDefaults, projectPCSelection } from "../src/shared_admin_state.ts";

test("project form with no PC checkboxes clears both previous purposes before command construction", () => {
  const draft={id:"project",label:"PCの再登録待ち",controller_device_ids:["deleted-pc"],runner_device_ids:["deleted-pc"]};
  const form=new FormData();
  Object.assign(draft,projectPCSelection(form));
  assert.deepEqual(adminRequest("save_project",draft),{kind:"save_project",id:"project",label:"PCの再登録待ち",archived:false,controller_device_ids:[],runner_device_ids:[]});
  form.append("controller_device_ids","WinA"); form.append("runner_device_ids","WinB");
  assert.deepEqual(projectPCSelection(form),{controller_device_ids:["WinA"],runner_device_ids:["WinB"]});
});

test("a project saves PC purposes without changing existing people and group roles", () => {
  const request=adminRequest("save_project",{id:"project-one",label:"  解析  ","participant:alice":"contributor","participant:bob":"",controller_device_ids:["WinA","WinB","WinA"],runner_device_ids:["WinB","WinC"],template_id:"ignored",capacity:128});
  assert.deepEqual(request,{kind:"save_project",id:"project-one",label:"解析",archived:false,controller_device_ids:["WinA","WinB"],runner_device_ids:["WinB","WinC"]});
  assert.equal("members" in request, false);
  assert.deepEqual(adminTabs.map(([id])=>id),["projects","maintenance","storage"]);
});

test("an environment reuses the selected Runner's one existing pool and exact capacity without granting projects", () => {
  const environment = { id: "a", label: "解析", runner_id: "runner-a", resource_id: "shared-pc", enabled: false, project_ids: ["private"] };
  const snapshot = { environments: [environment, { ...environment, id: "b" }], resources: [{ id: "shared-pc", capacity: 3 }] };
  const defaults = environmentDefaults(snapshot, "runner-a", "new-pool");
  assert.deepEqual(defaults, { runner_id: "runner-a", resource_id: "shared-pc", capacity: "3", template_id: "" });
  const draft = { ...defaults, id: "new-environment", label: "新しい解析", template_id: "approved-template" };
  assert.throws(() => adminRequest("create_environment", draft), "publication must still be explicitly selected");
  assert.deepEqual(adminRequest("create_environment", { ...draft, project_ids: ["chosen"] }), {
    kind: "create_environment", id: "new-environment", label: "新しい解析", runner_id: "runner-a", resource_id: "shared-pc", capacity: 3, template_id: "approved-template", project_ids: ["chosen"],
  });
  assert.equal(snapshot.resources[0].capacity, 3);
});

test("ambiguous or missing existing pools require a choice; a new Runner begins with one new slot", () => {
  const environment = { id: "a", label: "解析", runner_id: "runner-a", resource_id: "pool-a", enabled: true, project_ids: [] };
  const snapshot = { environments: [environment, { ...environment, id: "b", resource_id: "pool-b" }], resources: [{ id: "pool-a", capacity: 4 }, { id: "pool-b", capacity: 1 }] };
  assert.equal(environmentDefaults(snapshot, "runner-a", "new-pool").resource_id, "");
  assert.deepEqual(environmentDefaults(snapshot, "runner-new", "new-pool"), { runner_id: "runner-new", resource_id: "new-pool", capacity: "1", template_id: "" });
  assert.equal(environmentDefaults({ environments: [environment], resources: [] }, "runner-a", "new-pool").resource_id, "", "missing saved pool is not replaced with a new resource");
});
test("provision retry keeps the displayed attempt generation and retention requires a preview", () => {
  assert.deepEqual(adminRequest("request_provision", { environment_id: "env-a", template_id: "safe", generation: 4 }), { kind: "request_provision", environment_id: "env-a", template_id: "safe", generation: 4 });
  assert.throws(() => adminRequest("prune_shared_data", { before_ms: 1234 }));
  assert.deepEqual(adminRequest("prune_shared_data", { before_ms: 1234, preview_confirmed: true }), { kind: "prune_shared_data", before_ms: 1234 });
});

test("human groups and project roles produce explicit membership commands", () => {
  assert.deepEqual(adminRequest("group_member", { group_id: "research", user_id: "alice", member: false }),
    { kind: "group_member", group_id: "research", user_id: "alice", member: false });
  assert.deepEqual(adminRequest("group_project_role", { group_id: "research", project_id: "alpha", role: "" }),
    { kind: "group_project_role", group_id: "research", project_id: "alpha", role: null });
  assert.throws(() => adminRequest("project_member", { project_id: "alpha", user_id: "alice", role: "administrator" }));
});

test("environment registration contains shared identity and only selected projects", () => {
  assert.deepEqual(adminRequest("create_environment", { id: "solver", label: "解析", runner_id: "WinE", resource_id: "shared-license", capacity: "2", project_ids: ["alpha", "beta"], directory: "untrusted-path" }),
    { kind: "create_environment", id: "solver", label: "解析", runner_id: "WinE", resource_id: "shared-license", capacity: 2, project_ids: ["alpha", "beta"] });
  for (const value of ["0", "1.5", "129", "1e2", "2suffix"]) assert.throws(() => adminRequest("set_resource", { id: "resource", capacity: value }));
});

test("execution PC capabilities use one exact environment and a bounded line list", () => {
  assert.deepEqual(adminRequest("set_environment_capabilities", { environment_id: "winb-work", names: "Flask\n  Gurobi  \nFlask\n" }),
    { kind: "set_environment_capabilities", environment_id: "winb-work", names: ["Flask", "Gurobi"] });
  assert.deepEqual(adminRequest("set_environment_capabilities", { environment_id: "winb-work", names: "" }),
    { kind: "set_environment_capabilities", environment_id: "winb-work", names: [] });
  assert.throws(() => adminRequest("set_environment_capabilities", { environment_id: "winb-work", names: "x".repeat(129) }));
  assert.throws(() => adminRequest("set_environment_capabilities", { environment_id: "", names: "Flask" }));
});

test("device principal assignment uses exact device and user identities with no credentials", () => {
  assert.deepEqual(adminRequest("bind_device_principal", { device_id: "pc-a", user_id: "alice", password: "ignored" }),
    { kind: "bind_device_principal", device_id: "pc-a", user_id: "alice" });
  assert.deepEqual(adminRequest("bind_device_principal", { device_id: "pc-b", user_id: "", display_name: "WinB" }),
    { kind: "bind_device_principal", device_id: "pc-b", user_id: null, display_name: "WinB" });
  assert.throws(() => adminRequest("bind_device_principal", { user_id: "alice" }));
});

test("uncertain effect settlement requires the exact attempt generation and explicit confirmation", () => {
  const draft = { attempt_id: "attempt", generation: 4, reason: "現地でプロセス終了を確認", acknowledge_stopped: false };
  assert.throws(() => adminRequest("settle_attempt", draft));
  assert.deepEqual(adminRequest("settle_attempt", { ...draft, acknowledge_stopped: true }),
    { kind: "settle_attempt", attempt_id: "attempt", generation: 4, reason: draft.reason, acknowledge_stopped: true });
});
