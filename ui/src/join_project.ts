import { invokeSharedAdmin, ManagementFailure, onAuthenticationChange } from "./management.ts";
import type { NetworkSnapshot, NetworkClientAction } from "./network_state.ts";
import type { SharedAdminSnapshot } from "./shared_admin_state.ts";

const esc = (v: unknown) => String(v ?? "").replace(/[&<>"']/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"})[c]!);
/** PC registration and project settings are saved separately. Keep the join proof if only registration succeeds. */
export function attachJoinProject(host: HTMLElement, changed: () => void) {
  let network: NetworkSnapshot | null = null, shared: SharedAdminSnapshot | null = null;
  let request: Record<string, unknown> | null = null;
  let busy = false, uncertain = false, disposed = false;
  const dialog = document.createElement("dialog");
  dialog.id = "join-project-dialog";
  dialog.setAttribute("aria-labelledby", "join-project-title");
  host.append(dialog);
  const node = <T extends HTMLElement>(id: string) => dialog.querySelector<T>(`#${id}`)!;
  function lock() {
    for (const el of dialog.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLButtonElement>("input,select,button")) el.disabled = busy || (uncertain && el.id !== "join-project-save" && el.id !== "join-project-close");
    const hasProject = Boolean(node<HTMLSelectElement>("join-project-project").value);
    for (const id of ["join-project-control", "join-project-execute"])
      node<HTMLInputElement>(id).disabled = busy || uncertain || !hasProject;
    if (!uncertain) node("join-project-save").textContent = hasProject ? "参加と用途を許可" : "参加を許可";
  }
  document.addEventListener("click", event => {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>("button[data-network-action]");
    if (!button || button.disabled || disposed) return;
    const action = JSON.parse(button.dataset.networkAction!) as NetworkClientAction;
    if (action.command !== "hub_network_approve") return;
    event.preventDefault(); event.stopPropagation();
    const pending = network?.join_requests.find(r => r.request_id === action.args.requestId);
    if (!pending) return;
    request = { requestId: pending.request_id, keySha256: pending.key_sha256, label: pending.label,
      expectedNetworkRevision: action.args.expectedRevision, expectedSharedRevision: shared?.revision ?? "0", commandId: crypto.randomUUID(), project: null };
    uncertain = false;
    dialog.innerHTML = `<form id="join-project-form"><h2 id="join-project-title">PCの参加を許可</h2><p>下の申請情報でWindowsユーザー名やIPアドレスなどを確かめ、分かりやすいPC名を付けてください。Windowsユーザー名は確認用です。同じ名前でも別のPCとして登録します。</p><p>Windowsユーザー名: ${esc(pending.windows_username || "未取得")}</p><p class="network-mono">申請IP: ${esc(pending.observed_ip)}<br>申請ID: ${esc(pending.request_id)}<br>公開鍵 SHA-256: ${esc(pending.key_sha256)}</p><label>PC名<input id="join-project-label" value="${esc(pending.label)}" maxlength="160" required></label><label>このPCを追加するプロジェクト<select id="join-project-project"><option value="">PCの参加だけ許可する</option>${shared?.projects.filter(p => !p.archived && !p.legacy_devices_unrestricted).map(p => `<option value="${esc(p.id)}">${esc(p.label)}</option>`).join("") ?? ""}</select></label><div class="network-actions"><label><input type="checkbox" id="join-project-control">操作に使う</label><label><input type="checkbox" id="join-project-execute">仕事を実行する</label></div><p class="footnote">同じPCを両方に使えます。実行するPCではDesktopのAI・保存先・権限の確認が必要です。操作PCでは、Desktopを開くと使えるプロジェクトが表示されます。</p><p id="join-project-error" role="status"></p><div class="network-actions"><button type="submit" id="join-project-save" class="primary">参加と用途を許可</button><button type="button" id="join-project-close" class="secondary">閉じる</button></div></form>`;
    lock(); dialog.showModal();
  }, true);
  dialog.addEventListener("change", event => {
    if ((event.target as HTMLElement).id !== "join-project-project" || busy || uncertain) return;
    if (!node<HTMLSelectElement>("join-project-project").value)
      for (const id of ["join-project-control", "join-project-execute"]) node<HTMLInputElement>(id).checked = false;
    lock();
  });
  dialog.addEventListener("click", event => { if ((event.target as HTMLElement).id === "join-project-close" && !busy) dialog.close(); });
  dialog.addEventListener("cancel", event => { if (busy) event.preventDefault(); });
  dialog.addEventListener("submit", event => {
    event.preventDefault(); if (busy || !request || disposed) return;
    if (!uncertain) {
      const id = node<HTMLSelectElement>("join-project-project").value;
      const can_control = node<HTMLInputElement>("join-project-control").checked;
      const can_execute = node<HTMLInputElement>("join-project-execute").checked;
      if (id && !can_control && !can_execute) { node("join-project-error").textContent = "このプロジェクトで使う用途を選択してください。"; return; }
      request.label = node<HTMLInputElement>("join-project-label").value.trim();
      request.project = id ? { id, can_control, can_execute } : null;
    }
    busy = true; lock();
    void invokeSharedAdmin<{device_id: string; project_saved: boolean; error: string | null; snapshot: SharedAdminSnapshot}>("hub_join_project", request)
      .then(result => {
        if (disposed) return;
        shared = result.snapshot;
        if (result.project_saved) { dialog.close(); request = null; changed(); return; }
        // The PC is already registered. Keep the exact join identity and ask
        // explicitly before retrying project purposes at the returned revision.
        request!.expectedSharedRevision = result.snapshot.revision;
        request!.commandId = crypto.randomUUID();
        uncertain = true;
        node("join-project-error").textContent = `PCは登録済みです（${result.device_id}）。プロジェクト設定は未完了です。表示中の用途を現在のプロジェクトへ追加するには、もう一度保存してください。${result.error ?? ""}`;
        node("join-project-save").textContent = "登録済みPCの用途を保存";
        changed();
      }).catch(error => {
        if (disposed) return;
        uncertain = !(error instanceof ManagementFailure && [400,403,404,409].includes(error.status));
        node("join-project-error").textContent = uncertain ? "受付結果が未確認です。同じ内容で結果を確認してください。" : `${error instanceof Error ? error.message : String(error)} 閉じて最新情報を取得してください。`;
        if (uncertain) node("join-project-save").textContent = "同じ内容で結果を確認";
      }).finally(() => { busy = false; if (!disposed) lock(); });
  });
  onAuthenticationChange(session => { if (!session?.authentication.user) { disposed = true; request = null; dialog.remove(); } });
  return { updateNetwork(value: NetworkSnapshot) { network = value; }, updateShared(value: SharedAdminSnapshot) { shared = value; } };
}
