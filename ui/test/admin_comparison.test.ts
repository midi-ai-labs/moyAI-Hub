import assert from "node:assert/strict";
import test from "node:test";
import { compareAdminEdit } from "../src/admin_comparison.ts";
import type { SharedAdminSnapshot } from "../src/shared_admin_state.ts";

const snapshot = (revision = "1"): SharedAdminSnapshot => ({ revision, users: [{user_id:"alice",username:"alice",display_name:"Alice",administrator:false,disabled:false}], groups: [], group_memberships: [], projects: [{id:"project",label:"元の名前",archived:false}], project_devices: [], project_memberships: [], group_project_roles: [], environments: [], resources: [], templates: [], provisioning: [], storage:{retained_bytes:0,asset_count:0,purged_asset_count:0,limit_bytes:100}, maintenance:[],maintenance_next_before:null,maintenance_total:0 });

test("device association compares exact identities and keeps inputs across unrelated changes", () => {
  const initial=snapshot(), latest=snapshot("2");
  latest.device_principals=[{device_id:"pc-a",user_id:"alice",display_name:"Alice"}];
  const draft={device_id:"pc-a",user_id:"",display_name:"WinA"};
  const before=JSON.stringify({initial,latest,draft});
  const review=compareAdminEdit("bind_device_principal",draft,initial,latest,[{device_id:"pc-a",label:"WinA"}]);
  assert.equal(review.revision,"2"); assert.equal(review.blocked,null);
  assert.ok(review.rows.some(row=>row.saved==="Alice（alice）" && row.input==="このPC専用の利用者"));
  assert.equal(JSON.stringify({initial,latest,draft}),before);
});
test("device association rejects a revoked PC or an unavailable existing user", () => {
  const latest=snapshot("2");
  assert.ok(compareAdminEdit("bind_device_principal",{device_id:"pc",user_id:"alice"},snapshot(),latest,[{device_id:"pc",label:"PC",revoked:true}]).blocked);
  for (const state of ["disabled","setup_pending"] as const) {
    latest.users[0][state]=true;
    assert.ok(compareAdminEdit("bind_device_principal",{device_id:"pc",user_id:"alice"},snapshot(),latest,[{device_id:"pc",label:"PC"}]).blocked);
    latest.users[0][state]=false;
  }
});
test("deleted identities stay blocked even when another person has the same name", () => {
  const latest=snapshot("2"); latest.users=[{...latest.users[0],user_id:"replacement"}];
  assert.match(compareAdminEdit("update_user",{user_id:"alice",display_name:"入力"},snapshot(),latest,[]).blocked!,/削除/);
  latest.projects=[{id:"replacement",label:"元の名前",archived:false}];
  assert.match(compareAdminEdit("save_project",{id:"project",label:"入力"},snapshot(),latest,[]).blocked!,/削除/);
});
test("project comparison reveals only changed PC grants and leaves existing people roles untouched", () => {
  const original=snapshot(), latest=snapshot("2"); latest.projects[0].label="別画面の名前";
  latest.project_memberships=[{project_id:"project",user_id:"alice",role:"manager"}];
  latest.project_devices=[{project_id:"project",device_id:"pc",can_control:true,can_execute:false,environment_id:null,preparation_state:"not_selected",error:null}];
  const draft={id:"project",label:"入力中の名前",controller_device_ids:[],runner_device_ids:[]};
  const review=compareAdminEdit("save_project",draft,original,latest,[{device_id:"pc",label:"WinA"}]);
  assert.equal(review.blocked,null);
  assert.equal(review.rows.some(row=>row.label==="Alice（alice）"),false);
  assert.deepEqual(review.rows.find(row=>row.label==="操作に使うPC"),{label:"操作に使うPC",saved:"WinA（pc）",input:"なし"});
  assert.equal(draft.label,"入力中の名前");
});
test("new project identity collisions require a fresh target but people changes are outside the PC editor", () => {
  const original=snapshot(); original.projects=[];
  assert.ok(compareAdminEdit("save_project",{id:"project",label:"入力"},original,snapshot("2"),[]).blocked);
  const latest=snapshot("3"); latest.users=[];
  assert.equal(compareAdminEdit("save_project",{id:"project","participant:alice":"manager"},snapshot(),latest,[]).blocked,null);
});

test("project conflict review shows the latest overview beside the intact multiline draft", () => {
  const original=snapshot(), latest=snapshot("2");
  latest.projects[0].overview="保存済みの概要\nWinC: DB";
  const draft={id:"project",label:"元の名前",overview:"入力中の概要\nWinB: Worker",controller_device_ids:[],runner_device_ids:[]};
  const before=JSON.stringify(draft);
  const review=compareAdminEdit("save_project",draft,original,latest,[]);
  assert.deepEqual(review.rows.find(row=>row.label==="プロジェクト概要"),{label:"プロジェクト概要",saved:latest.projects[0].overview,input:draft.overview});
  assert.equal(JSON.stringify(draft),before);
});

test("selected deleted or revoked PCs cannot continue through a project comparison", () => {
  for (const key of ["controller_device_ids", "runner_device_ids"]) {
    const draft={id:"project",label:"入力",[key]:["removed-pc"]};
    assert.match(compareAdminEdit("save_project",draft,snapshot(),snapshot("2"),[]).blocked!,/削除または失効/);
    assert.ok(compareAdminEdit("save_project",draft,snapshot(),snapshot("2"),[{device_id:"removed-pc",label:"同じPC",revoked:true}]).blocked);
    assert.equal(compareAdminEdit("save_project",draft,snapshot(),snapshot("2"),[{device_id:"removed-pc",label:"同じPC",revoked:false}]).blocked,null);
  }
});
test("destructive and generation-bound commands are not rebased by a generic comparison", () => {
  for (const kind of ["delete_user","delete_project","settle_attempt","retry_project_device","prune_shared_data"] as const) {
    assert.ok(compareAdminEdit(kind,{user_id:"alice",password:"secret",generation:1},snapshot(),snapshot("2"),[]).blocked,kind);
  }
});

test("existing group membership and project role comparisons remain available through users", () => {
  const original = snapshot(), latest = snapshot("2");
  for (const value of [original, latest]) value.groups = [{ id: "research", label: "研究", disabled: false }];
  latest.group_memberships = [{ group_id: "research", user_id: "alice" }];
  latest.group_project_roles = [{ project_id: "project", group_id: "research", role: "reader" }];
  latest.project_memberships = [{ project_id: "project", user_id: "alice", role: "manager" }];
  const cases = [
    ["group_member", { group_id: "research", user_id: "alice", member: false }, "はい", "いいえ"],
    ["group_project_role", { project_id: "project", group_id: "research", role: "contributor" }, "閲覧", "仕事の依頼・実行"],
    ["project_member", { project_id: "project", user_id: "alice", role: "" }, "プロジェクト管理", "参加しない"],
  ] as const;
  for (const [kind, draft, saved, input] of cases) {
    const before = JSON.stringify({ draft, latest });
    const comparison = compareAdminEdit(kind, draft, original, latest, []);
    assert.equal(comparison.blocked, null);
    assert.deepEqual(comparison.rows.map(row => [row.saved, row.input]), [[saved, input]]);
    assert.equal(JSON.stringify({ draft, latest }), before);
    const missing = structuredClone(latest);
    if (kind === "project_member") missing.users = [];
    else missing.groups = [];
    assert.match(compareAdminEdit(kind, draft, original, missing, []).blocked!, /削除/);
  }
});
