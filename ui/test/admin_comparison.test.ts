import assert from "node:assert/strict";
import test from "node:test";
import { compareAdminEdit } from "../src/admin_comparison.ts";
import type { SharedAdminSnapshot } from "../src/shared_admin_state.ts";

const snapshot = (revision = "1"): SharedAdminSnapshot => ({ revision, users: [{user_id:"alice",username:"alice",display_name:"Alice",administrator:false,disabled:false}], groups: [], group_memberships: [], projects: [{id:"project",label:"元の名前",archived:false}], project_devices: [], project_memberships: [], group_project_roles: [], environments: [], resources: [], templates: [], provisioning: [], storage:{retained_bytes:0,asset_count:0,purged_asset_count:0,limit_bytes:100}, maintenance:[],maintenance_next_before:null,maintenance_total:0 });

test("a new user draft survives unrelated changes without exposing secrets or editing the snapshot", () => {
  const initial=snapshot(), latest=snapshot("2");
  const draft={username:"bob",display_name:"入力した名前",password:"never-render-this-secret",administrator:false};
  const before=JSON.stringify({initial,latest,draft});
  const review=compareAdminEdit("create_user",draft,initial,latest,[]);
  assert.equal(review.revision,"2"); assert.equal(review.blocked,null);
  assert.ok(review.rows.some(row=>row.input==="入力した名前"));
  assert.doesNotMatch(JSON.stringify(review),/never-render-this-secret/);
  assert.equal(JSON.stringify({initial,latest,draft}),before);
});
test("a username collision cannot be accepted as permission to edit the registered person", () => {
  const review=compareAdminEdit("create_user_with_setup_code",{username:"ALICE",display_name:"別人"},snapshot(),snapshot("2"),[]);
  assert.match(review.blocked!,/登録されています/);
});
test("deleted identities stay blocked even when another person has the same name", () => {
  const latest=snapshot("2"); latest.users=[{...latest.users[0],user_id:"replacement"}];
  assert.match(compareAdminEdit("update_user",{user_id:"alice",display_name:"入力"},snapshot(),latest,[]).blocked!,/削除/);
  latest.projects=[{id:"replacement",label:"元の名前",archived:false}];
  assert.match(compareAdminEdit("save_project",{id:"project",label:"入力"},snapshot(),latest,[]).blocked!,/削除/);
});
test("project comparison reveals new participants and PC grants that the retained draft would remove", () => {
  const original=snapshot(), latest=snapshot("2"); latest.projects[0].label="別画面の名前";
  latest.project_memberships=[{project_id:"project",user_id:"alice",role:"manager"}];
  latest.project_devices=[{project_id:"project",device_id:"pc",can_control:true,can_execute:false,environment_id:null,preparation_state:"not_selected",error:null}];
  const draft={id:"project",label:"入力中の名前",controller_device_ids:[],runner_device_ids:[]};
  const review=compareAdminEdit("save_project",draft,original,latest,[{device_id:"pc",label:"WinA"}]);
  assert.equal(review.blocked,null);
  assert.deepEqual(review.rows.find(row=>row.label==="Alice（alice）"),{label:"Alice（alice）",saved:"プロジェクト管理",input:"参加しない"});
  assert.deepEqual(review.rows.find(row=>row.label==="操作に使うPC"),{label:"操作に使うPC",saved:"WinA（pc）",input:"なし"});
  assert.equal(draft.label,"入力中の名前");
});
test("new project identity collisions and removed selected participants require a fresh target", () => {
  const original=snapshot(); original.projects=[];
  assert.ok(compareAdminEdit("save_project",{id:"project",label:"入力"},original,snapshot("2"),[]).blocked);
  const latest=snapshot("3"); latest.users=[];
  assert.ok(compareAdminEdit("save_project",{id:"project","participant:alice":"manager"},snapshot(),latest,[]).blocked);
});
test("pending first-person setup never grants administrator rights during comparison", () => {
  const latest=snapshot("2"); latest.users[0].setup_pending=true;
  assert.ok(compareAdminEdit("update_user",{user_id:"alice",administrator:true},snapshot(),latest,[]).blocked);
  assert.ok(compareAdminEdit("set_password",{user_id:"alice",password:"secret"},snapshot(),latest,[]).blocked);
});

test("selected deleted or revoked PCs cannot continue through a project comparison", () => {
  for (const key of ["controller_device_ids", "runner_device_ids"]) {
    const draft={id:"project",label:"入力",[key]:["removed-pc"]};
    assert.match(compareAdminEdit("save_project",draft,snapshot(),snapshot("2"),[]).blocked!,/削除または失効/);
    assert.ok(compareAdminEdit("save_project",draft,snapshot(),snapshot("2"),[{device_id:"removed-pc",label:"同じPC",revoked:true}]).blocked);
    assert.equal(compareAdminEdit("save_project",draft,snapshot(),snapshot("2"),[{device_id:"removed-pc",label:"同じPC",revoked:false}]).blocked,null);
  }
});
test("destructive, generation-bound and one-time-code commands are not rebased by a generic comparison", () => {
  for (const kind of ["delete_user","settle_attempt","request_provision","prune_shared_data","reissue_setup_code","revoke_setup_code"] as const) {
    assert.ok(compareAdminEdit(kind,{user_id:"alice",password:"secret",generation:1},snapshot(),snapshot("2"),[]).blocked,kind);
  }
});
