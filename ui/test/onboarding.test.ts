import test from "node:test";
import assert from "node:assert/strict";
import { projectPreparation } from "../src/onboarding_state.ts";
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
