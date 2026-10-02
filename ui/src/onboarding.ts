import type { NetworkSnapshot } from "./network_state.ts";
import type { SharedAdminSnapshot } from "./shared_admin_state.ts";
import { onboardingNextStep, projectPreparation } from "./onboarding_state.ts";
import { projectPreparationLabel } from "./shared_admin_state.ts";
import { reconcileStableRows } from "./stable_rows.ts";
import "./onboarding.css";

const esc = (v: unknown) => String(v ?? "").replace(/[&<>"']/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"})[c]!);
function rowKey(node: Element): string | null {
  if (node.id) return `${node.tagName}:id:${node.id}`;
  if (node.hasAttribute("data-onboarding-edit")) return `${node.tagName}:project:${node.getAttribute("data-onboarding-project")}`;
  for (const name of ["data-onboarding-first", "href"])
    if (node.hasAttribute(name)) return `${node.tagName}:${name}:${node.getAttribute(name)}`;
  return null;
}
export function attachOnboarding(host: HTMLElement, actions: { project: (id?: string) => void }) {
  let shared: SharedAdminSnapshot | null = null, network: NetworkSnapshot | null = null;
  let projectId = "", modelCount: number | null = null;
  host.innerHTML = `<div class="section-title"><div><p class="eyebrow">PCの登録から最初の依頼まで</p><h2>チームの初期設定</h2></div><a href="#device-network">接続設定へ</a></div><p class="subtle">HubにモデルとPCを登録し、プロジェクトで使うPCを選びます。実行PCの準備ができたら、Desktopから依頼できます。</p><label>設定するプロジェクト<select id="onboarding-project"><option value="">まだプロジェクトがありません</option></select></label><div class="network-actions"><button id="onboarding-new-project" class="secondary">プロジェクトを作る</button></div><div id="onboarding-steps" class="onboarding-steps" aria-live="polite"></div><details><summary>使うAIと、起動しておくPC</summary><p>実行PCの「設定」→「AIの接続」でHubのモデルを選びます。依頼・閲覧だけに使うPCではAI設定は不要です。</p><p>仕事の実行中はHub・実行PC・AIサーバーを起動しておいてください。管理画面を閉じてもHubは動きます。Hub PCのサインアウト・スリープ中は利用できません。</p><a href="#models">共有AIを登録・確認する</a></details><details id="onboarding-first-job"><summary>サンプルを依頼して成果を開く</summary><ol><li>操作PCのDesktopで、このプロジェクトを開きます。</li><li>通常のチャット欄に、たとえば「10・20・30の件数と合計を計算し、日本語で回答してください。結果を作業フォルダーの moyai-sample-result.md に保存して共有してください。ほかのファイルの変更や外部への接続は不要です」と入力します。</li><li>「送信」を押します。使うPCはAIが判断します。</li><li>回答が件数3・合計60になっていることを確認し、成果ファイルを開きます。</li></ol><p>閲覧だけが許可されたPCでは、同じプロジェクトの仕事と成果をDesktopで開いてください。</p></details>`;
  const select = host.querySelector<HTMLSelectElement>("#onboarding-project")!;
  const steps = host.querySelector<HTMLElement>("#onboarding-steps")!;
  steps.tabIndex = -1;
  const next = document.createElement("section");
  next.id = "onboarding-next";
  next.className = "onboarding-next";
  next.tabIndex = -1;
  next.setAttribute("aria-live", "polite");
  next.setAttribute("aria-labelledby", "onboarding-next-title");
  select.closest("label")!.before(next);
  function render() {
    if (!shared) { next.innerHTML = '<h3 id="onboarding-next-title">設定を読み込んでいます。</h3>'; steps.textContent = ""; return; }
    const projects = shared.projects.filter(p => !p.archived);
    if (!projects.some(p => p.id === projectId)) projectId = projects[0]?.id ?? "";
    if (document.activeElement !== select) {
      const html = projects.map(p => `<option value="${esc(p.id)}">${esc(p.label)}</option>`).join("") || '<option value="">まだプロジェクトがありません</option>';
      if (select.innerHTML !== html) select.innerHTML = html;
      select.value = projectId;
    }
    const p = projectPreparation(shared, network, projectId);
    const guidance = onboardingNextStep(shared, network, projectId, modelCount);
    const action = guidance.action;
    const nextAction = !action || guidance.kind === "project" ? "" : action.kind === "link"
      ? `<a href="${esc(action.href)}">${esc(action.label)}</a>`
      : `<button type="button" class="primary" ${action.kind === "project" ? `data-onboarding-edit data-onboarding-project="${esc(projectId)}"` : "data-onboarding-first"}>${esc(action.label)}</button>`;
    const nextHtml = `<p class="eyebrow">次にすること</p><h3 id="onboarding-next-title">${esc(guidance.title)}</h3><dl><div><dt>担当</dt><dd>${esc(guidance.owner)}</dd></div><div><dt>操作する場所</dt><dd>${esc(guidance.location)}</dd></div></dl><p>${esc(guidance.detail)}</p>${nextAction}`;
    reconcileStableRows(next, nextHtml, next, rowKey);
    const name = (id: string) => network?.devices.find(d => d.device_id === id)?.label ?? id;
    const state = projectPreparationLabel;
    const card = (title: string, detail: string, action: string) => `<article><h3>${title}</h3><p>${detail}</p>${action}</article>`;
    const html = card("1. プロジェクト", p.project ? `${esc(p.project.label)}：操作PC ${p.controllers.length} 台・実行PC ${p.executors.length} 台` : "最初のプロジェクトは未作成です。", p.project ? `<button type="button" class="secondary" data-onboarding-edit data-onboarding-project="${esc(projectId)}">PCの用途を設定</button>` : "")
      + card("2. PCを接続", !network ? "接続状態を確認しています。" : network.server.running ? `Hubへの接続を受け付けています。承認待ち ${p.pendingRequests} 台。接続するPCのDesktopでも接続を診断してください。` : "管理者がLANのIPアドレスとファイアウォールを設定し、接続の受付を開始してください。", '<a href="#device-network">接続先と配布ファイル</a> · <a href="#clients">PCの参加と用途を許可</a>')
      + card("3. 操作するPC", p.controllers.length ? p.controllers.map(row => esc(name(row.device_id))).join("、") : p.project?.legacy_devices_unrestricted ? "従来のPC設定です。プロジェクトで現在の用途を確認してください。" : "管理者がこのプロジェクトを操作するPCを指定してください。", p.project ? '<a href="#shared-administration">プロジェクトのPC用途を確認</a>' : "")
      + card("4. 実行するPCとAI", p.executors.length ? p.executors.map(row => `${esc(name(row.device_id))}：${esc(state(row.preparation_state))}${row.error ? `（${esc(row.error)}）` : ""}`).join("<br>") : "管理者が実行PCを指定し、そのPCの担当者がDesktopでAI・保存先・権限を確認してください。", `<p class="footnote">${modelCount === null ? "Hubの登録モデルを確認しています。" : `Hubの登録モデルは${modelCount}件です。`}チームの仕事にはHubに登録したAIが必要です。</p><a href="#models">共有AIを登録・確認する</a>`)
      + card("5. 最初の仕事と成果", "Desktopからサンプルを依頼し、回答と成果ファイルを開いてください。実行結果はDesktopに表示されます。", '<button type="button" class="secondary" data-onboarding-first>サンプルの依頼手順</button>');
    reconcileStableRows(steps, html, steps, rowKey);
  }
  select.addEventListener("change", () => { projectId = select.value; render(); });
  host.addEventListener("click", event => {
    const el = (event.target as HTMLElement).closest<HTMLButtonElement>("button");
    if (!el) return;
    if (el.id === "onboarding-new-project") actions.project();
    if (el.hasAttribute("data-onboarding-edit")) {
      const target = el.dataset.onboardingProject ?? projectId;
      if (target && !shared?.projects.some(project => project.id === target && !project.archived)) return;
      actions.project(target || undefined);
    }
    if (el.hasAttribute("data-onboarding-first")) { const detail = host.querySelector<HTMLDetailsElement>("#onboarding-first-job")!; detail.open = true; detail.scrollIntoView(); }
  });
  render();
  return { updateShared(value: SharedAdminSnapshot) { shared=value; render(); }, updateNetwork(value: NetworkSnapshot) { network=value; render(); }, updateModels(count: number) { modelCount=count; render(); } };
}
