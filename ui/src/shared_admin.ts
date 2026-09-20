import { invoke, invokeSharedAdmin, ManagementFailure, onAuthenticationChange } from "./management.ts";
import type { NetworkSnapshot } from "./network_state.ts";
import { adminRequest, adminTabs, environmentDefaults, operationLabels, projectPCSelection, roleLabel, type AdminDraft, type AdminOperation, type SharedAdminSnapshot, type SharedAdminTab } from "./shared_admin_state.ts";
import { compareAdminEdit, type AdminComparison } from "./admin_comparison.ts";
import "./shared_admin.css";

const esc = (value: unknown) => String(value ?? "").replace(/[&<>"']/g, char => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[char]!);
const action = (kind: AdminOperation, label = operationLabels[kind], id = "") => `<button type="button" class="secondary" data-sa-operation="${kind}" data-sa-id="${esc(id)}">${esc(label)}</button>`;
export interface SharedAdminDevice { device_id: string; label: string; revoked: boolean; admission?: string }
interface Editor { kind: AdminOperation; draft: AdminDraft; revision: string; conflict: boolean; baseline: SharedAdminSnapshot; comparison: AdminComparison | null }
interface Pending { request_id: string; expected_revision: string; request: Record<string, unknown> }

export function attachSharedAdmin(host: HTMLElement, onSnapshot?: (value: SharedAdminSnapshot) => void): { refresh: () => Promise<void>; updateDevices: (devices: SharedAdminDevice[]) => void; openProject: (id?: string) => void; openUser: () => void } {
  let snapshot: SharedAdminSnapshot | null = null;
  let devices: SharedAdminDevice[] = [];
  let tab: SharedAdminTab = "projects";
  let editor: Editor | null = null;
  let pending: Pending | null = null;
  let maintenanceBefore: string | null = null;
  let busy = false, polling = false, serial = 0, disposed = false;
  host.innerHTML = `<div class="section-title"><div><p class="eyebrow">利用者とPCのアクセス設定</p><h2>プロジェクト・利用者</h2></div><button id="shared-admin-refresh" type="button" class="secondary">最新情報を取得</button></div>
    <p class="subtle">プロジェクトごとに、参加者、依頼を送るPC、仕事を実行するPCを選びます。PCは「接続PC」で参加を許可してから選んでください。</p>
    <p class="footnote">同じPCを両方の用途や複数のプロジェクトで使えます。利用者は「利用者」で登録できます。Hub管理者も仕事の閲覧にはプロジェクトへの参加が必要です。</p>
    <p id="shared-admin-notice" role="status" aria-live="polite"></p><section id="shared-admin-setup-code" hidden aria-label="今回だけ表示する本人設定コード"><h3>この利用者だけに本人設定コードを渡してください</h3><p id="shared-admin-setup-code-owner"></p><input id="shared-admin-setup-code-value" readonly aria-label="本人設定コード"><p>接続ファイルとは別に安全な方法で渡してください。パスワードは本人が設定します。画面を閉じるとコードは再表示できません。紛失した場合は「本人設定コードを再発行」を押してください。</p><button type="button" class="secondary" id="shared-admin-setup-code-close">コード表示を閉じる</button></section>
    <div class="shared-admin-tabs" role="tablist" aria-label="共同利用の管理">${adminTabs.map(([id, label]) => `<button type="button" role="tab" data-sa-tab="${id}" aria-selected="${id === tab}" aria-controls="shared-admin-content">${label}</button>`).join("")}</div>
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
        case "users": return action("create_user_with_setup_code") + action("create_user", "パスワードを指定して追加（管理者も可）") + s.users.map(row => card(row.display_name, `<p>${esc(row.username)} · ${row.administrator ? "Hub管理者" : "利用者"} · ${row.disabled ? "利用停止" : row.setup_pending ? "本人の初回設定待ち" : "有効"}</p>`, action("update_user", "編集", row.user_id) + (row.setup_pending ? action("reissue_setup_code",undefined,row.user_id)+action("revoke_setup_code",undefined,row.user_id) : action("set_password", "パスワード再設定", row.user_id)) + action("delete_user", "削除", row.user_id))).join("");
        case "groups": return action("create_group") + s.groups.map(row => card(row.label, `<p>${row.disabled ? "利用停止" : "有効"}</p><p>所属: ${s.group_memberships.filter(m => m.group_id === row.id).map(m => esc(user(m.user_id))).join("、") || "なし"}</p>`, action("update_group", "編集", row.id) + action("group_member", "所属を設定", row.id) + action("delete_group", "削除", row.id))).join("");
        case "projects": return action("save_project", "プロジェクトを追加") + s.projects.map(row => {
          const assignments=(s.project_devices ?? []).filter(d=>d.project_id===row.id);
          const state=(value:string)=>({waiting_setup:"このPCの実行設定待ち",pending:"作業フォルダーを準備中",ready:"作業フォルダー作成済み（Desktopでサンプルを実行してください）",failed:"作業フォルダーを作成できません",not_selected:"実行しない"})[value] ?? value;
          const controls=row.legacy_devices_unrestricted ? "従来の設定（登録したPCから操作可）" : assignments.filter(d=>d.can_control).map(d=>runner(d.device_id)).join("、") || "未選択";
          const executors=assignments.filter(d=>d.can_execute).map(d=>`<p>${esc(runner(d.device_id))}: ${esc(state(d.preparation_state))}${d.error ? ` — ${esc(d.error)}` : ""}${d.preparation_state==="failed" ? `<button type="button" class="secondary" data-sa-retry-project="${esc(row.id)}" data-sa-device="${esc(d.device_id)}">フォルダー作成を再試行</button>` : ""}</p>`).join("") || "<p>未選択</p>";
          return card(row.label, `<p>${row.archived ? "保管中" : "利用中"}</p><p>参加者: ${s.project_memberships.filter(m=>m.project_id===row.id).map(m=>`${esc(user(m.user_id))}（${roleLabel(m.role)}）`).join("、") || "未選択"}</p><p>操作に使うPC: ${esc(controls)}</p><div>仕事を実行するPC${executors}</div>`, action("save_project","設定を編集",row.id)+action("delete_project","削除",row.id));
        }).join("");
        case "environments": return `<h3>作業フォルダーのひな形から追加</h3><p>端末とひな形を引き継いで、名前と利用するプロジェクトを設定できます。保存すると、登録とフォルダー作成をまとめて依頼します。</p>${s.templates.map(t => card(t.label, `<p>${esc(runner(t.runner_id))}</p>`, `<button type="button" class="secondary" data-sa-template="${esc(t.id)}" data-sa-runner="${esc(t.runner_id)}">このひな形で追加</button>`)).join("") || '<p>提供側のDesktopで「このPCで仕事を受け付ける」から作業フォルダーのひな形を公開してください。</p>'}<p>${action("create_environment", "実行先を指定して追加")}</p><h3>登録した仕事の実行先</h3>` + s.environments.map(row => card(row.label, `<p>${esc(runner(row.runner_id))} · ${row.enabled ? "受付対象" : "新規受付停止"}</p><p>共有枠: ${esc(row.resource_id)} / 利用するプロジェクト: ${row.project_ids.map(id => esc(s.projects.find(p => p.id === id)?.label ?? id)).join("、")}</p>`, action("update_environment", "編集", row.id) + action("request_provision", "フォルダー作成・再試行", row.id) + action("delete_environment", "削除", row.id) + `<p>${s.provisioning.filter(v => v.environment_id === row.id).map(v => `${esc(({ pending: "フォルダー作成待ち", ready: "フォルダーの準備完了", failed: "フォルダー作成に失敗" } as Record<string, string>)[v.state] ?? v.state)} ${esc(v.error ?? "")}`).join("")}</p>`)).join("");
        case "resources": return `<p>同じPCやソフトウェアを使う実行先は、同時利用の枠を共用してください。ソフトウェアのライセンスで許可された数も確認してください。</p>${action("set_resource")}` + s.resources.map(row => card(row.id, `<p>同時利用上限: ${row.capacity}</p><p>${s.environments.filter(e => e.resource_id === row.id).map(e => esc(e.label)).join("、") || "利用する実行先なし"}</p>`, action("set_resource", "枠を変更", row.id) + action("delete_resource", "削除", row.id))).join("");
        case "maintenance": return `<p>実行を引き受けたPCと仕事を表示します。実行状態が不明な場合は、そのPCの処理と外部ジョブが止まったことを確認してから「停止確認を記録」を押してください。確認が済むまで、その仕事が使っている枠は再利用できません。</p>` + (s.maintenance.map(row => card(runner(row.runner_id), `<p>状態: ${esc(({assigned:"実行PCへの割り当て済み",running:"実行中",stop_requested:"停止要求中"} as Record<string,string>)[row.state] ?? row.state)}</p><p>${esc(row.uncertainty_reason ?? "")}</p><p class="shared-admin-mono">仕事 ${esc(row.job_id)}<br>試行 ${esc(row.attempt_id)}</p>`, action("settle_attempt", "停止確認を記録", row.attempt_id))).join("") || "<p>停止確認の対象はありません。</p>");
        case "storage": return `<p>保存中: ${s.storage.retained_bytes.toLocaleString()} / ${s.storage.limit_bytes.toLocaleString()} バイト<br>ファイル ${s.storage.asset_count}件 · 内容削除済み ${s.storage.purged_asset_count}件</p><p>終了した仕事の保存期間を指定して、古いファイルを削除できます。利用中の入力と、まだ完了していない追加依頼に必要なファイルは残ります。削除は管理者が実行したときだけ行われます。</p>${action("prune_shared_data")}<h3>バックアップと復元</h3><p>Hubを停止し、Hubの端末上でバックアップを作成してください。バックアップには利用者情報・証明書・仕事の内容が含まれます。復元前に、関係する実行PCのRunnerと、そのRunnerが起動した処理を停止したことを各PCで確認してください。復元先には、まだ存在しない新しいフォルダーを指定します（親フォルダーは作成済みである必要があります）。復元後はPCの接続と新しい仕事の受付が停止します。実行状態が不明な仕事の停止確認を記録してから、接続と受付を再開してください。</p><pre>moyai-hub --backup-dir BACKUP --data-dir HUBDATA\nmoyai-hub --restore-from BACKUP --data-dir NEW_DIR --acknowledge-runners-stopped</pre>`;
      }
    })();
    if (tab === "maintenance") rows += `<p>対象: ${s.maintenance_total}件</p><div class="shared-admin-actions">${s.maintenance_next_before ? '<button type="button" id="shared-admin-maintenance-next" class="secondary">次の復旧対象</button>' : ""}${maintenanceBefore ? '<button type="button" id="shared-admin-maintenance-latest" class="secondary">先頭の復旧対象</button>' : ""}</div>`;
    const container = node("shared-admin-content");
    // Preserve focused row controls until focus leaves the list.
    if (!container.contains(document.activeElement)) container.innerHTML = rows;
    for (const button of host.querySelectorAll<HTMLElement>("[data-sa-tab]")) button.setAttribute("aria-selected", String(button.dataset.saTab === tab));
  }
  function open(kind: AdminOperation, id: string, template?: { runner_id: string; id: string }) {
    if (!snapshot || busy || pending) return;
    clearSetupCode();
    let draft: AdminDraft = { id, user_id: id, capacity: "1", enabled: true, member: true, role: "contributor" };
    if (kind === "save_project") {
      const project=snapshot.projects.find(row=>row.id===id);
      draft={ id:project?.id ?? `project-${crypto.randomUUID()}`, label:project?.label ?? "", archived:project?.archived ?? false,
        controller_device_ids:project?.legacy_devices_unrestricted ? devices.filter(d=>!d.revoked).map(d=>d.device_id) : (snapshot.project_devices ?? []).filter(d=>d.project_id===id&&d.can_control).map(d=>d.device_id),
        runner_device_ids:(snapshot.project_devices ?? []).filter(d=>d.project_id===id&&d.can_execute).map(d=>d.device_id) };
      for(const member of snapshot.project_memberships.filter(m=>m.project_id===id)) draft[`participant:${member.user_id}`]=member.role;
    }
    if (["create_group", "create_project", "create_environment"].includes(kind)) draft.id = `${kind.slice(7)}-${crypto.randomUUID()}`;
    if (template) {
      const current = snapshot.templates.find(t => t.id === template.id && t.runner_id === template.runner_id);
      if (!current || !devices.some(d => d.device_id === template.runner_id && !d.revoked)) { notice("ひな形と端末の最新情報を取得してください。", true); return; }
      Object.assign(draft, environmentDefaults(snapshot, current.runner_id, `resource-${crypto.randomUUID()}`), { template_id: current.id, label: current.label });
    }
    if (["update_user", "set_password", "delete_user"].includes(kind)) Object.assign(draft, snapshot.users.find(row => row.user_id === id));
    if (kind === "update_group") Object.assign(draft, snapshot.groups.find(row => row.id === id));
    if (kind === "update_project") Object.assign(draft, snapshot.projects.find(row => row.id === id));
    if (kind === "group_member") draft.group_id = id;
    if (kind === "project_member" || kind === "group_project_role") draft.project_id = id;
    if (kind === "update_environment") Object.assign(draft, snapshot.environments.find(row => row.id === id));
    if (kind === "set_resource" && id) Object.assign(draft, snapshot.resources.find(row => row.id === id));
    if (kind === "settle_attempt") Object.assign(draft, snapshot.maintenance.find(row => row.attempt_id === id));
    if (kind === "request_provision") {
      const environment = snapshot.environments.find(row => row.id === id);
      const provision = snapshot.provisioning.find(row => row.environment_id === id);
      Object.assign(draft, { environment_id: id, runner_id: environment?.runner_id ?? "", generation: provision?.generation ?? 0, template_id: provision?.template_id ?? "" });
    }
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
    const environmentFields = () => {
      const creating = kind === "create_environment";
      const existingResource = snapshot!.resources.find(r => r.id === d.resource_id);
      const resourceChoices = snapshot!.resources.map(r => `<option value="${esc(r.id)}">${r.capacity}枠 · ${esc(snapshot!.environments.filter(e => e.resource_id === r.id).map(e => e.label).join("、"))}</option>`).join("");
      return field("label", "実行先の名前")
        + select("runner_id", "仕事を実行する端末", devices.filter(row => !row.revoked).map(row => [row.device_id, row.label]))
        + (creating ? select("template_id", "作業フォルダーのひな形", snapshot!.templates.filter(t => t.runner_id === d.runner_id).map(t => [t.id, t.label]), "作成を依頼しない（端末で設定済み）") + '<p class="footnote">ひな形を選ぶと、端末で許可された場所にフォルダーを作成します。任意の保存先パスを入力する必要はありません。</p>' : check("enabled", "新規受付の対象にする"))
        + `<label>同時利用の枠<input name="resource_id" id="shared-admin-resource_id" list="shared-admin-resource-options" value="${esc(d.resource_id)}" required autocomplete="off"><datalist id="shared-admin-resource-options">${resourceChoices}</datalist><small>同じ端末・ソフトを使う実行先は、既存の枠を選んで共用します。一つに決まる場合は入力済みです。別の枠を作る場合だけ新しい識別名を指定してください。</small></label>`
        + (creating ? `<label>同時に使える数<input name="capacity" id="shared-admin-capacity" type="number" min="1" max="128" value="${esc(existingResource?.capacity ?? d.capacity)}" ${existingResource ? "readonly" : ""} required><small>既存の枠の上限はそのまま使います。上限の変更は「同時利用の枠」で行います。</small></label>` : "")
        + `<fieldset><legend>この実行先を使えるプロジェクト（選択必須）</legend>${snapshot!.projects.filter(p => !p.archived).map(p => `<label class="shared-admin-check"><input name="project_ids" type="checkbox" value="${esc(p.id)}" ${Array.isArray(d.project_ids) && d.project_ids.includes(p.id) ? "checked" : ""}>${esc(p.label)}</label>`).join("") || '<p>先に「プロジェクト」で追加してください。</p>'}</fieldset>`
        + field("id", "実行先の識別名", { readonly: !creating, hint: creating ? "自動入力済みです。そのまま使えます。保存後は変更できません。" : "仕事で使用した実行端末と共有枠は変更できません。移動先には新しい実行先を追加してください。" });
    };
    const fields = (() => {
      switch (kind) {
        case "save_project": {
          const selected=(key:string,id:string)=>Array.isArray(d[key])&&(d[key] as string[]).includes(id);
          const pcRows=devices.filter(pc=>!pc.revoked || selected("controller_device_ids",pc.device_id)||selected("runner_device_ids",pc.device_id)).map(pc=>`<tr><th scope="row">${esc(pc.label)}${pc.revoked ? "（登録失効）" : ""}</th><td><input type="checkbox" name="controller_device_ids" value="${esc(pc.device_id)}" aria-label="${esc(pc.label)}を操作に使う" ${selected("controller_device_ids",pc.device_id)?"checked":""}></td><td><input type="checkbox" name="runner_device_ids" value="${esc(pc.device_id)}" aria-label="${esc(pc.label)}で仕事を実行する" ${selected("runner_device_ids",pc.device_id)?"checked":""}></td></tr>`).join("");
          return field("label","プロジェクト名")+`<fieldset><legend>参加する利用者</legend>${snapshot!.users.map(person=>select(`participant:${person.user_id}`,`${person.display_name}${person.disabled?"（利用停止）":""}`,roles,"参加しない")).join("") || '<p>「利用者」でアカウントを追加してください。</p>'}</fieldset><fieldset><legend>使うPC</legend><p>同じPCを両方に選べます。実行するPCでは初回だけ保存先と権限の確認が必要です。</p><table class="shared-admin-pc-table"><thead><tr><th>PC</th><th>操作に使う</th><th>仕事を実行する</th></tr></thead><tbody>${pcRows}</tbody></table>${!pcRows?'<p>「接続PC」でPCの参加申請を許可してください。</p>':""}</fieldset><p>保存すると選択したPCへ反映します。準備中や接続確認待ちはプロジェクトに表示されます。</p>`+check("archived","プロジェクトを保管する（新しい仕事を受け付けない）");
        }
        case "retry_project_device": return `<p>${esc(runner(String(d.device_id)))} の実行準備をもう一度依頼します。保存先の問題は、そのPCで修正してください。</p>`;

        case "create_user_with_setup_code": return field("username", "ログイン名") + field("display_name", "表示名") + "<p>本人がDesktopでパスワードを設定するための、一度限り・24時間のコードを発行します。一般利用者として追加します。</p>";
        case "reissue_setup_code": return `<p>${esc(user(String(d.user_id)))} の以前の本人設定コードを失効し、新しいコードを発行します。</p>`;
        case "revoke_setup_code": return `<p>${esc(user(String(d.user_id)))} の未使用の本人設定コードを失効します。再び設定するには新しいコードを発行してください。</p>`;
        case "create_user": return field("username", "ログイン名") + field("display_name", "表示名") + field("password", "初期パスワード", { type: "password", hint: "12文字以上。利用者へ安全な方法で伝えてください。" }) + check("administrator", "Hub管理者にする");
        case "update_user": return `<p>${esc(d.username)}</p>` + field("display_name", "表示名") + (snapshot!.users.some(person => person.user_id === d.user_id && person.setup_pending)
          ? '<label class="shared-admin-check"><input name="administrator" type="checkbox" disabled data-fixed-disabled>Hub管理者にする</label><p class="footnote">本人の初回パスワード設定が済んでから管理者に変更できます。</p>'
          : check("administrator", "Hub管理者にする")) + check("disabled", "利用を停止する");
        case "set_password": return `<p>${esc(user(String(d.user_id)))}</p>` + field("password", "新しいパスワード", { type: "password", hint: "既存のログインを失効させます。" });
        case "create_group": case "create_project": return field("label", "表示名") + field("id", "識別名", { hint: "自動入力済みです。そのまま使えます。半角英数字・ハイフン等に変更することもできますが、保存後は変更できません。" });
        case "update_group": return field("id", "識別名", { readonly: true }) + field("label", "表示名") + check("disabled", "グループ経由の利用を停止する");
        case "update_project": return field("id", "識別名", { readonly: true }) + field("label", "表示名") + check("archived", "保管状態にする");
        case "group_member": return groupSelect() + userSelect() + check("member", "このグループへ所属させる");
        case "project_member": return projectSelect() + userSelect() + select("role", "直接指定する役割", roles, "直接の所属を解除");
        case "group_project_role": return projectSelect() + groupSelect() + select("role", "グループに与える役割", roles, "グループの役割を解除");
        case "create_environment": case "update_environment": return environmentFields();
        case "set_resource": return field("id", "同時利用の枠の識別名", { readonly: Boolean(d.id) }) + field("capacity", "同時利用枠", { type: "number" });
        case "request_provision": return field("environment_id", "フォルダーを作成する実行先", { readonly: true }) + select("template_id", "作業フォルダーのひな形", snapshot!.templates.filter(t => t.runner_id === d.runner_id).map(t => [t.id, t.label])) + `<p>端末の管理者が指定した親フォルダー内に、この実行先専用のフォルダーを作成します。</p>`;
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
      if (["project_ids","controller_device_ids","runner_device_ids"].includes(field.name)) editor.draft[field.name] = data.getAll(field.name).map(String);
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
      const issued = result.issued_setup_code;
      delete result.issued_setup_code;
      pending = null; editor = null; maintenanceBefore = null; accept(result); renderEditor(); notice("設定を保存しました。");
      if (issued) {
        node("shared-admin-setup-code").hidden = false;
        node("shared-admin-setup-code-owner").textContent = `${issued.username} · 有効期限 ${new Date(issued.expires_at_ms).toLocaleString()}`;
        node<HTMLInputElement>("shared-admin-setup-code-value").value = issued.code;
        node("shared-admin-setup-code").scrollIntoView();
      }
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
        target.kind === "save_project" ? invoke<NetworkSnapshot>("hub_network_snapshot") : Promise.resolve(null),
      ]);
      if (disposed || expected !== serial || editor !== target) return;
      if (network) devices = network.devices;
      accept(latest);
      target.comparison = compareAdminEdit(target.kind, target.draft, target.baseline, latest, devices);
      const comparison = target.comparison;
      node("shared-admin-comparison").innerHTML = `<h4>保存中の設定と今回の入力</h4><p>まだ保存していません。異なる項目を確認してください。</p>${comparison.rows.length ? `<div class="table-wrap"><table><thead><tr><th>項目</th><th>現在保存されている値</th><th>今回の入力</th></tr></thead><tbody>${comparison.rows.map(row => `<tr class="${row.saved === row.input ? "" : "shared-admin-changed"}"><th scope="row">${esc(row.label)}</th><td>${esc(row.saved)}</td><td>${esc(row.input)}</td></tr>`).join("")}</tbody></table></div>` : ""}${comparison.blocked ? `<p role="status">${esc(comparison.blocked)}</p>` : '<p>今回の入力を使う場合は編集を続け、最後に保存してください。最新の値へ自動で置き換えることはありません。</p><button id="shared-admin-accept-comparison" type="button" class="secondary">比較した内容で編集を続ける</button>'}`;
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
    if (target.id === "shared-admin-setup-code-close") clearSetupCode();
    if (target.dataset.saTab && !pending) { clearSetupCode(); tab = target.dataset.saTab as SharedAdminTab; editor = null; renderRows(); renderEditor(); lock(); }
    if (target.dataset.saRetryProject && target.dataset.saDevice && snapshot && !pending) {
      editor={kind:"retry_project_device",draft:{project_id:target.dataset.saRetryProject,device_id:target.dataset.saDevice},revision:snapshot.revision,conflict:false,baseline:snapshot,comparison:null}; renderEditor(); lock();
    }
    if (target.dataset.saOperation) open(target.dataset.saOperation as AdminOperation, target.dataset.saId ?? "");
    if (target.dataset.saTemplate && target.dataset.saRunner) open("create_environment", "", { runner_id: target.dataset.saRunner, id: target.dataset.saTemplate });
    if (target.id === "shared-admin-close" && !pending) { editor = null; renderEditor(); }
    if (target.id === "shared-admin-retry") void save(true);
    if (target.id === "shared-admin-refresh") void refresh();
    if (target.id === "shared-admin-preview") void previewRetention();
    if (target.id === "shared-admin-maintenance-next" && !editor) { maintenanceBefore = snapshot?.maintenance_next_before ?? null; void refresh(); }
    if (target.id === "shared-admin-maintenance-latest" && !editor) { maintenanceBefore = null; void refresh(); }
  });
  host.addEventListener("input", event => {
    if ((event.target as HTMLElement).id === "shared-admin-cutoff" && editor) { delete editor.draft.before_ms; editor.draft.preview_confirmed = false; }
    if ((event.target as HTMLElement).id === "shared-admin-resource_id" && editor?.kind === "create_environment") {
      const existing = snapshot?.resources.find(r => r.id === (event.target as HTMLInputElement).value);
      const capacity = node<HTMLInputElement>("shared-admin-capacity");
      capacity.readOnly = Boolean(existing);
      if (existing) capacity.value = String(existing.capacity);
    }
  });
  host.addEventListener("change", event => {
    if ((event.target as HTMLElement).id !== "shared-admin-runner_id" || editor?.kind !== "create_environment" || !snapshot || busy || pending) return;
    captureEditor();
    Object.assign(editor.draft, environmentDefaults(snapshot, String(editor.draft.runner_id), `resource-${crypto.randomUUID()}`));
    renderEditor(); lock(); node<HTMLSelectElement>("shared-admin-runner_id").focus();
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
  function clearSetupCode() { node<HTMLInputElement>("shared-admin-setup-code-value").value=""; node("shared-admin-setup-code-owner").textContent=""; node("shared-admin-setup-code").hidden=true; }
  return { refresh, updateDevices(value) { devices = value; }, openProject(id = "") { if (pending || busy) return; tab="projects"; renderRows(); open("save_project",id); host.scrollIntoView(); }, openUser() { if (pending || busy) return; tab="users"; renderRows(); open("create_user_with_setup_code",""); host.scrollIntoView(); } };
}
