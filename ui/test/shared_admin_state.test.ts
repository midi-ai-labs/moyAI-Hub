import assert from "node:assert/strict";
import test from "node:test";
import { adminRequest, adminTabs, projectPCSelection } from "../src/shared_admin_state.ts";

test("project form with no PC checkboxes clears both previous purposes before command construction", () => {
  const draft={id:"project",label:"PCの再登録待ち",controller_device_ids:["deleted-pc"],runner_device_ids:["deleted-pc"]};
  const form=new FormData();
  Object.assign(draft,projectPCSelection(form));
  assert.deepEqual(adminRequest("save_project",draft),{kind:"save_project",id:"project",label:"PCの再登録待ち",overview:"",archived:false,controller_device_ids:[],runner_device_ids:[]});
  form.append("controller_device_ids","WinA"); form.append("runner_device_ids","WinB");
  assert.deepEqual(projectPCSelection(form),{controller_device_ids:["WinA"],runner_device_ids:["WinB"]});
});

test("a project saves PC purposes without changing existing people and group roles", () => {
  const request=adminRequest("save_project",{id:"project-one",label:"  解析  ","participant:alice":"contributor","participant:bob":"",controller_device_ids:["WinA","WinB","WinA"],runner_device_ids:["WinB","WinC"],template_id:"ignored",capacity:128});
  assert.deepEqual(request,{kind:"save_project",id:"project-one",label:"解析",overview:"",archived:false,controller_device_ids:["WinA","WinB"],runner_device_ids:["WinB","WinC"]});
  assert.equal("members" in request, false);
  assert.deepEqual(adminTabs.map(([id])=>id),["projects","maintenance","storage"]);
});

test("project overview preserves interior lines and rejects UTF-8 byte overflow", () => {
  const draft={id:"project",label:"解析",controller_device_ids:[],runner_device_ids:[]};
  assert.equal(adminRequest("save_project",{...draft,overview:"  WinB: Worker\n  WinC: DB  \n"}).overview,"WinB: Worker\n  WinC: DB");
  const boundary="あ".repeat(2730)+"ab";
  assert.equal(new TextEncoder().encode(boundary).length,8192);
  assert.equal(adminRequest("save_project",{...draft,overview:boundary}).overview,boundary);
  assert.throws(()=>adminRequest("save_project",{...draft,overview:boundary+"x"}));
});

test("project folder retry keeps the selected PC and retention requires a preview", () => {
  assert.deepEqual(adminRequest("retry_project_device", { project_id: "project-a", device_id: "pc-b" }), { kind: "retry_project_device", project_id: "project-a", device_id: "pc-b" });
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
