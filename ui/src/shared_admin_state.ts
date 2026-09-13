export type ProjectRole = "reader" | "contributor" | "manager";
export interface SharedAdminUser { user_id: string; username: string; display_name: string; administrator: boolean; disabled: boolean }
export interface SharedAdminSnapshot {
  revision: string;
  users: SharedAdminUser[];
  groups: { id: string; label: string; disabled: boolean }[];
  group_memberships: { group_id: string; user_id: string }[];
  projects: { id: string; label: string; archived: boolean }[];
  project_memberships: { project_id: string; user_id: string; role: ProjectRole }[];
  group_project_roles: { project_id: string; group_id: string; role: ProjectRole }[];
  environments: { id: string; label: string; resource_id: string; runner_id: string; enabled: boolean; project_ids: string[] }[];
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
  ["users", "利用者"], ["groups", "利用者グループ"], ["projects", "プロジェクト"],
  ["environments", "仕事の実行先"], ["resources", "同時利用の枠"], ["maintenance", "停止状態の確認"], ["storage", "保存と保守"],
];
export type AdminOperation = "create_user" | "update_user" | "set_password" | "delete_user"
  | "create_group" | "update_group" | "delete_group" | "group_member"
  | "create_project" | "update_project" | "delete_project" | "project_member" | "group_project_role"
  | "create_environment" | "update_environment" | "delete_environment" | "set_resource" | "delete_resource" | "settle_attempt" | "request_provision" | "prune_shared_data";
export type AdminDraft = Record<string, string | boolean | string[] | number>;
/** Suggest the existing physical pool only when the selected Runner has one unambiguous pool. */
export function environmentDefaults(snapshot: Pick<SharedAdminSnapshot, "environments" | "resources">, runnerId: string, newResourceId: string): AdminDraft {
  const ids = [...new Set(snapshot.environments.filter(row => row.runner_id === runnerId).map(row => row.resource_id))];
  const resource = ids.length === 1 ? snapshot.resources.find(row => row.id === ids[0]) : undefined;
  return { runner_id: runnerId, resource_id: resource?.id ?? (ids.length === 0 ? newResourceId : ""), capacity: String(resource?.capacity ?? 1), template_id: "" };
}
export const operationLabels: Record<AdminOperation, string> = {
  create_user: "利用者を追加", update_user: "利用者を編集", set_password: "パスワードを再設定", delete_user: "利用者を削除",
  create_group: "グループを追加", update_group: "グループを編集", delete_group: "グループを削除", group_member: "グループの所属を設定",
  create_project: "プロジェクトを追加", update_project: "プロジェクトを編集", delete_project: "プロジェクトを削除",
  project_member: "利用者の役割を設定", group_project_role: "グループの役割を設定",
  create_environment: "実行先を追加", update_environment: "実行先を編集", delete_environment: "実行先を削除",
  set_resource: "共有枠を設定", delete_resource: "共有資源を削除", settle_attempt: "実停止を確認して清算",
  request_provision: "実行領域の作成を依頼", prune_shared_data: "保存データを整理",
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
function password(draft: AdminDraft): string {
  const value = String(draft.password ?? "");
  if ([...value].length < 12 || new TextEncoder().encode(value).length > 1024) throw "パスワードは12文字以上、UTF-8で1024バイト以内にしてください。";
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
    case "create_user": return { kind, username: required(draft, "username"), display_name: required(draft, "display_name"), password: password(draft), administrator: flag(draft, "administrator") };
    case "update_user": return { kind, user_id: user_id(), display_name: required(draft, "display_name"), administrator: flag(draft, "administrator"), disabled: flag(draft, "disabled") };
    case "set_password": return { kind, user_id: user_id(), password: password(draft) };
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
      if (!project_ids.length) throw "公開するプロジェクトを一つ以上選択してください。";
      return { kind, id: id(), label: required(draft, "label"), resource_id: required(draft, "resource_id"), runner_id: required(draft, "runner_id"), project_ids,
        ...(kind === "create_environment" ? { capacity: capacity(draft), ...(text(draft, "template_id") ? { template_id: text(draft, "template_id") } : {}) } : { enabled: flag(draft, "enabled") }) };
    }
    case "request_provision": {
      const generation = Number(draft.generation);
      if (!Number.isSafeInteger(generation) || generation < 0) throw "最新の配置状態を確認してください。";
      return { kind, environment_id: required(draft, "environment_id"), template_id: required(draft, "template_id"), generation };
    }
    case "prune_shared_data": {
      const before_ms = Number(draft.before_ms);
      if (!Number.isSafeInteger(before_ms) || before_ms < 1 || !flag(draft, "preview_confirmed")) throw "保存期間と削除予定の件数を確認してください。";
      return { kind, before_ms };
    }
    case "set_resource": return { kind, id: id(), capacity: capacity(draft) };
    case "settle_attempt": {
      if (!flag(draft, "acknowledge_stopped")) throw "対象の実処理と資源が停止済みであることを確認してください。";
      const generation = Number(draft.generation);
      if (!Number.isSafeInteger(generation) || generation < 1) throw "最新の復旧対象を選択してください。";
      return { kind, attempt_id: required(draft, "attempt_id"), generation, acknowledge_stopped: true, reason: required(draft, "reason") };
    }
  }
}
export function roleLabel(role: ProjectRole): string {
  return { reader: "閲覧", contributor: "仕事の投入・実行", manager: "プロジェクト管理" }[role];
}
