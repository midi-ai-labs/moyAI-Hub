import { roleLabel, type AdminDraft, type AdminOperation, type SharedAdminSnapshot } from "./shared_admin_state.ts";

export interface AdminComparison {
  revision: string;
  rows: { label: string; saved: string; input: string }[];
  blocked: string | null;
}

/** Read-only comparison. It never changes a request, revision, or persisted value. */
export function compareAdminEdit(kind: AdminOperation, draft: AdminDraft, original: SharedAdminSnapshot, latest: SharedAdminSnapshot, devices: { device_id: string; label: string; revoked?: boolean }[]): AdminComparison {
  const result: AdminComparison = { revision: latest.revision, rows: [], blocked: null };
  const value = (key: string) => String(draft[key] ?? "");
  const display = (v: unknown) => typeof v === "boolean" ? v ? "はい" : "いいえ" : Array.isArray(v) ? v.join("、") || "なし" : String(v ?? "未登録");
  const row = (label: string, saved: unknown, input: unknown) => result.rows.push({ label, saved: display(saved), input: display(input) });
  const block = (message: string) => { result.blocked = message; return result; };
  const person = (id: string) => { const p = latest.users.find(u => u.user_id === id); return p ? `${p.display_name}（${p.username}）` : `削除された利用者（${id}）`; };
  const pc = (id: string) => `${devices.find(d => d.device_id === id)?.label ?? "登録を確認できないPC"}（${id}）`;
  const projectName = (id: string) => `${latest.projects.find(p => p.id === id)?.label ?? "削除されたプロジェクト"}（${id}）`;
  const users = ["create_user", "create_user_with_setup_code"];
  if (users.includes(kind)) {
    const existing = latest.users.find(u => u.username.toLowerCase() === value("username").trim().toLowerCase());
    row("ログイン名", existing?.username, value("username"));
    row("表示名", existing?.display_name, value("display_name"));
    row("Hub管理者", existing?.administrator, kind === "create_user" && draft.administrator === true);
    if (kind === "create_user") row("初期パスワード", "内容は表示しません", "入力を保持しています（非表示）");
    return existing ? block("このログイン名はすでに登録されています。入力を修正し、もう一度比較してください。") : result;
  }
  if (kind === "update_user" || kind === "set_password") {
    const saved = latest.users.find(u => u.user_id === value("user_id"));
    if (!saved || !original.users.some(u => u.user_id === saved.user_id)) return block("この利用者は削除されたか、登録を確認できません。別の利用者へは保存しません。");
    row("利用者", `${saved.display_name}（${saved.username}）`, person(saved.user_id));
    if (kind === "set_password") {
      if (saved.setup_pending) return block("この利用者は本人の初回設定待ちです。対象を開き直して設定方法を確認してください。");
      row("新しいパスワード", "内容は取得・表示しません", "入力を保持しています（非表示）");
    } else {
      row("表示名", saved.display_name, draft.display_name);
      row("Hub管理者", saved.administrator, draft.administrator === true);
      row("利用停止", saved.disabled, draft.disabled === true);
      if (saved.setup_pending && draft.administrator === true) return block("本人の初回設定が終わるまで管理者には変更できません。管理者の選択を外して比較してください。");
    }
    return result;
  }
  if (kind === "save_project") {
    const saved = latest.projects.find(p => p.id === value("id"));
    if (original.projects.some(p => p.id === value("id")) && !saved) return block("このプロジェクトは削除されました。同じ入力から作り直さず、対象を開き直してください。");
    if (!original.projects.some(p => p.id === value("id")) && saved) return block("この識別名のプロジェクトが別の操作で登録されました。対象を開き直してください。");
    row("プロジェクト名", saved?.label, draft.label);
    row("保管する", saved?.archived, draft.archived === true);
    const memberships = latest.project_memberships.filter(m => m.project_id === value("id"));
    const ids = new Set([...memberships.map(m => m.user_id), ...Object.keys(draft).filter(k => k.startsWith("participant:")).map(k => k.slice(12))]);
    const role = (v: unknown) => v === "reader" || v === "contributor" || v === "manager" ? roleLabel(v) : "参加しない";
    for (const id of ids) {
      row(person(id), role(memberships.find(m => m.user_id === id)?.role), role(draft[`participant:${id}`]));
      if (draft[`participant:${id}`] && !latest.users.some(u => u.user_id === id)) result.blocked = "選択した利用者が削除されました。対象を開き直してください。";
    }
    const assigned = (latest.project_devices ?? []).filter(p => p.project_id === value("id"));
    for (const [key, flag, label] of [["controller_device_ids", "can_control", "操作に使うPC"], ["runner_device_ids", "can_execute", "仕事を実行するPC"]] as const) {
      const selected = Array.isArray(draft[key]) ? draft[key] as string[] : [];
      row(label, saved?.legacy_devices_unrestricted && flag === "can_control" ? "従来の設定：登録したPCから操作可" : assigned.filter(p => p[flag]).map(p => pc(p.device_id)), selected.map(pc));
      if (selected.some(id => !devices.some(device => device.device_id === id && !device.revoked))) result.blocked = "選択したPCの登録が削除または失効しています。対象を開き直してPCを選び直してください。";
    }
    return result;
  }
  if (["create_group", "update_group", "create_project", "update_project"].includes(kind)) {
    const groups = kind.endsWith("group");
    const old = (groups ? original.groups : original.projects).find(p => p.id === value("id"));
    const saved = (groups ? latest.groups : latest.projects).find(p => p.id === value("id"));
    if (kind.startsWith("update_") && (!old || !saved)) return block("編集していた対象が削除されました。対象を開き直してください。");
    if (kind.startsWith("create_") && saved) return block("この識別名は登録済みです。別の識別名を入力して比較してください。");
    row("識別名", saved?.id, draft.id); row("表示名", saved?.label, draft.label);
    if (kind.startsWith("update_")) row(groups ? "利用停止" : "保管する", saved && ("disabled" in saved ? saved.disabled : saved.archived), groups ? draft.disabled === true : draft.archived === true);
    return result;
  }
  if (kind === "group_member" || kind === "project_member" || kind === "group_project_role") {
    if (kind !== "group_project_role" && !latest.users.some(u => u.user_id === value("user_id"))) return block("選択した利用者が削除されました。対象を開き直してください。");
    if (kind !== "project_member" && !latest.groups.some(g => g.id === value("group_id"))) return block("選択したグループが削除されました。対象を開き直してください。");
    if (kind !== "group_member" && !latest.projects.some(p => p.id === value("project_id"))) return block("選択したプロジェクトが削除されました。対象を開き直してください。");
    if (kind === "group_member") row("グループへの所属", latest.group_memberships.some(m => m.user_id === value("user_id") && m.group_id === value("group_id")), draft.member === true);
    else {
      const membership = kind === "project_member" ? latest.project_memberships.find(m => m.project_id === value("project_id") && m.user_id === value("user_id")) : latest.group_project_roles.find(m => m.project_id === value("project_id") && m.group_id === value("group_id"));
      row(projectName(value("project_id")), membership ? roleLabel(membership.role) : "参加しない", draft.role ? roleLabel(draft.role as "reader" | "contributor" | "manager") : "参加しない");
    }
    return result;
  }
  return block("この操作は最新の対象と確認内容を開き直してから行ってください。入力はこの画面に残っています。");
}
