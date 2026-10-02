import { invoke, invokeSharedAdmin, ManagementFailure, onAuthenticationChange } from "./management.ts";
import type { NetworkSnapshot } from "./network_state.ts";
import { adminRequest, adminTabs, operationLabels, projectPCSelection, projectPreparationLabel, roleLabel, type AdminDraft, type AdminOperation, type SharedAdminSnapshot, type SharedAdminTab } from "./shared_admin_state.ts";
import { compareAdminEdit, type AdminComparison } from "./admin_comparison.ts";
import { reconcileStableRows } from "./stable_rows.ts";
import "./shared_admin.css";

const esc = (value: unknown) => String(value ?? "").replace(/[&<>"']/g, char => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[char]!);
const action = (kind: AdminOperation, label = operationLabels[kind], id = "") => `<button type="button" class="secondary" data-sa-operation="${kind}" data-sa-id="${esc(id)}">${esc(label)}</button>`;
function rowKey(node: Element): string | null {
  if (node.classList.contains("shared-admin-row")) {
    const action = node.querySelector(".shared-admin-actions > button");
    const identity = action && rowKey(action);
    return identity ? `row:${identity}` : null;
  }
  if (node.id) return `${node.tagName}:id:${node.id}`;
  for (const attributes of [["data-sa-operation", "data-sa-id"], ["data-sa-retry-project", "data-sa-device"]]) {
    if (node.hasAttribute(attributes[0])) return `${node.tagName}:${JSON.stringify(attributes.map(name => node.getAttribute(name)))}`;
  }
  return null;
}
export interface SharedAdminDevice { device_id: string; label: string; revoked: boolean; admission?: string; windows_username?: string | null }
interface Editor { kind: AdminOperation; draft: AdminDraft; revision: string; conflict: boolean; baseline: SharedAdminSnapshot; comparison: AdminComparison | null }
interface Pending { request_id: string; expected_revision: string; request: Record<string, unknown> }

export function attachSharedAdmin(host: HTMLElement, onSnapshot?: (value: SharedAdminSnapshot) => void): { refresh: () => Promise<void>; updateDevices: (devices: SharedAdminDevice[]) => void; openProject: (id?: string) => void } {
  let snapshot: SharedAdminSnapshot | null = null;
  let devices: SharedAdminDevice[] = [];
  let tab: SharedAdminTab = "projects";
  let editor: Editor | null = null;
  let pending: Pending | null = null;
  let maintenanceBefore: string | null = null;
  let busy = false, polling = false, serial = 0, disposed = false;
  host.innerHTML = `<div class="section-title"><div><p class="eyebrow">プロジェクトで使うPC</p><h2>プロジェクト</h2></div><button id="shared-admin-refresh" type="button" class="secondary">最新情報を取得</button></div>
    <p class="subtle">プロジェクトごとに、依頼を送るPCと仕事を実行するPCを選びます。PCは「接続PC」で参加を許可してから選んでください。</p>
    <p class="footnote">同じPCを両方の用途や複数のプロジェクトで使えます。操作PCでは、Desktopを開くと使えるプロジェクトが表示されます。</p>
    <p id="shared-admin-notice" role="status" aria-live="polite"></p>
    <div class="shared-admin-tabs" role="tablist" aria-label="共同利用の管理">${adminTabs.map(([id, label]) => `<button type="button" role="tab" data-sa-tab="${id}" aria-selected="${id === tab}" aria-controls="shared-admin-content">${label}</button>`).join("")}</div>
    <details><summary>既存の利用者・権限（詳細）</summary><p>複数PCで利用者の権限や過去の記録を共有する場合に設定します。通常は設定不要です。</p><button type="button" class="secondary" data-sa-tab="users">利用者と既存の権限</button></details>
    <div class="shared-admin-columns"><section id="shared-admin-content" role="tabpanel" tabindex="-1"></section><section id="shared-admin-editor" aria-label="設定の編集"></section></div>`;
  const node = <T extends HTMLElement>(id: string) => host.querySelector<T>(`#${id}`)!;
  const notice = (message: string, failure = false) => { if (disposed) return; node("shared-admin-notice").textContent = message; node("shared-admin-notice").classList.toggle("error", failure); };
  function lock() {
    if (disposed) return;
    host.setAttribute("aria-busy", String(busy));
    for (const control of host.querySelectorAll<HTMLButtonElement | HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>("button,input,select,textarea")) {
      control.disabled = control.hasAttribute("data-fixed-disabled") || busy || (!snapshot && control.id !== "shared-admin-refresh") || (Boolean(pending) && !["shared-admin-retry", "shared-admin-refresh"].includes(control.id)) || (Boolean(editor?.conflict) && control.id === "shared-admin-save");
    }
  }
  function accept(value: SharedAdminSnapshot) {
    if (disposed) return;
    if (snapshot && BigInt(value.revision) < BigInt(snapshot.revision)) return;
    snapshot = value;
    onSnapshot?.(value);
    renderRows(); lock();
  }
  const user = (id: string) => snapshot?.users.find(row => row.user_id === id)?.display_name ?? id;
  const runner = (id: string) => devices.find(row => row.device_id === id)?.label ?? id;
  function renderRows() {
    if (disposed) return;
    if (!snapshot) { node("shared-admin-content").textContent = "共有設定を取得しています。"; return; }
    const s = snapshot;
    const card = (title: string, detail: string, controls: string) => `<article class="shared-admin-row"><h3>${esc(title)}</h3><div class="shared-admin-description">${detail}</div><div class="shared-admin-actions">${controls}</div></article>`;
    let rows = (() => {
      switch (tab) {
        case "users": return `<h3>PCの利用者を設定</h3><p>新しいPCには専用の利用者を自動登録します。既存の権限・記録を使う場合は利用者を選んでください。Windowsユーザー名が同じでも自動では統合しません。</p>`
          + devices.filter(pc => !pc.revoked).map(pc => { const principal = s.device_principals?.find(p => p.device_id === pc.device_id); return card(pc.label, `<p>Windowsユーザー名: ${esc(pc.windows_username || "未取得")}</p><p>記録する利用者: ${esc(principal?.display_name ?? "未設定（プロジェクトの保存で準備）")}</p>`, action("bind_device_principal", "利用者を選ぶ", pc.device_id)); }).join("")
          + `<h3>保存済みの利用者と権限</h3><p>これまでの依頼・承認の記録は保持します。閲覧だけなどの個別の権限を設定した場合は、その指定が操作PCの標準権限より優先されます。</p>${action("project_member")}${action("group_member")}${action("group_project_role")}`
          + s.users.map(row => card(row.display_name, `<p>${row.disabled ? "利用停止" : row.setup_pending ? "旧方式の未完了登録（引き継ぎ不可）" : "有効"} · ${row.administrator ? "Hub管理を許可" : "Hub管理の許可なし"}</p><p>${s.project_memberships.filter(m => m.user_id === row.user_id).map(m => `${esc(s.projects.find(p => p.id === m.project_id)?.label ?? m.project_id)}: ${roleLabel(m.role)}`).join("、") || "個別の役割指定なし"}</p>`, action("update_user", "表示名・利用状態", row.user_id) + action("delete_user", "削除", row.user_id))).join("");
        case "projects": return action("save_project", "プロジェクトを追加") + s.projects.map(row => {
          const assignments=(s.project_devices ?? []).filter(d=>d.project_id===row.id);
          const state = projectPreparationLabel;
          const controls=row.legacy_devices_unrestricted ? "従来の設定（登録したPCから操作可）" : assignments.filter(d=>d.can_control).map(d=>runner(d.device_id)).join("、") || "未選択";
          const executors=assignments.filter(d=>d.can_execute).map(d=>{ const environment=s.environments.find(e=>e.id===d.environment_id); const names=environment?.capabilities?.map(c=>c.name) ?? []; return `<p>${esc(runner(d.device_id))}: ${esc(state(d.preparation_state))}${d.error ? ` — ${esc(d.error)}` : ""}${d.preparation_state==="failed" ? `<button type="button" class="secondary" data-sa-retry-project="${esc(row.id)}" data-sa-device="${esc(d.device_id)}">フォルダー作成を再試行</button>` : ""}</p>${environment ? `<p>このPCで使えるもの: ${names.length ? names.map(esc).join("、") : "未設定"} ${action("set_environment_capabilities", "設定", environment.id)}</p>` : ""}`; }).join("") || "<p>未選択</p>";
          return card(row.label, `<p>${row.archived ? "保管中" : "利用中"}</p><p>操作に使うPC: ${esc(controls)}</p><div>仕事を実行するPC${executors}</div>`, action("save_project","設定を編集",row.id)+action("delete_project","削除",row.id));
        }).join("");
        case "maintenance": return `<p>実行を引き受けたPCと仕事を表示します。実行状態が不明な場合は、そのPCの処理と外部ジョブが止まったことを確認してから「停止確認を記録」を押してください。確認が済むまで、その仕事が使っている枠は再利用できません。</p>` + (s.maintenance.map(row => card(runner(row.runner_id), `<p>状態: ${esc(({assigned:"実行PCへの割り当て済み",running:"実行中",stop_requested:"停止要求中"} as Record<string,string>)[row.state] ?? row.state)}</p><p>${esc(row.uncertainty_reason ?? "")}</p><p class="shared-admin-mono">仕事 ${esc(row.job_id)}<br>試行 ${esc(row.attempt_id)}</p>`, action("settle_attempt", "停止確認を記録", row.attempt_id))).join("") || "<p>停止確認の対象はありません。</p>");
        case "storage": return `<p>保存中: ${s.storage.retained_bytes.toLocaleString()} / ${s.storage.limit_bytes.toLocaleString()} バイト<br>ファイル ${s.storage.asset_count}件 · 内容削除済み ${s.storage.purged_asset_count}件</p><p>終了した仕事の保存期間を指定して、古いファイルを削除できます。利用中の入力と、まだ完了していない追加依頼に必要なファイルは残ります。削除は管理者が実行したときだけ行われます。</p>${action("prune_shared_data")}<h3>バックアップと復元</h3><p>Hubを停止し、Hubの端末上でバックアップを作成してください。バックアップには利用者情報・証明書・仕事の内容が含まれます。復元前に、関係する実行PCのRunnerと、そのRunnerが起動した処理を停止したことを各PCで確認してください。復元先には、まだ存在しない新しいフォルダーを指定します（親フォルダーは作成済みである必要があります）。復元後はPCの接続と新しい仕事の受付が停止します。実行状態が不明な仕事の停止確認を記録してから、接続と受付を再開してください。</p><pre>moyai-hub --backup-dir BACKUP --data-dir HUBDATA\nmoyai-hub --restore-from BACKUP --data-dir NEW_DIR --acknowledge-runners-stopped</pre>`;
      }
    })();
    if (tab === "maintenance") rows += `<p>対象: ${s.maintenance_total}件</p><div class="shared-admin-actions">${s.maintenance_next_before ? '<button type="button" id="shared-admin-maintenance-next" class="secondary">次の復旧対象</button>' : ""}${maintenanceBefore ? '<button type="button" id="shared-admin-maintenance-latest" class="secondary">先頭の復旧対象</button>' : ""}</div>`;
    const container = node("shared-admin-content");
    // These rows contain server facts/actions; the separate editor owns drafts.
    reconcileStableRows(container, rows, container, rowKey);
    for (const button of host.querySelectorAll<HTMLElement>("[data-sa-tab]")) button.setAttribute("aria-selected", String(button.dataset.saTab === tab));
  }
  function open(kind: AdminOperation, id: string) {
    if (!snapshot || busy || pending) return;
    let draft: AdminDraft = { id, user_id: id, member: true, role: "contributor" };
    if (kind === "save_project") {
      const project=snapshot.projects.find(row=>row.id===id);
      draft={ id:project?.id ?? `project-${crypto.randomUUID()}`, label:project?.label ?? "", overview:project?.overview ?? "", archived:project?.archived ?? false,
        controller_device_ids:project?.legacy_devices_unrestricted ? devices.filter(d=>!d.revoked).map(d=>d.device_id) : (snapshot.project_devices ?? []).filter(d=>d.project_id===id&&d.can_control).map(d=>d.device_id),
        runner_device_ids:(snapshot.project_devices ?? []).filter(d=>d.project_id===id&&d.can_execute).map(d=>d.device_id) };
    }
    if (["update_user", "delete_user"].includes(kind)) Object.assign(draft, snapshot.users.find(row => row.user_id === id));
    if (kind === "bind_device_principal") draft = { device_id: id, user_id: snapshot.device_principals?.find(p => p.device_id === id)?.user_id ?? "", display_name: runner(id) };
    if (kind === "group_member") draft.group_id = id;
    if (kind === "project_member" || kind === "group_project_role") draft.project_id = id;
    if (kind === "set_environment_capabilities") { const environment=snapshot.environments.find(row=>row.id===id); if (!environment) { notice("実行先の最新情報を取得してください。", true); return; } draft={environment_id:id,names:(environment.capabilities ?? []).map(c=>c.name).join("\n")}; }
    if (kind === "settle_attempt") Object.assign(draft, snapshot.maintenance.find(row => row.attempt_id === id));
    editor = { kind, draft, revision: snapshot.revision, conflict: false, baseline: snapshot, comparison: null };
    renderEditor(); lock();
    node("shared-admin-editor").querySelector<HTMLElement>("input:not([readonly]),select,textarea,button")?.focus();
  }
  function renderEditor() {
    if (disposed) return;
    if (!editor || !snapshot) { node("shared-admin-editor").innerHTML = "<p>項目を選ぶと設定を編集できます。</p>"; return; }
    const { kind, draft: d } = editor;
    const field = (key: string, label: string, options: { type?: string; readonly?: boolean; hint?: string } = {}) => `<label>${esc(label)}<input name="${key}" id="shared-admin-${key}" type="${options.type ?? "text"}" value="${esc(d[key])}" ${options.readonly ? "readonly" : ""} ${options.type === "password" ? 'autocomplete="new-password"' : 'autocomplete="off"'} required>${options.hint ? `<small>${esc(options.hint)}</small>` : ""}</label>`;
    const check = (key: string, label: string) => `<label class="shared-admin-check"><input name="${key}" type="checkbox" ${d[key] === true ? "checked" : ""}>${esc(label)}</label>`;
    const select = (key: string, label: string, rows: [string, string][], placeholder = "選択してください") => `<label>${esc(label)}<select name="${key}" id="shared-admin-${key}"><option value="">${esc(placeholder)}</option>${rows.map(([id, label]) => `<option value="${esc(id)}" ${d[key] === id ? "selected" : ""}>${esc(label)}</option>`).join("")}</select></label>`;
    const roles: [string, string][] = [["reader", "閲覧"], ["contributor", "仕事の依頼・実行"], ["manager", "プロジェクト管理"]];
    const userSelect = () => select("user_id", "利用者", snapshot!.users.map(u => [u.user_id, `${u.display_name} (${u.username})`]));
    const projectSelect = () => select("project_id", "プロジェクト", snapshot!.projects.map(p => [p.id, p.label]));
    const groupSelect = () => select("group_id", "グループ", snapshot!.groups.map(g => [g.id, g.label]));
    const fields = (() => {
      switch (kind) {
        case "save_project": {
          const selected=(key:string,id:string)=>Array.isArray(d[key])&&(d[key] as string[]).includes(id);
          const pcRows=devices.filter(pc=>!pc.revoked || selected("controller_device_ids",pc.device_id)||selected("runner_device_ids",pc.device_id)).map(pc=>`<tr><th scope="row">${esc(pc.label)}${pc.revoked ? "（登録失効）" : ""}</th><td><input type="checkbox" name="controller_device_ids" value="${esc(pc.device_id)}" aria-label="${esc(pc.label)}を操作に使う" ${selected("controller_device_ids",pc.device_id)?"checked":""}></td><td><input type="checkbox" name="runner_device_ids" value="${esc(pc.device_id)}" aria-label="${esc(pc.label)}で仕事を実行する" ${selected("runner_device_ids",pc.device_id)?"checked":""}></td></tr>`).join("");
          return field("label","プロジェクト名")+`<label>プロジェクト概要<textarea name="overview" id="shared-admin-overview" rows="5" placeholder="目的、各PCの役割、接続関係、開発上の制約など">${esc(d.overview ?? "")}</textarea><small>例: WinBはAPサーバー、WinCはDB。AIが作業を計画する際に参照します。変更は次の依頼から反映します。</small></label><fieldset><legend>使うPC</legend><p>操作PCでは依頼と成果の確認ができます。同じPCを両方に選べます。実行PCでは初回だけAI・保存先・権限の確認が必要です。</p><table class="shared-admin-pc-table"><thead><tr><th>PC</th><th>操作に使う</th><th>仕事を実行する</th></tr></thead><tbody>${pcRows}</tbody></table>${!pcRows?'<p>「接続PC」でPCの参加申請を許可してください。</p>':""}</fieldset><p>保存すると選択したPCへ反映します。</p>`+check("archived","プロジェクトを保管する（新しい仕事を受け付けない）");
        }
        case "retry_project_device": return `<p>${esc(runner(String(d.device_id)))} の実行準備をもう一度依頼します。保存先の問題は、そのPCで修正してください。</p>`;

        case "bind_device_principal": return `<p>${esc(runner(String(d.device_id)))}</p><p>このPCで今後行う依頼・承認を、選んだ利用者の記録として扱います。その利用者のプロジェクト権限とHub管理の許可が、このPCにも適用されます。</p>` + select("user_id", "このPCの利用者", snapshot!.users.filter(p => !p.disabled && !p.setup_pending).map(p => [p.user_id, `${p.display_name}（${p.administrator ? "Hub管理を許可" : "Hub管理の許可なし"}）`]), "このPC専用の利用者を使う") + '<p class="footnote">別のPCからHubを管理するにはHTTPSの管理URLも必要です。PC自体の登録と用途は変わりません。既存の依頼の続行・承認は、依頼元のPCで行います。</p>';
        case "update_user": return field("display_name", "表示名") + (snapshot!.users.some(p => p.user_id === d.user_id && p.setup_pending)
          ? "" : check("administrator", "この利用者のPCからHubを管理できる") + '<p class="footnote">この利用者を引き継いだすべてのPCが対象です。他のPCから管理するには、HubのHTTPS管理URLの設定も必要です。</p>') + check("disabled", "利用を停止する");
        case "group_member": return groupSelect() + userSelect() + check("member", "このグループへ所属させる");
        case "project_member": return projectSelect() + userSelect() + select("role", "直接指定する役割", roles, "直接の所属を解除");
        case "group_project_role": return projectSelect() + groupSelect() + select("role", "グループに与える役割", roles, "グループの役割を解除");
        case "set_environment_capabilities": return `<p>${esc(snapshot!.environments.find(row=>row.id===d.environment_id)?.label ?? d.environment_id)}</p><p>この実行先で使えるソフトや機器を、一行に一つずつ入力してください。同じ実行先を使うすべてのプロジェクトに反映します。ここで登録しても、実際に動くことの確認にはなりません。</p><label>使えるもの<textarea name="names" id="shared-admin-names" rows="6">${esc(d.names)}</textarea></label><p class="footnote">空欄で保存すると登録をすべて消します。</p>`;
        case "prune_shared_data": return field("cutoff", "この日より前に終了した仕事", { type: "date" }) + `<button id="shared-admin-preview" type="button" class="secondary">削除予定を確認</button><p id="shared-admin-preview-result" role="status">先に削除予定の件数と容量を確認してください。</p>` + check("preview_confirmed", "表示された保存内容の削除を確認した（仕事の記録は保持）");
        case "settle_attempt": return `<p class="shared-admin-mono">${esc(d.attempt_id)}</p><p>実行PCの処理と外部ジョブが止まり、使用していたPCやソフトウェアを再利用できることを確認してください。PCが未接続でも、処理が続いている場合があります。</p><label>確認した方法と理由<textarea name="reason" id="shared-admin-reason" required rows="4">${esc(d.reason)}</textarea></label>` + check("acknowledge_stopped", "対象の処理が停止し、PCやソフトウェアを再利用できることを確認した");
        default: return `<p>${esc(kind === "delete_user" ? user(String(d.user_id)) : d.id)} を削除します。使用済みの記録がある場合は削除できません。利用停止・保管を使ってください。</p>`;
      }
    })();
    node("shared-admin-editor").innerHTML = `<form id="shared-admin-form"><h3>${operationLabels[kind]}</h3>${fields}<p id="shared-admin-form-error" role="status"></p><section id="shared-admin-comparison" aria-label="保存中の設定と入力の比較" hidden></section><div class="shared-admin-actions"><button id="shared-admin-save" type="submit" class="${kind.startsWith("delete_") ? "danger" : "primary"}">${operationLabels[kind]}</button><button id="shared-admin-review-conflict" type="button" class="secondary" hidden>入力を残して最新の設定と比較</button><button id="shared-admin-close" type="button" class="secondary">閉じる</button><button id="shared-admin-retry" type="button" class="secondary" hidden>同じ内容の受付を確認</button></div></form>`;
    node("shared-admin-review-conflict").hidden = !editor.conflict;
  }
  async function refresh() {
    if (disposed || busy || polling) return;
    polling = true; const expected = serial;
    try {
      const result = await invokeSharedAdmin<SharedAdminSnapshot>("hub_shared_snapshot");
      if (maintenanceBefore) {
        const page = await invokeSharedAdmin<{ items: SharedAdminSnapshot["maintenance"]; next_before: string | null; total_active: number }>("hub_shared_maintenance", { before: maintenanceBefore, limit: 128 });
        result.maintenance = page.items; result.maintenance_next_before = page.next_before; result.maintenance_total = page.total_active;
      }
      if (expected === serial) accept(result);
    }
    catch (error) { if (expected === serial) notice(error instanceof Error ? error.message : String(error), true); }
    finally { polling = false; lock(); }
  }
  function captureEditor() {
    if (!editor) return;
    const data = new FormData(node<HTMLFormElement>("shared-admin-form"));
    // An empty PC list is still a selection: clear deleted devices even when
    // no checkbox remains to visit below.
    if (editor.kind === "save_project") Object.assign(editor.draft, projectPCSelection(data));
    for (const field of node("shared-admin-editor").querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>("[name]")) {
      if (["controller_device_ids","runner_device_ids"].includes(field.name)) editor.draft[field.name] = data.getAll(field.name).map(String);
      else editor.draft[field.name] = field instanceof HTMLInputElement && field.type === "checkbox" ? field.checked : field.value;
    }
  }
  async function save(retry: boolean) {
    if (disposed || !snapshot || !editor || busy || editor.conflict) return;
    if (!retry) {
      captureEditor();
      try { pending = { request_id: crypto.randomUUID(), expected_revision: editor.revision, request: adminRequest(editor.kind, editor.draft) }; }
      catch (error) { node("shared-admin-form-error").textContent = String(error); return; }
    }
    if (!pending) return;
    busy = true; const expected = ++serial; lock(); notice("設定を保存しています…");
    try {
      const result = await invokeSharedAdmin<SharedAdminSnapshot>("hub_shared_command", { ...pending });
      if (disposed || expected !== serial) return;
      pending = null; editor = null; maintenanceBefore = null; accept(result); renderEditor(); notice("設定を保存しました。");
    } catch (error) {
      if (disposed || expected !== serial) return;
      const definitive = error instanceof ManagementFailure && [400, 403, 404, 409, 429].includes(error.status) && error.code !== "unavailable";
      if (definitive) { pending = null; if (error.code === "conflict") editor!.conflict = true; }
      const message = editor?.conflict ? "設定が別の操作で更新されました。入力は残っています。最新の設定と比較してから保存し直してください。" : error instanceof Error ? error.message : String(error);
      notice(message, true);
      if (editor) {
        node("shared-admin-form-error").textContent = editor.conflict ? message : pending ? "受付結果が未確認です。入力を変更せず、同じ内容で確認してください。" : message;
        node("shared-admin-retry").hidden = !pending;
        node("shared-admin-review-conflict").hidden = !editor.conflict;
      }
    } finally { busy = false; lock(); }
  }
  async function reviewConflict() {
    if (disposed || !editor?.conflict || busy || pending) return;
    captureEditor();
    const target = editor, expected = ++serial;
    target.comparison = null;
    node("shared-admin-comparison").hidden = true;
    busy = true; lock(); notice("入力を保持したまま、最新の設定を取得しています…");
    try {
      const [latest, network] = await Promise.all([
        invokeSharedAdmin<SharedAdminSnapshot>("hub_shared_snapshot"),
        ["save_project", "bind_device_principal"].includes(target.kind) ? invoke<NetworkSnapshot>("hub_network_snapshot") : Promise.resolve(null),
      ]);
      if (disposed || expected !== serial || editor !== target) return;
      if (network) devices = network.devices;
      accept(latest);
      target.comparison = compareAdminEdit(target.kind, target.draft, target.baseline, latest, devices);
      const comparison = target.comparison;
      node("shared-admin-comparison").innerHTML = `<h4>保存中の設定と今回の入力</h4><p>まだ保存していません。異なる項目を確認してください。</p>${comparison.rows.length ? `<div class="table-wrap"><table><thead><tr><th>項目</th><th>現在保存されている値</th><th>今回の入力</th></tr></thead><tbody>${comparison.rows.map(row => `<tr class="${row.saved === row.input ? "" : "shared-admin-changed"}"><th scope="row">${esc(row.label)}</th><td>${esc(row.saved)}</td><td>${esc(row.input)}</td></tr>`).join("")}</tbody></table></div>` : ""}${comparison.blocked ? `<p role="status">${esc(comparison.blocked)}</p>` : '<p>比較を確認したら、編集を続けて保存してください。</p><button id="shared-admin-accept-comparison" type="button" class="secondary">比較した内容で編集を続ける</button>'}`;
      node("shared-admin-comparison").hidden = false;
      notice(comparison.blocked ?? "比較結果を表示しました。保存する内容を確認してください。", Boolean(comparison.blocked));
    } catch (error) { if (!disposed && expected === serial && editor === target) notice(error instanceof Error ? error.message : String(error), true); }
    finally { busy = false; lock(); }
  }
  function invalidateComparison() {
    if (!editor?.comparison) return;
    editor.comparison = null;
    node("shared-admin-comparison").hidden = true;
    node("shared-admin-form-error").textContent = "入力を変更しました。最新の設定ともう一度比較してください。";
  }
  async function previewRetention() {
    if (!editor || editor.kind !== "prune_shared_data" || busy || disposed) return;
    const beforeMs = new Date(node<HTMLInputElement>("shared-admin-cutoff").value).getTime();
    if (!Number.isSafeInteger(beforeMs) || beforeMs < 1) { notice("保存期限の日付を選択してください。", true); return; }
    const expected = ++serial; busy = true; lock();
    try {
      const value = await invokeSharedAdmin<{ before_ms: number; asset_count: number; byte_count: number }>("hub_shared_retention_preview", { beforeMs });
      if (disposed || expected !== serial || !editor) return;
      editor.draft.before_ms = value.before_ms;
      editor.draft.preview_cutoff = node<HTMLInputElement>("shared-admin-cutoff").value;
      node("shared-admin-preview-result").textContent = `${value.asset_count}件 / ${value.byte_count.toLocaleString()} bytes。実行時にも利用中データを保護します。`;
    } catch (error) { if (!disposed && expected === serial) notice(String(error), true); }
    finally { busy = false; lock(); }
  }
  host.addEventListener("submit", event => {
    if (event.target instanceof HTMLFormElement && event.target.getAttribute("id") === "shared-admin-form") { event.preventDefault(); void save(false); }
  });
  for (const eventName of ["input", "change"]) host.addEventListener(eventName, event => {
    if ((event.target as HTMLElement).closest("#shared-admin-form")) invalidateComparison();
  });
  host.addEventListener("click", event => {
    const target = (event.target as HTMLElement).closest<HTMLButtonElement>("button");
    if (!target || target.disabled) return;
    if (target.id === "shared-admin-review-conflict") { void reviewConflict(); return; }
    if (target.id === "shared-admin-accept-comparison" && editor?.comparison && !busy) {
      const comparison = editor.comparison;
      if (comparison.blocked) return;
      if (!snapshot || snapshot.revision !== comparison.revision) {
        invalidateComparison(); notice("比較した後に設定が更新されました。もう一度、最新の設定と比較してください。", true); return;
      }
      editor.revision = comparison.revision; editor.baseline = snapshot; editor.comparison = null; editor.conflict = false;
      renderEditor(); lock(); notice("入力を保持しました。内容を編集し、保存してください。"); return;
    }
    if (target.dataset.saTab && !pending) { tab = target.dataset.saTab as SharedAdminTab; editor = null; renderRows(); renderEditor(); lock(); }
    if (target.dataset.saRetryProject && target.dataset.saDevice && snapshot && !pending) {
      editor={kind:"retry_project_device",draft:{project_id:target.dataset.saRetryProject,device_id:target.dataset.saDevice},revision:snapshot.revision,conflict:false,baseline:snapshot,comparison:null}; renderEditor(); lock();
    }
    if (target.dataset.saOperation) open(target.dataset.saOperation as AdminOperation, target.dataset.saId ?? "");
    if (target.id === "shared-admin-close" && !pending) { editor = null; renderEditor(); }
    if (target.id === "shared-admin-retry") void save(true);
    if (target.id === "shared-admin-refresh") void refresh();
    if (target.id === "shared-admin-preview") void previewRetention();
    if (target.id === "shared-admin-maintenance-next" && !editor) { maintenanceBefore = snapshot?.maintenance_next_before ?? null; void refresh(); }
    if (target.id === "shared-admin-maintenance-latest" && !editor) { maintenanceBefore = null; void refresh(); }
  });
  host.addEventListener("input", event => {
    if ((event.target as HTMLElement).id === "shared-admin-cutoff" && editor) { delete editor.draft.before_ms; editor.draft.preview_confirmed = false; }
  });
  host.addEventListener("focusout", event => {
    // focusout precedes the new button's focus/click. Keep that exact row alive.
    if (event.relatedTarget instanceof Node && node("shared-admin-content").contains(event.relatedTarget)) return;
    queueMicrotask(() => { if (!busy) { renderRows(); lock(); } });
  });
  onAuthenticationChange(session => {
    if (!session?.authentication.user) { disposed = true; ++serial; snapshot = null; editor = null; pending = null; host.replaceChildren(); }
  });
  renderRows(); renderEditor(); lock();
  return { refresh, updateDevices(value) { devices = value; }, openProject(id = "") { if (pending || busy) return; tab="projects"; renderRows(); open("save_project",id); host.scrollIntoView(); } };
}
