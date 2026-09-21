import type { NetworkSnapshot } from "./network_state.ts";
import type { SharedAdminSnapshot } from "./shared_admin_state.ts";
import { onboardingNextStep, projectPreparation } from "./onboarding_state.ts";
import "./onboarding.css";

const esc = (v: unknown) => String(v ?? "").replace(/[&<>"']/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"})[c]!);
export function attachOnboarding(host: HTMLElement, actions: { project: (id?: string) => void; user: () => void }) {
  let shared: SharedAdminSnapshot | null = null, network: NetworkSnapshot | null = null;
  let projectId = "", modelCount = 0;
  host.innerHTML = `<div class="section-title"><div><p class="eyebrow">利用者の登録から最初の依頼まで</p><h2>チームの初期設定</h2></div><a href="#device-network">接続設定へ</a></div><p class="subtle">管理者は利用者・プロジェクト・PCの用途を設定します。実行PCの担当者はAI・保存先・権限を確認し、利用者はDesktopから仕事を依頼します。途中で閉じても保存済みの設定から続けられます。</p><label>設定するプロジェクト<select id="onboarding-project"><option value="">まだプロジェクトがありません</option></select></label><div class="network-actions"><button id="onboarding-new-project" class="secondary">プロジェクトを作る</button><button id="onboarding-new-user" class="secondary">利用者を追加する</button></div><div id="onboarding-steps" class="onboarding-steps" aria-live="polite"></div><details><summary>使うAIと、起動しておくPC</summary><p>実行PCでは、そのPCに直接設定したAIか、Hubに登録した共有AIを選びます。AIのサーバーは担当者が起動してください。依頼の送信と成果の閲覧だけに使うPCは、AI設定なしで利用できます。</p><p>仕事の実行中は、Hub・実行PC・AIのサーバーを起動しておいてください。管理画面を閉じてもHubは動きます。Hub PCのシャットダウン・サインアウト・スリープ中は利用できません。</p><a href="#models">共有AIを登録する（PCに直接設定したAIなら不要）</a></details><details id="onboarding-first-job"><summary>サンプルを依頼して成果を開く</summary><ol><li>Desktopに自分のアカウントでログインし、このプロジェクトを開きます。Hub管理者も、仕事を扱うプロジェクトの参加者に追加してください。</li><li>実行するPCでAIの接続と保存先・権限を設定します。実行だけを担当するPCでは、本人のアカウントへのログインは不要です。</li><li>Desktopの「接続状況とサンプル」で「サンプルの依頼を入力」を押します。数値10・20・30のサンプルファイルと依頼文が入力されます。内容を確認して「送信」を押してください。</li><li>仕事が終了したら、回答が件数3・合計60になっていることを確認し、成果ファイルを開きます。ここまでできれば、このPCからの依頼と成果の受け取りを確認できます。</li></ol><p>閲覧だけの利用者は、同じプロジェクトの仕事と成果をDesktopで開いてください。会話や成果の閲覧にはプロジェクトの参加権限が必要です。</p></details>`;
  const select = host.querySelector<HTMLSelectElement>("#onboarding-project")!;
  const steps = host.querySelector<HTMLElement>("#onboarding-steps")!;
  const next = document.createElement("section");
  next.id = "onboarding-next";
  next.className = "onboarding-next";
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
    const guidance = onboardingNextStep(shared, network, projectId);
    const action = guidance.action;
    const nextAction = !action ? "" : action.kind === "link"
      ? `<a href="${esc(action.href)}">${esc(action.label)}</a>`
      : `<button type="button" class="primary" ${action.kind === "project" ? `data-onboarding-edit data-onboarding-project="${esc(projectId)}"` : "data-onboarding-first"}>${esc(action.label)}</button>`;
    const nextHtml = `<p class="eyebrow">次にすること</p><h3 id="onboarding-next-title">${esc(guidance.title)}</h3><dl><div><dt>担当</dt><dd>${esc(guidance.owner)}</dd></div><div><dt>操作する場所</dt><dd>${esc(guidance.location)}</dd></div></dl><p>${esc(guidance.detail)}</p>${nextAction}`;
    if (!next.contains(document.activeElement) && next.innerHTML !== nextHtml) next.innerHTML = nextHtml;
    const name = (id: string) => network?.devices.find(d => d.device_id === id)?.label ?? id;
    const state = (value: string) => ({waiting_setup:"実行PCの保存先・権限の確認待ち",pending:"作業フォルダーを準備中",ready:"作業フォルダー作成済み・Desktopでサンプルを実行してください",failed:"作業フォルダーを作成できません。実行PCの設定を確認してください"})[value] ?? value;
    const card = (title: string, detail: string, action: string) => `<article><h3>${title}</h3><p>${detail}</p>${action}</article>`;
    const html = card("1. 利用者とプロジェクト", p.project ? `${esc(p.project.label)}：参加者 ${p.people.length} 人${p.canSubmit ? "・依頼できる担当者あり" : p.people.some(person => person.role !== "reader" && shared?.users.some(user => user.user_id === person.user_id && user.setup_pending)) ? "・本人の初回設定待ち" : "。依頼できる参加者を設定してください"}` : "管理者が最初のプロジェクトと参加者を指定してください。", '<button type="button" class="secondary" data-onboarding-edit>参加者とPC用途を設定</button>')
      + card("2. PCを接続", !network ? "接続状態を確認しています。" : network.server.running ? `Hubへの接続を受け付けています。承認待ち ${p.pendingRequests} 台。接続するPCのDesktopでも接続を診断してください。` : "管理者がLANのIPアドレスとファイアウォールを設定し、接続の受付を開始してください。", '<a href="#device-network">接続先と配布ファイル</a> · <a href="#clients">PCの参加と用途を許可</a>')
      + card("3. 操作するPC", p.controllers.length ? p.controllers.map(row => esc(name(row.device_id))).join("、") : p.project?.legacy_devices_unrestricted ? "従来のPC設定です。プロジェクトで現在の用途を確認してください。" : "管理者がこのプロジェクトを操作するPCを指定してください。", '<a href="#shared-administration">プロジェクトのPC用途を確認</a>')
      + card("4. 実行するPCとAI", p.executors.length ? p.executors.map(row => `${esc(name(row.device_id))}：${esc(state(row.preparation_state))}${row.error ? `（${esc(row.error)}）` : ""}`).join("<br>") : "管理者が実行PCを指定し、そのPCの担当者がDesktopでAI・保存先・権限を確認してください。", `<p class="footnote">Hubの登録モデルは${modelCount}件です。AIをPCに直接設定した場合、Hubへの登録は不要です。選んだAIでサンプルを実行し、結果を確認してください。</p><a href="#models">共有AIを登録する（PCに直接設定したAIなら不要）</a>`)
      + card("5. 最初の仕事と成果", "Desktopからサンプルを依頼し、回答と成果ファイルを開いてください。実行結果はDesktopに表示されます。", '<button type="button" class="secondary" data-onboarding-first>サンプルの依頼手順</button>');
    if (!steps.contains(document.activeElement) && steps.innerHTML !== html) steps.innerHTML = html;
  }
  select.addEventListener("change", () => { projectId = select.value; render(); });
  host.addEventListener("click", event => {
    const el = (event.target as HTMLElement).closest<HTMLButtonElement>("button");
    if (!el) return;
    if (el.id === "onboarding-new-project") actions.project();
    if (el.id === "onboarding-new-user") actions.user();
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
