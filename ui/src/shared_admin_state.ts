export type ProjectRole = "reader" | "contributor" | "manager";
/** Shared project preparation copy, including persisted pre-folder-selection states. */
export function projectPreparationLabel(value: string): string {
  return ({
    waiting_setup: "このPCの実行設定待ち",
    waiting_local_folder: "このPCでプロジェクトの作業フォルダーを選んでください",
    pending: "作業フォルダーを準備中",
    ready: "作業フォルダーの準備完了（Desktopでサンプルを実行してください）",
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
export type SharedAdminTab = "users" | "groups" | "projects" | "environments" | "resources" | "maintenance" | "storage";
export const adminTabs: [SharedAdminTab, string][] = [
  ["projects", "プロジェクト"], ["maintenance", "停止状態の確認"], ["storage", "保存と保守"],
];
export type AdminOperation = "update_user" | "delete_user" | "bind_device_principal"
  | "save_project" | "retry_project_device"
  | "create_group" | "update_group" | "delete_group" | "group_member"
  | "create_project" | "update_project" | "delete_project" | "project_member" | "group_project_role"
  | "create_environment" | "update_environment" | "set_environment_capabilities" | "delete_environment" | "set_resource" | "delete_resource" | "settle_attempt" | "request_provision" | "prune_shared_data";
export type AdminDraft = Record<string, string | boolean | string[] | number>;
export function projectPCSelection(data: Pick<FormData, "getAll">): Pick<AdminDraft, "controller_device_ids" | "runner_device_ids"> {
  return { controller_device_ids: data.getAll("controller_device_ids").map(String), runner_device_ids: data.getAll("runner_device_ids").map(String) };
}
/** Suggest the existing physical pool only when the selected Runner has one unambiguous pool. */
export function environmentDefaults(snapshot: Pick<SharedAdminSnapshot, "environments" | "resources">, runnerId: string, newResourceId: string): AdminDraft {
  const ids = [...new Set(snapshot.environments.filter(row => row.runner_id === runnerId).map(row => row.resource_id))];
  const resource = ids.length === 1 ? snapshot.resources.find(row => row.id === ids[0]) : undefined;
  return { runner_id: runnerId, resource_id: resource?.id ?? (ids.length === 0 ? newResourceId : ""), capacity: String(resource?.capacity ?? 1), template_id: "" };
}
export const operationLabels: Record<AdminOperation, string> = {
  bind_device_principal: "このPCの利用者を設定",
  save_project: "プロジェクトを保存", retry_project_device: "実行PCのフォルダー作成を再試行",
  update_user: "利用者を編集", delete_user: "利用者を削除",
  create_group: "グループを追加", update_group: "グループを編集", delete_group: "グループを削除", group_member: "グループの所属を設定",
  create_project: "プロジェクトを追加", update_project: "プロジェクトを編集", delete_project: "プロジェクトを削除",
  project_member: "利用者の役割を設定", group_project_role: "グループの役割を設定",
  create_environment: "実行先を追加", update_environment: "実行先を編集", set_environment_capabilities: "このPCで使えるものを設定", delete_environment: "実行先を削除",
  set_resource: "共有枠を設定", delete_resource: "同時利用の枠を削除", settle_attempt: "停止を確認して枠を解放",
  request_provision: "作業フォルダーの作成を依頼", prune_shared_data: "保存データを整理",
};
const text = (draft: AdminDraft, key: string) => String(draft[key] ?? "").trim();
const flag = (draft: AdminDraft, key: string) => draft[key] === true;
function required(draft: AdminDraft, key: string): string {
  const value = text(draft, key);
  if (!value) throw "必須項目を入力してください。";
  return value;
}
function capacity(draft: AdminDraft): number {
  const value = text(draft, "capacity");
  if (!/^[1-9]\d*$/.test(value) || Number(value) > 128) throw "同時利用枠は1〜128で指定してください。";
  return Number(value);
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
    case "create_group": case "create_project": return { kind, id: id(), label: required(draft, "label") };
    case "update_group": return { kind, id: id(), label: required(draft, "label"), disabled: flag(draft, "disabled") };
    case "update_project": return { kind, id: id(), label: required(draft, "label"), archived: flag(draft, "archived") };
    case "delete_group": case "delete_project": case "delete_environment": case "delete_resource": return { kind, id: id() };
    case "group_member": return { kind, group_id: required(draft, "group_id"), user_id: user_id(), member: flag(draft, "member") };
    case "project_member": return { kind, project_id: required(draft, "project_id"), user_id: user_id(), role: role(draft) };
    case "group_project_role": return { kind, project_id: required(draft, "project_id"), group_id: required(draft, "group_id"), role: role(draft) };
    case "create_environment": case "update_environment": {
      const project_ids = Array.isArray(draft.project_ids) ? [...draft.project_ids] : [];
      if (!project_ids.length) throw "この実行先を使えるプロジェクトを一つ以上選択してください。";
      return { kind, id: id(), label: required(draft, "label"), resource_id: required(draft, "resource_id"), runner_id: required(draft, "runner_id"), project_ids,
        ...(kind === "create_environment" ? { capacity: capacity(draft), ...(text(draft, "template_id") ? { template_id: text(draft, "template_id") } : {}) } : { enabled: flag(draft, "enabled") }) };
    }
    case "set_environment_capabilities": {
      const names = [...new Set(String(draft.names ?? "").split(/\r?\n/).map(name => name.trim()).filter(Boolean))];
      if (names.length > 32 || names.some(name => new TextEncoder().encode(name).length > 128)) throw "使えるものは32件まで、各行128バイト以内で入力してください。";
      return { kind, environment_id: required(draft, "environment_id"), names };
    }
    case "request_provision": {
      const generation = Number(draft.generation);
      if (!Number.isSafeInteger(generation) || generation < 0) throw "最新のフォルダー作成状況を確認してください。";
      return { kind, environment_id: required(draft, "environment_id"), template_id: required(draft, "template_id"), generation };
    }
    case "prune_shared_data": {
      const before_ms = Number(draft.before_ms);
      if (!Number.isSafeInteger(before_ms) || before_ms < 1 || !flag(draft, "preview_confirmed")) throw "保存期間と削除予定の件数を確認してください。";
      return { kind, before_ms };
    }
    case "set_resource": return { kind, id: id(), capacity: capacity(draft) };
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
