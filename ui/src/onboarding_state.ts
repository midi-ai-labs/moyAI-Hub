import type { NetworkSnapshot } from "./network_state.ts";
import type { SharedAdminSnapshot } from "./shared_admin_state.ts";

export function projectPreparation(shared: SharedAdminSnapshot, network: NetworkSnapshot | null, id: string) {
  const project = shared.projects.find(p => p.id === id && !p.archived);
  const people = shared.project_memberships.filter(m => m.project_id === id && shared.users.some(u => u.user_id === m.user_id && !u.disabled));
  const assignments = (shared.project_devices ?? []).filter(row => row.project_id === id);
  const current = (device: string) => network?.devices.some(d => d.device_id === device && !d.revoked && d.admission === "allowed");
  const controllers = assignments.filter(row => row.can_control && current(row.device_id));
  const executors = assignments.filter(row => row.can_execute && current(row.device_id));
  return { project, people, controllers, executors,
    canSubmit: people.some(p => p.role !== "reader" && shared.users.some(u => u.user_id === p.user_id && !u.setup_pending)),
    foldersPrepared: executors.filter(row => row.preparation_state === "ready").length,
    pendingRequests: network?.join_requests.filter(row => row.status === "pending").length ?? 0 };
}
