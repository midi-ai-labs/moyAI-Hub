export type ProjectRole = "reader" | "contributor" | "manager";
/** Shared project preparation copy, including persisted pre-folder-selection states. */
export function projectPreparationLabel(value: string): string {
  return ({
    waiting_setup: "このPCの実行設定待ち",
    waiting_local_folder: "このPCでプロジェクトの作業フォルダーを選んでください",
    pending: "作業フォルダーを準備中",
    ready: "作業フォルダーの準備完了",
    failed: "作業フォルダーを利用できません。実行PCの設定を確認してください",
    not_selected: "実行しない",
  } as Record<string, string>)[value] ?? "準備状態を確認できません。最新情報を取得してください";
}
export interface SharedAdminUser { user_id: string; username: string; display_name: string; administrator: boolean; disabled: boolean; setup_pending?: boolean }
export interface SharedAdminSnapshot {
  revision: string;
  users: SharedAdminUser[];
  device_principals?: { device_id: string; user_id: string; display_name: string }[];
  groups: { id: string; label: string; disabled: boolean }[];
  group_memberships: { group_id: string; user_id: string }[];
  projects: { id: string; label: string; overview?: string; archived: boolean; legacy_devices_unrestricted?: boolean }[];
  project_devices?: { project_id: string; device_id: string; can_control: boolean; can_execute: boolean; environment_id: string | null; preparation_state: string; error: string | null }[];
  project_memberships: { project_id: string; user_id: string; role: ProjectRole }[];
  group_project_roles: { project_id: string; group_id: string; role: ProjectRole }[];
  environments: { id: string; label: string; resource_id: string; runner_id: string; enabled: boolean; project_ids: string[]; capabilities?: { name: string; source: "declared"; observed_at_ms: null }[] }[];
  resources: { id: string; capacity: number }[];
  templates: { runner_id: string; id: string; label: string }[];
  provisioning: { environment_id: string; runner_id: string; template_id: string; generation: number; state: string; error: string | null }[];
  storage: { retained_bytes: number; asset_count: number; purged_asset_count: number; limit_bytes: number };
  maintenance: { attempt_id: string; generation: number; job_id: string; runner_id: string; state: string; uncertainty_reason: string | null; updated_at_ms: number }[];
  maintenance_next_before: string | null;
  maintenance_total: number;
}
export type SharedAdminTab = "users" | "projects" | "maintenance" | "storage";
export const adminTabs: [SharedAdminTab, string][] = [
  ["projects", "プロジェクト"], ["maintenance", "停止状態の確認"], ["storage", "保存と保守"],
];
export type AdminOperation = "update_user" | "delete_user" | "bind_device_principal"
  | "save_project" | "retry_project_device"
  | "group_member" | "delete_project" | "project_member" | "group_project_role"
  | "set_environment_capabilities" | "settle_attempt" | "prune_shared_data";
export type AdminDraft = Record<string, string | boolean | string[] | number>;
export function projectPCSelection(data: Pick<FormData, "getAll">): Pick<AdminDraft, "controller_device_ids" | "runner_device_ids"> {
  return { controller_device_ids: data.getAll("controller_device_ids").map(String), runner_device_ids: data.getAll("runner_device_ids").map(String) };
}
export const operationLabels: Record<AdminOperation, string> = {
  bind_device_principal: "このPCの利用者を設定",
  save_project: "プロジェクトを保存", retry_project_device: "実行PCのフォルダー作成を再試行",
  update_user: "利用者を編集", delete_user: "利用者を削除",
  group_member: "グループの所属を設定", delete_project: "プロジェクトを削除",
  project_member: "利用者の役割を設定", group_project_role: "グループの役割を設定",
  set_environment_capabilities: "このPCで使えるものを設定", settle_attempt: "停止を確認して枠を解放",
  prune_shared_data: "保存データを整理",
};
const text = (draft: AdminDraft, key: string) => String(draft[key] ?? "").trim();
const flag = (draft: AdminDraft, key: string) => draft[key] === true;
function required(draft: AdminDraft, key: string): string {
  const value = text(draft, key);
  if (!value) throw "必須項目を入力してください。";
  return value;
}
function role(draft: AdminDraft): ProjectRole | null {
  const value = text(draft, "role");
  if (!value) return null;
  if (!["reader", "contributor", "manager"].includes(value)) throw "役割を選択してください。";
  return value as ProjectRole;
}
export function adminRequest(kind: AdminOperation, draft: AdminDraft): Record<string, unknown> {
  const id = () => required(draft, "id");
  const user_id = () => required(draft, "user_id");
  switch (kind) {
    case "bind_device_principal": return { kind, device_id: required(draft, "device_id"), user_id: text(draft, "user_id") || null, ...(text(draft, "display_name") ? { display_name: text(draft, "display_name") } : {}) };
    case "save_project": {
      const selected = (key:string) => Array.isArray(draft[key]) ? [...new Set(draft[key] as string[])] : [];
      const overview = text(draft, "overview");
      if (new TextEncoder().encode(overview).length > 8 * 1024) throw "プロジェクト概要はUTF-8で8 KiB以内にしてください。";
      // This editor owns PC purposes, not the preserved individual/group roles.
      return { kind,id:id(),label:required(draft,"label"),overview,archived:flag(draft,"archived"),controller_device_ids:selected("controller_device_ids"),runner_device_ids:selected("runner_device_ids") };
    }
    case "retry_project_device": return {kind,project_id:required(draft,"project_id"),device_id:required(draft,"device_id")};
    case "update_user": return { kind, user_id: user_id(), display_name: required(draft, "display_name"), administrator: flag(draft, "administrator"), disabled: flag(draft, "disabled") };
    case "delete_user": return { kind, user_id: user_id() };
    case "delete_project": return { kind, id: id() };
    case "group_member": return { kind, group_id: required(draft, "group_id"), user_id: user_id(), member: flag(draft, "member") };
    case "project_member": return { kind, project_id: required(draft, "project_id"), user_id: user_id(), role: role(draft) };
    case "group_project_role": return { kind, project_id: required(draft, "project_id"), group_id: required(draft, "group_id"), role: role(draft) };
    case "set_environment_capabilities": {
      const names = [...new Set(String(draft.names ?? "").split(/\r?\n/).map(name => name.trim()).filter(Boolean))];
      if (names.length > 32 || names.some(name => new TextEncoder().encode(name).length > 128)) throw "使えるものは32件まで、各行128バイト以内で入力してください。";
      return { kind, environment_id: required(draft, "environment_id"), names };
    }
    case "prune_shared_data": {
      const before_ms = Number(draft.before_ms);
      if (!Number.isSafeInteger(before_ms) || before_ms < 1 || !flag(draft, "preview_confirmed")) throw "保存期間と削除予定の件数を確認してください。";
      return { kind, before_ms };
    }
    case "settle_attempt": {
      if (!flag(draft, "acknowledge_stopped")) throw "対象の処理が停止し、PCやソフトウェアを再利用できることを確認してください。";
      const generation = Number(draft.generation);
      if (!Number.isSafeInteger(generation) || generation < 1) throw "最新の復旧対象を選択してください。";
      return { kind, attempt_id: required(draft, "attempt_id"), generation, acknowledge_stopped: true, reason: required(draft, "reason") };
    }
  }
}
export function roleLabel(role: ProjectRole): string {
  return { reader: "閲覧", contributor: "仕事の依頼・実行", manager: "プロジェクト管理" }[role];
}
