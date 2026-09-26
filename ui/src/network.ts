import { copyText, invoke, savedMessage } from "./management.ts";
import { NetworkEdits, networkAdmissionStatus, networkCanDistribute, networkIsLocalOnly, networkCertificateStatus, networkClientAction, networkClients, networkDeviceDeletion, networkDeviceStatus, networkDiagnosticMatches, networkDiagnosticStages, networkDiagnosticTarget, networkGatewayPort, networkSetupStep, networkSubject } from "./network_state.ts";
import type { NetworkClientAction, NetworkClientFilter, NetworkDiagnostics, NetworkSnapshot, NetworkSubject } from "./network_state.ts";
import "./network.css";
import { reconcileNetworkRows } from "./network_rows.ts";
import { attachFirewall } from "./firewall.ts";
import type { FirewallStatus } from "./firewall.ts";

const escape = (value: unknown) => String(value ?? "").replace(/[&<>"']/g, char => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[char]!);
const when = (value: number | null) => value ? new Date(value).toLocaleString("ja-JP") : "未確認";

export function attachDeviceNetwork(host: HTMLElement, clientsHost: HTMLElement, interactionActive: () => boolean,
    onSnapshot: (snapshot: NetworkSnapshot) => void): { refresh: () => Promise<void> } {
    host.innerHTML = `<div class="section-title"><div><p class="eyebrow">初めての接続・端末の追加</p><h2 id="network-heading">PCの接続</h2></div><button id="network-refresh" class="secondary">最新情報を取得</button></div>
      <p class="subtle">最初の1台で接続を確かめてから、同じ接続ファイルをチームに配ります。下の1〜4の順に進めてください。</p>
      <aside class="network-next-action" aria-label="次の操作"><strong id="network-next-title">接続状態を読み込んでいます</strong><p id="network-next-detail"></p><a id="network-next-link" href="#network-step-start">接続先を設定する</a></aside>
      <p id="network-notice" class="field-status" role="status" aria-live="polite"></p>
      <div class="network-grid network-setup-grid"><section id="network-step-start" class="network-card"><h3>1. 接続先を設定してHubを開始 <span id="network-server-status" class="badge gray">停止中</span></h3><p class="footnote">このPCでHubとモデル通信の受付を起動します。Windowsの受信許可は、実際のポートが決まった後に手順2で設定します。</p>
        <label>共有するLANの候補<select id="network-lan"><option value="">候補を確認しています…</option></select></label><p id="network-lan-status" class="footnote"></p><button id="network-lan-refresh" type="button" class="secondary">LAN候補を再取得</button>
        <p class="footnote">モデル通信は保存済みポートで開始します。変更する場合だけ下を開いて保存してください。</p>
        <details id="network-gateway-settings" data-details-key="network-gateway-settings"><summary>モデル通信ポートを変更する（通常は変更不要）</summary><form id="network-settings-form"><label>Gatewayポート<input id="network-gateway-port" inputmode="numeric" autocomplete="off" placeholder="9472" /></label><p class="footnote">表示中の保存済み設定で開始します。通常は変更・保存せずに下の開始ボタンへ進めます。新規Hubの標準は9472です。変更するときだけ「ポート設定を保存」を押してください。稼働中の変更は次回開始時に適用し、空欄または0は再開始時にポートが変わる自動選択です。</p><p class="footnote">Gatewayはチームの仕事で使うAIとの通信を中継し、Hubと一緒に起動します。使うAIをHubへ登録し、実行PCのDesktopで接続を確認してください。</p><button type="submit" class="secondary">ポート設定を保存</button></form></details>
        <form id="network-server-form"><div class="network-fields"><label>このHubのIP<input id="network-ip" placeholder="例: 192.168.1.10" required autocomplete="off" spellcheck="false" aria-describedby="network-ip-help" /></label><label>Hubポート<input id="network-port" value="9471" inputmode="numeric" required /></label></div>
        <p id="network-ip-help" class="footnote">チームで使う場合は、同じPCのDesktopも最初からLAN側IPへ接続してください。複数LAN・VPNがある場合は参加PCから届くものを選びます。127.0.0.1はこのPCだけの試用向けです。登録済みDesktopのURL変更は、同じHub・同じCAの接続ファイルで接続先を確認して行います。別Hub・CAへの差し替えは対象外です。</p>
        <p class="footnote">一度開始できたIPとポートは保存されます。次回はStart-Hub.cmdから同じ設定で再開できます。モデル接続とGatewayもまとめて開始・停止します。</p><p id="network-hosting-error" class="field-status error" role="status" hidden></p><div class="network-actions"><button id="network-start" class="primary">接続の受付を開始</button><button id="network-stop" type="button" class="danger" hidden>接続の受付を停止</button><button id="network-diagnose" type="button" class="secondary">接続状態を診断</button></div></form>
        <section id="network-diagnostic-result" aria-label="接続診断の結果" aria-live="polite" hidden></section>
        <details data-details-key="network-connection-details"><summary>接続と証明書の詳細</summary><p id="network-identity" class="network-mono"></p><p id="network-ca" class="network-mono"></p>
          <p id="network-gateway-bind" class="network-mono"></p>
          <p id="network-certificate-status" class="footnote" role="status"></p><dl id="network-certificate-times" class="network-certificate-times"></dl>
          <button id="network-renew-certificate" type="button" class="secondary">証明書を今すぐ更新</button><p class="footnote">HubとGatewayの証明書を更新します。新しい接続から適用し、進行中の通信は継続します。</p>
        </details></section>
      <section id="network-step-firewall" class="network-card"><h3>2. Windowsで受信を許可</h3><p>未設定の場合は、下に表示する変更内容を確認して接続を許可します。管理者へ作業を依頼する場合は、同じ内容の設定スクリプトを保存できます。</p><p id="network-firewall-prerequisite" class="footnote"></p>
        <div id="network-firewall-native"></div><details><summary>受信許可の設定を管理者へ依頼</summary><p>次の実行ファイルとHub・Gatewayの受信TCPポートに限定します。利用する接続元PCまたはサブネットと、社内LANのネットワークプロファイルを管理者が確認してください。</p><pre id="network-firewall-info"></pre><button id="network-firewall-copy" type="button" class="secondary">設定依頼をコピー</button><p class="footnote">コピーした内容をPC管理者へ渡し、設定を依頼してください。設定後は参加するDesktopから接続を診断します。許可が引き続き有効になるよう、更新後もHubの実行ファイルは同じ場所に置いてください。</p></details><p><a href="#network-step-file">受信許可を設定したら、接続ファイルを配布する →</a></p></section>
      <section id="network-step-file" class="network-card"><h3>3. 接続ファイルの配布</h3>
        <p class="subtle">hub-config.moyai-joinを保存して、まず接続を試すPCへ渡します。そのPCにmoyAIをセットアップし、ファイルをダブルクリックしてDesktopで接続先を確認してください。Desktopの「moyAI Hub → PCの接続」からも読み込めます。IPや証明書をPCごとに入力する必要はありません。</p>
        <div class="network-actions"><button id="network-save-config" type="button" class="primary">接続ファイルを保存</button><button id="network-copy-config" type="button" class="secondary">接続設定をコピー</button></div>
        <p id="network-distribution-status" role="status"></p>
        <details id="network-external-firewall"><summary>受信許可を会社・PC管理者が設定している場合</summary><p>moyAI以外で設定したルールは自動確認できません。管理者が現在のHubの実行ファイルとポートへの受信を許可した場合に選んでください。この確認は画面を開き直すか、接続先・受信許可の確認結果が変わると解除されます。</p><label class="network-check"><input id="network-firewall-external-confirm" type="checkbox"><span>管理者が現在のHubへの受信を許可済みであることを確認した</span></label></details>
        <p class="footnote">受信許可を確認してから保存・コピーします。含まれるのはHubのURLと公開CAです。管理画面のURLや利用者のパスワードは含みません。</p>
        <details id="network-config-preview" hidden><summary>接続設定を選択してコピー</summary><textarea id="network-config-text" readonly rows="7" aria-label="Desktopに渡す接続設定"></textarea></details>
        <p><a href="#network-step-confirm">Desktopで接続ファイルを開いたら、参加を許可する →</a></p></section>
      <section id="network-step-confirm" class="network-card"><h3>4. 参加を許可して、Desktopで接続を診断</h3>
        <p class="subtle">Desktopが自動で申請します。PC名とIPを確認し、許可画面で既存プロジェクトへの操作・実行用途も選べます。名前は後から変更できます。</p>
        <a id="network-open-pending" href="#clients" class="primary-link">接続PC・承認待ちを確認する <span id="network-pending-summary"></span> →</a>
        <ol><li>上の一覧で申請元のPCを確かめ、「許可」を押します。</li><li>そのPCのDesktopで「PCの接続 → 接続の詳細・診断 → Hubへの接続を診断」を実行します。</li><li>仕事を実行するPCでは「このPCでHubのAIを使う場合の診断」も実行します。</li></ol><p>接続と必要なAIが使えることを確かめたら、手順3の同じ接続ファイルを残りのPCへ渡します。各PCの参加はそれぞれ許可してください。</p><a href="#network-step-file">チームに配る接続ファイルを保存する</a>
        <p class="footnote">共有仕事を使う人とプロジェクトは先に<a href="#team-onboarding">チームの初期設定</a>から設定できます。PC参加だけを許可し、用途を後から指定することもできます。</p></section></div>
      <div hidden><div class="network-rule-introduction"><h3>PC同士で直接仕事を依頼する場合</h3><p class="subtle">端末グループと接続ルールを設定します。Hubが受け付ける共有仕事の利用者・実行先は「プロジェクト・利用者」で管理します。</p></div>
      <div class="network-grid"><section class="network-card"><h3>端末グループ</h3><p class="footnote">PCをまとめて接続許可を設定できます。例:「開発」「GPU搭載」の両方へ同じ端末を追加できます。</p>
        <form id="network-group-form"><label>編集するグループ<select id="network-group-id"><option value="">新しいグループを追加</option></select></label><label>グループ名<input id="network-group-label" maxlength="160" required /></label><button class="secondary">グループを保存</button></form><ul id="network-group-list" class="network-group-list"></ul></section>
      <section class="network-card"><h3>どのPCからどのPCへ依頼できるか</h3><form id="network-rule-form"><div class="network-fields"><label>送信元<select id="network-rule-from" required></select></label><label>受入先<select id="network-rule-to" required></select></label></div>
        <div class="network-fields"><label>依頼の経路<select id="network-rule-usage"><option value="direct">直接の依頼</option><option value="delegated">再委任の宛先</option></select></label><label>扱い<select id="network-rule-effect"><option value="allow">許可する</option><option value="deny">拒否する（個別例外）</option></select></label></div>
        <label>公開対象<select id="network-rule-profile"><option value="">この宛先の公開対象すべて</option></select></label><label>最大の委任段数<select id="network-rule-depth"><option value="1">1段</option><option value="2">2段（例: 00 → 19 → 20）</option><option value="3">3段</option></select></label><button class="primary">接続ルールを追加</button></form>
        <p class="footnote">矢印は一方向です。逆向きは別に許可します。再委任には直前の端末からの直接許可と、最初の依頼元から宛先への再委任許可の両方が必要です。</p></section></div>
      <section class="network-card"><h3>接続ルールと個別例外</h3><div id="network-rule-list"></div><p class="footnote">端末の停止・失効やルール変更は新しい受付を止めます。既に実行しているタスクの停止完了は、Desktopの委任経路と停止状況で確認してください。</p></section></div>`;
    clientsHost.innerHTML = `<p class="subtle">参加申請を許可したり、登録したPCの名前を変更したりできます。「承認状態」は利用の許可、「稼働状態」はPCが現在接続しているかを表します。</p>
      <div class="network-client-toolbar"><label class="network-search">PCを検索<input id="network-search" type="search" placeholder="PC名・IP" /></label>
        <label>表示<select id="network-filter"><option value="all">すべてのPC</option><option value="pending">承認待ちのみ</option></select></label>
        <button id="network-clients-refresh" class="secondary">最新情報を取得</button></div>
      <p id="network-clients-summary" class="footnote"></p><p id="network-clients-notice" class="field-status" role="status" aria-live="polite"></p>
      <div class="table-wrap"><table class="network-table"><thead><tr><th>PC名・接続時のIPアドレス</th><th>承認状態</th><th>稼働状態</th><th hidden>所属グループ</th><th>時刻</th><th>操作</th></tr></thead><tbody id="network-device-rows"></tbody></table></div>
      <p id="network-devices-empty" class="subtle">登録されたPCはありません。接続ファイルをDesktopで開くと、参加申請が届きます。</p>
      <p class="footnote">「停止」は新しい利用を止める操作です。登録情報と接続ルールは保持し、「再許可」で戻せます。実行中のタスクは即座には止まりません。停止完了はDesktopの委任経路で確認してください。</p>
      <details hidden class="technical-details network-legacy-invite" data-details-key="network-legacy-invite"><summary>従来の参加コードで登録する（互換）</summary><div class="network-card">
        <form id="network-invite-form"><label>登録するPC名<input id="network-invite-label" placeholder="例: Win19" maxlength="160" required autocomplete="off" /></label>
        <fieldset hidden><legend>所属グループ（複数選択可）</legend><div id="network-invite-groups" class="network-memberships"></div></fieldset><button class="secondary">参加コードを発行</button></form>
        <div id="network-invite-receipt" hidden><label>参加コード<input id="network-invite-code" readonly autocomplete="off" /></label><p id="network-invite-expiry" class="footnote"></p><button id="network-copy-invite" class="secondary">コードをコピー</button></div>
        <p class="footnote">コード入力に対応したDesktopで使う互換手順です。参加コードは10分以内に1回だけ使えます。</p></div></details>
      <dialog id="network-device-dialog" aria-labelledby="network-device-title"><form id="network-device-form"><div class="section-title"><h2 id="network-device-title">PCの登録を編集</h2><button id="network-device-close" type="button" class="icon-button" aria-label="閉じる">×</button></div>
        <p id="network-device-id" class="network-mono"></p><label>PC名<input id="network-device-label" maxlength="160" aria-describedby="network-device-error network-device-name-help" required /></label><p id="network-device-error" class="field-status" role="status"></p><p id="network-device-name-help" class="footnote">同じHubで使われていない名前を指定してください。前後の空白・大文字小文字は区別しません。</p><fieldset hidden><legend>所属グループ（複数選択可）</legend><div id="network-device-groups" class="network-memberships"></div></fieldset>
        <details id="network-device-revoke-details"><summary>端末認証の恒久失効（元に戻せません）</summary><label class="network-check"><input id="network-device-revoked" type="checkbox" /><span>この端末の認証を恒久的に失効する</span></label><p class="footnote">一時的に利用を止める場合は、一覧の「停止」を使ってください。恒久失効は再許可できず、実行中のタスクの停止完了を意味しません。</p></details>
        <div class="network-actions"><button class="primary" id="network-device-save">変更を保存</button><button type="button" class="secondary" id="network-device-review">最新情報を取得</button></div></form></dialog>
      <details data-details-key="network-deleted-devices"><summary>削除済みの登録</summary><p class="footnote">削除前のPCの登録IDと過去の履歴を表示します。接続を再開するには、Desktopから新しく参加を申請してください。</p><div id="network-deleted-list"></div></details>
      <dialog id="network-delete-dialog" aria-labelledby="network-delete-title"><h2 id="network-delete-title">この端末の登録を削除しますか</h2><p id="network-delete-identity" class="network-mono"></p><p id="network-delete-warning"></p><p id="network-delete-error" class="field-status error" role="status"></p><div class="network-actions"><button id="network-delete-cancel" class="secondary" type="button">取消</button><button id="network-delete-confirm" class="danger" type="button">この登録を削除</button></div></dialog>`;
    const roots = [host, clientsHost];
    const node = <T extends HTMLElement = HTMLElement>(id: string) => (host.querySelector<T>(`#${id}`) ?? clientsHost.querySelector<T>(`#${id}`))!;
    const value = (id: string) => node<HTMLInputElement | HTMLSelectElement>(id).value;
    const firewallHost = node("network-firewall-native");
    let firewallState: FirewallStatus["state"] | null = null;
    const firewall = attachFirewall(firewallHost, state => {
        if (state === null || state !== firewallState) node<HTMLInputElement>("network-firewall-external-confirm").checked = false;
        firewallState = state; render();
    }, () => {
        window.location.hash = "network-step-file";
        node("network-step-file").scrollIntoView({block:"start"});
    });
    const message = (text: string, error = false) => { for (const id of ["network-notice", "network-clients-notice"]) { node(id).textContent = text; node(id).classList.toggle("error", error); } };
    const owner = new NetworkEdits();
    let busy = false;
    let polling: Promise<boolean> | null = null;
    let requestSerial = 0;
    let selectedClient: { kind: "request" | "device"; id: string } | null = null;
    let diagnostic: { result: NetworkDiagnostics; target: string; checkedAt: number } | null = null;
    let copiedConfigTarget: string | null = null;
    let executable: string | null = null;
    let loadingOptions = false;
    // Compare server projection markup with its previous projection, not live
    // DOM serialization: details.open and user selections belong to the UI.
    let renderedClientRows: string | null = null;
    const dialog = node<HTMLDialogElement>("network-device-dialog");
    const deleteDialog = node<HTMLDialogElement>("network-delete-dialog");
    let deletion: ReturnType<typeof networkDeviceDeletion> | null = null;
    const groupsIn = (id: string) => [...node(id).querySelectorAll<HTMLInputElement>("input:checked")].map(input => input.value);
    const memberships = (id: string, selected: string[] = groupsIn(id)) => {
        const container = node(id);
        if (container.contains(document.activeElement)) return;
        const html = owner.snapshot!.groups.map(group => `<label class="network-check"><input type="checkbox" value="${escape(group.id)}" ${selected.includes(group.id) ? "checked" : ""}/><span>${escape(group.label)}</span></label>`).join("") || '<p class="footnote">グループは後から追加できます。</p>';
        if (container.innerHTML !== html) container.innerHTML = html;
    };
    const options = (id: string) => {
        const select = node<HTMLSelectElement>(id);
        if (select === document.activeElement) return;
        const snapshot = owner.snapshot!;
        const html = '<option value="">選択してください</option>' + snapshot.groups.map(group => `<option value="group:${escape(group.id)}">グループ: ${escape(group.label)}</option>`).join("")
            + snapshot.devices.filter(device => !device.revoked).map(device => `<option value="device:${escape(device.device_id)}">端末: ${escape(device.label)}</option>`).join("");
        if (select.innerHTML !== html) { const old = select.value; select.innerHTML = html; select.value = old; }
    };
    const subjectLabel = (subject: NetworkSubject) => subject.kind === "group"
        ? `グループ: ${owner.snapshot?.groups.find(group => group.id === subject.id)?.label ?? subject.id}`
        : `端末: ${owner.snapshot?.devices.find(device => device.device_id === subject.id)?.label ?? subject.id}`;
    function lock(): void {
        for (const root of roots) {
            root.setAttribute("aria-busy", String(busy));
            for (const control of root.querySelectorAll<HTMLButtonElement | HTMLInputElement | HTMLSelectElement>("button,input,select")) {
                // The Firewall child owns its review/confirmation and in-flight
                // state. Network polling must not enable its guarded actions.
                if (!firewallHost.contains(control)) control.disabled = busy || !owner.snapshot;
            }
        }
        node<HTMLButtonElement>("network-refresh").disabled = busy;
        node<HTMLButtonElement>("network-clients-refresh").disabled = busy;
        node<HTMLButtonElement>("network-device-review").disabled = busy;
        node<HTMLInputElement>("network-ip").disabled = busy || !owner.snapshot || Boolean(owner.snapshot.server.running);
        node<HTMLInputElement>("network-port").disabled = busy || !owner.snapshot || Boolean(owner.snapshot.server.running);
        node<HTMLSelectElement>("network-lan").disabled = busy || loadingOptions || Boolean(owner.snapshot?.server.running);
        node<HTMLButtonElement>("network-lan-refresh").disabled = busy || loadingOptions;
        node<HTMLButtonElement>("network-renew-certificate").disabled = busy || !owner.snapshot?.server.running;
        node<HTMLButtonElement>("network-diagnose").disabled = busy || !owner.snapshot?.server.running;
        for (const id of ["network-copy-config", "network-save-config"]) node<HTMLButtonElement>(id).disabled = busy || !canDistribute();
        node<HTMLInputElement>("network-firewall-external-confirm").disabled = busy || !owner.snapshot?.server.running;
        node<HTMLButtonElement>("network-device-revoked").disabled = busy || !owner.snapshot || Boolean(owner.snapshot.devices.find(device => device.device_id === selectedClient?.id)?.revoked);
        node<HTMLButtonElement>("network-device-save").disabled = busy || !owner.snapshot || (selectedClient?.kind === "request"
            ? !owner.snapshot.join_requests.some(request => request.request_id === selectedClient?.id && request.status === "pending")
            : !owner.snapshot.devices.some(device => device.device_id === selectedClient?.id));
        node<HTMLButtonElement>("network-delete-confirm").disabled = busy || !deletion || !owner.snapshot?.devices.some(device => device.device_id === deletion?.args.deviceId);
    }
    function render(): void {
        const snapshot = owner.snapshot;
        if (!snapshot) return;
        const currentConfigTarget = JSON.stringify([snapshot.hub_id, snapshot.server.running, snapshot.server.bind, snapshot.ca_sha256]);
        if (copiedConfigTarget !== currentConfigTarget) {
            node("network-config-preview").hidden = true;
            node<HTMLTextAreaElement>("network-config-text").value = "";
        }
        node("network-server-status").textContent = snapshot.server.running ? "稼働中" : "停止中";
        firewall.refresh(networkDiagnosticTarget(snapshot), snapshot.server.running);
        const external = node<HTMLInputElement>("network-firewall-external-confirm").checked;
        const step = networkSetupStep(snapshot, firewallState, external);
        node("network-next-title").textContent = step.title;
        node("network-next-detail").textContent = step.detail;
        node<HTMLAnchorElement>("network-next-link").href = `#${step.anchor}`;
        node("network-next-link").textContent = step.action;
        node("network-external-firewall").hidden = firewallState === "matching" || networkIsLocalOnly(snapshot);
        node("network-distribution-status").textContent = !snapshot.server.running ? "Hubの開始後、受信許可を確認してから配布します。"
            : networkIsLocalOnly(snapshot) ? "この接続ファイルはHubと同じPCでのみ使えます。別PCへの配布にはLAN側の接続先が必要です。"
            : firewallState === "matching" ? "受信許可を確認しました。接続ファイルを配布し、別PCのDesktopで接続を確認してください。"
            : external ? "管理者による設定済みの確認に基づいて保存できます。別PCからの接続はまだ確認していません。"
            : "先に手順2で受信を許可してください。管理者が別途設定済みの場合は下の確認欄を使います。";
        node("network-firewall-prerequisite").textContent = snapshot.server.running ? "現在のHubとGatewayのポートを使って受信許可を設定します。設定後の接続確認はDesktopで行ってください。" : "手順1で接続の受付を開始してから、受信許可を設定してください。";
        node("network-server-status").className = `badge ${snapshot.server.running ? "green" : "gray"}`;
        node("network-start").hidden = snapshot.server.running;
        node("network-stop").hidden = !snapshot.server.running;
        const address = owner.serverAddress();
        if (address && !node("network-server-form").contains(document.activeElement)) {
            node<HTMLInputElement>("network-ip").value = address.ip;
            node<HTMLInputElement>("network-port").value = address.port;
        }
        node("network-hosting-error").hidden = !snapshot.hosting.error;
        node("network-hosting-error").textContent = snapshot.hosting.error
            ? `${snapshot.hosting.error} IPとポートを確認して接続の受付を開始すると、起動設定を保存し直せます。` : "";
        node("network-identity").textContent = `Hub: ${snapshot.hub_id} / ${snapshot.server.bind ?? "停止中"} / 更新 ${snapshot.revision}`;
        node("network-ca").textContent = `公開CA SHA-256: ${snapshot.ca_sha256}`;
        const gatewayPort = node<HTMLInputElement>("network-gateway-port");
        if (gatewayPort !== document.activeElement && !owner.revisions.has("network-settings-form")) gatewayPort.value = snapshot.gateway_port ? String(snapshot.gateway_port) : "";
        node("network-gateway-bind").textContent = `稼働中のGateway: ${snapshot.server.gateway_bind ?? "停止中"}`;
        node("network-firewall-info").textContent = `moyAI Hub 受信許可の依頼\n実行ファイル: ${executable ?? "LAN候補を再取得して確認してください"}\nHub: ${snapshot.server.bind ?? snapshot.hosting.bind ?? "未設定"}\nGateway: ${snapshot.server.gateway_bind ?? `停止中（次回指定ポート: ${snapshot.gateway_port || "自動"}）`}\n通信: TCP\n接続元PC/サブネット: 管理者が利用範囲を指定\nネットワークプロファイル: 社内LANに限定\n別PCからの到達: Desktop側で確認が必要`;
        const certificateStatus = networkCertificateStatus(snapshot.server.certificate, Date.now());
        node("network-certificate-status").textContent = certificateStatus.label;
        node("network-certificate-status").classList.toggle("error", certificateStatus.warning);
        const certificate = snapshot.server.certificate;
        node("network-certificate-times").innerHTML = certificate ? `<dt>有効期限</dt><dd>${when(Number(certificate.expires_at_ms))}</dd><dt>自動更新の開始目安</dt><dd>${when(Number(certificate.renew_after_ms))}</dd><dt>前回の更新</dt><dd>${when(Number(certificate.last_success_ms))}</dd>` : "";
        if (diagnostic && diagnostic.target !== networkDiagnosticTarget(snapshot)) diagnostic = null;
        const diagnosticRegion = node("network-diagnostic-result");
        diagnosticRegion.hidden = !diagnostic;
        if (diagnostic) {
            const labels = { pass: "確認済み", fail: "確認できません", skipped: "未実施" };
            const html = `<p class="footnote">確認日時: ${when(diagnostic.checkedAt)}</p><ol class="network-diagnostic-stages">${networkDiagnosticStages(diagnostic.result, diagnostic.checkedAt).map(stage => `<li><strong>${escape(stage.label)} <span class="badge ${stage.status === "pass" ? "green" : stage.status === "fail" ? "red" : "gray"}">${labels[stage.status]}</span></strong><p>${escape(stage.detail)}</p></li>`).join("")}</ol>`;
            if (diagnosticRegion.innerHTML !== html) diagnosticRegion.innerHTML = html;
        }
        memberships("network-invite-groups");
        const groupSelect = node<HTMLSelectElement>("network-group-id");
        if (groupSelect !== document.activeElement) {
            const html = '<option value="">新しいグループを追加</option>' + snapshot.groups.map(group => `<option value="${escape(group.id)}">${escape(group.label)}</option>`).join("");
            if (groupSelect.innerHTML !== html) { const old = groupSelect.value; groupSelect.innerHTML = html; groupSelect.value = old; }
        }
        options("network-rule-from"); options("network-rule-to");
        const profileSelect = node<HTMLSelectElement>("network-rule-profile");
        if (profileSelect !== document.activeElement) {
            const to = networkSubject(value("network-rule-to"));
            const devices = snapshot.devices.filter(device => to?.kind === "device" ? device.device_id === to.id : to?.kind === "group" && device.groups.includes(to.id));
            const html = '<option value="">この宛先の公開対象すべて</option>' + devices.flatMap(device => device.publications.map(profile => `<option value="${escape(profile.profile_id)}">${escape(device.label)}: ${escape(profile.name)}</option>`)).join("");
            if (profileSelect.innerHTML !== html) {
                const old = profileSelect.value;
                profileSelect.innerHTML = html;
                profileSelect.value = old;
                // A publication removed by the destination change must leave a
                // visible selection, not an empty-looking select with index -1.
                if (profileSelect.selectedIndex < 0) profileSelect.value = "";
            }
        }
        const rows = networkClients(snapshot, value("network-search"), value("network-filter") as NetworkClientFilter);
        const pendingCount = snapshot.join_requests.filter(request => request.status === "pending").length;
        node("network-clients-summary").textContent = `承認待ち ${pendingCount} 台 · 登録済み ${snapshot.devices.length} 台 · オンライン ${snapshot.devices.filter(device => device.online).length} 台`;
        node("network-pending-summary").textContent = pendingCount ? `（${pendingCount} 台）` : "";
        const deletedHtml = snapshot.deleted_devices?.map(device => `<p><strong>${escape(device.label)}</strong> · 削除 ${when(device.deleted_at_ms)}<br><code>${escape(device.device_id)}</code></p>`).join("") || '<p class="subtle">削除済みの登録はありません。</p>';
        if (node("network-deleted-list").innerHTML !== deletedHtml) node("network-deleted-list").innerHTML = deletedHtml;
        onSnapshot(snapshot);
        const body = node("network-device-rows");
        {
            const html = rows.map(client => {
            const action = networkClientAction(client, snapshot.revision);
            const actionButton = action ? `<button type="button" class="${action.label === "許可" ? "primary" : "secondary"}" data-network-action="${escape(JSON.stringify(action))}" aria-label="${escape(client.kind === "request" ? client.request.label : client.device.label)}の${action.label}">${action.label}</button>` : "";
            if (client.kind === "request") {
                const request = client.request;
                const pending = request.status === "pending";
                return `<tr data-id="request:${escape(request.request_id)}"><td><strong>${escape(request.label)}</strong>${request.name_conflict ? '<small class="error">同名の登録あり。「名前」で別名へ変更してください。</small>' : ""}<small>Windowsユーザー名: ${escape(request.windows_username || "未取得")}</small><small>申請元IP: ${escape(request.observed_ip)}</small><details data-device-identity="request:${escape(request.request_id)}"><summary>照合用ID</summary><small>申請ID</small><code>${escape(request.request_id)}</code><small>公開鍵 SHA-256</small><code>${escape(request.key_sha256)}</code></details></td>
                    <td><span class="badge ${pending ? "amber" : "gray"}">${pending ? "承認待ち" : "申請期限切れ"}</span><small>${pending ? "Windowsユーザー名とIPを確認して許可してください。" : "Desktopからもう一度申請してください。"}</small></td><td><span class="badge gray">未参加</span><small>まだ利用できません。</small></td><td hidden>未設定</td><td><small>申請: ${when(request.created_at_ms)}</small><small>期限: ${when(request.expires_at_ms)}</small></td><td><div class="network-row-actions">${actionButton}<button type="button" class="tiny-button" data-network-request="${escape(request.request_id)}" data-network-revision="${escape(snapshot.revision)}" aria-label="${escape(request.label)}の名前を確認">名前</button></div></td></tr>`;
            }
            const device = client.device;
            const status = networkDeviceStatus(device);
            const admission = networkAdmissionStatus(device);
            return `<tr data-id="device:${escape(device.device_id)}"><td><strong>${escape(device.label)}</strong>${device.name_conflict ? '<small class="error">同名の登録があります。IDで識別し、名前の変更か不要登録の削除で解消してください。</small>' : ""}<small>Windowsユーザー名: ${escape(device.windows_username || "未取得")}</small><small>接続時のIPアドレス: ${escape(device.observed_ip ?? "未確認")}</small><details data-device-identity="device:${escape(device.device_id)}"><summary>照合用ID</summary><small>端末ID</small><code>${escape(device.device_id)}</code><small>証明書 SHA-256</small><code>${escape(device.certificate_sha256)}</code></details></td><td><span class="badge ${admission.color}">${admission.label}</span><small>${escape(admission.detail)}</small></td><td><span class="badge ${status.color}">${status.label}</span><small>${escape(status.detail)}</small><details data-network-legacy-mcp="${escape(device.device_id)}"><summary>以前のMCP連携</summary><small>${device.publications.some(profile => profile.enabled) ? "受付申告あり" : "受付の申告なし"}</small></details>${device.unsettled_jobs ? `<small>終了未確認のタスク: ${device.unsettled_jobs} 件</small>` : ""}</td><td hidden>${device.groups.map(id => `<span class="network-tag">${escape(snapshot.groups.find(group => group.id === id)?.label ?? id)}</span>`).join("") || "所属なし"}</td><td><small>最後の応答</small>${when(device.last_seen_ms)}</td><td><div class="network-row-actions">${actionButton}<button type="button" class="tiny-button" data-network-edit="${escape(device.device_id)}" data-network-revision="${escape(snapshot.revision)}" aria-label="${escape(device.label)}を管理">名前・詳細</button><button type="button" class="tiny-button" data-network-history="${escape(device.device_id)}" aria-label="${escape(device.label)}のMCP履歴を見る">履歴を見る</button><button type="button" class="tiny-button danger" data-network-delete-device="${escape(device.device_id)}" data-network-revision="${escape(snapshot.revision)}" aria-label="${escape(device.label)}の登録を削除">削除</button></div></td></tr>`;
            }).join("");
            if (renderedClientRows !== html) {
                reconcileNetworkRows(body, html, node("network-clients-refresh"));
                renderedClientRows = html;
            }
        }
        node("network-devices-empty").hidden = rows.length > 0;
        node("network-devices-empty").textContent = value("network-filter") === "pending" ? "条件に一致する承認待ちの申請はありません。"
            : snapshot.devices.length || snapshot.join_requests.length ? "検索に一致する端末はありません。" : "登録されたPCはありません。接続ファイルをDesktopで開くと、参加申請が届きます。";
        node("network-group-list").innerHTML = snapshot.groups.map(group => `<li><strong>${escape(group.label)}</strong><span>${snapshot.devices.filter(device => device.groups.includes(group.id)).length} 台</span></li>`).join("");
        const rules = node("network-rule-list");
        if (!rules.contains(document.activeElement)) rules.innerHTML = snapshot.rules.map(rule => `<article class="network-rule"><div><strong>${escape(subjectLabel(rule.from))} → ${escape(subjectLabel(rule.to))}</strong><p>${rule.usage === "direct" ? "直接の依頼" : "再委任の宛先"} · <span class="${rule.effect === "deny" ? "error" : ""}">${rule.effect === "allow" ? "許可" : "拒否"}</span> · 最大${rule.max_depth}段 · ${rule.profile_id ? escape(snapshot.devices.flatMap(device => device.publications).find(profile => profile.profile_id === rule.profile_id)?.name ?? rule.profile_id) : "公開対象すべて"}</p></div><button class="secondary" data-network-delete-rule="${escape(rule.id)}" data-network-revision="${escape(snapshot.revision)}" aria-label="${escape(subjectLabel(rule.from))}から${escape(subjectLabel(rule.to))}のルールを削除">削除</button></article>`).join("") || '<p class="subtle">接続ルールはありません。端末を参加させただけでは、ほかの端末へ依頼できません。</p>';
        lock();
    }
    async function readSnapshot(): Promise<boolean> {
        const serial = requestSerial;
        try {
            const snapshot = await invoke<NetworkSnapshot>("hub_network_snapshot");
            if (serial !== requestSerial || !owner.accept(snapshot)) return false;
            if (!interactionActive() && !dialog.open && !deleteDialog.open) render();
            return true;
        } catch { message("PCの接続状態を取得できません。最新情報を取得してください。", true); return false; }
    }
    async function acquireSnapshot(): Promise<boolean> {
        if (busy) return false;
        if (!polling) polling = readSnapshot().finally(() => { polling = null; });
        return polling;
    }
    async function refresh(): Promise<void> {
        await acquireSnapshot();
    }
    async function perform(editor: string, name: string, args: Record<string, unknown>, receipt?: (result: unknown) => void): Promise<void> {
        if (busy || !owner.snapshot) return;
        ++requestSerial;
        busy = true; lock();
        if (name === "hub_network_stop") message("Hub の受付とモデル中継を停止しています…");
        try {
            const result = await invoke(name, args);
            owner.saved(editor);
            if (result && typeof result === "object" && "devices" in result) owner.accept(result as NetworkSnapshot);
            message(name === "hub_network_stop" ? "接続の受付を停止しました。" : name === "hub_network_start"
                ? "接続の受付を開始し、起動設定を保存しました。次回は起動ショートカットで再開できます。" : "設定を更新しました。");
            receipt?.(result);
            await readSnapshot();
            if (name === "hub_network_start" && owner.snapshot && !networkIsLocalOnly(owner.snapshot)) {
                window.location.hash = "network-step-firewall";
                node("network-step-firewall").scrollIntoView({block:"start"});
            }
        } catch (error) {
            const text = typeof error === "string" ? error : "操作を完了できませんでした。入力内容を確認してください。";
            message(text, true); if (dialog.open) node("network-device-error").textContent = text;
            if (deleteDialog.open) node("network-delete-error").textContent = text;
            if (name === "hub_network_stop") await readSnapshot();
        } finally { busy = false; lock(); render(); }
    }
    for (const id of ["network-server-form", "network-invite-form", "network-group-form", "network-rule-form", "network-device-form", "network-settings-form"]) {
        node(id).addEventListener("input", () => owner.touch(id));
        node(id).addEventListener("change", () => owner.touch(id));
    }
    for (const id of ["network-refresh", "network-clients-refresh", "network-device-review"]) node(id).addEventListener("click", () => {
        void acquireSnapshot().then(accepted => {
            if (accepted) {
                owner.reviewLatest(); render(); message("最新の設定を取得しました。入力中の内容は保持しています。内容を確認してから保存してください。");
                if (dialog.open) node("network-device-error").textContent = node<HTMLButtonElement>("network-device-save").disabled
                    ? "この申請または端末は現在変更できません。一覧の最新状態を確認してください。" : "最新の状態を取得しました。入力内容を確認して保存してください。";
            }
        });
    });
    node("network-search").addEventListener("input", render);
    async function hostingOptions() {
        if (loadingOptions) return;
        loadingOptions = true; lock();
        try {
            const options = await invoke<{ candidates: {label:string;ip:string}[]; error:string|null; executable:string|null }>("hub_hosting_options");
            executable = options.executable;
            node<HTMLSelectElement>("network-lan").innerHTML = '<option value="">接続するLANを選択（手入力も可）</option>' + options.candidates.map(row => `<option value="${escape(row.ip)}">${escape(row.label)} · ${escape(row.ip)}</option>`).join("");
            node("network-lan-status").textContent = options.error ?? (options.candidates.length ? "接続するPCと通信できるLANのIPアドレスを選び、「接続の受付を開始」を押してください。" : "LAN候補がありません。OSのネットワーク設定で確認して手入力してください。");
        } catch { node("network-lan-status").textContent = "候補を取得できません。IPを手入力して続けられます。"; }
        finally { loadingOptions = false; lock(); render(); }
    }
    node("network-lan-refresh").addEventListener("click", () => void hostingOptions());
    node("network-lan").addEventListener("change", () => {
        if (!value("network-lan")) return;
        node<HTMLInputElement>("network-ip").value = value("network-lan"); owner.touch("network-server-form");
    });
    node("network-firewall-copy").addEventListener("click", () => { void copyText(node("network-firewall-info").textContent ?? "").then(() => message("設定依頼をコピーしました。接続元とプロファイルは管理者が指定してください。")).catch(() => message("コピーできませんでした。表示された設定依頼を選択してコピーしてください。",true)); });
    node("network-filter").addEventListener("change", render);
    node("network-open-pending").addEventListener("click", () => {
        node<HTMLSelectElement>("network-filter").value = owner.snapshot?.join_requests.some(row => row.status === "pending") ? "pending" : "all";
        node<HTMLInputElement>("network-search").value = "";
        render();
    });
    node("network-server-form").addEventListener("submit", event => {
        event.preventDefault(); const ip = value("network-ip").trim();
        const bind = `${ip.includes(":") ? `[${ip.replace(/^\[|\]$/g, "")}]` : ip}:${value("network-port").trim()}`;
        void perform("network-server-form", "hub_network_start", { bind, advertiseIp: ip });
    });
    node("network-stop").addEventListener("click", () => void perform("network-server-form", "hub_network_stop", {}));
    node("network-settings-form").addEventListener("submit", event => {
        event.preventDefault();
        const gatewayPort = networkGatewayPort(value("network-gateway-port"));
        if (gatewayPort === null) { message("ポートは1〜65535で指定するか、自動設定なら空欄にしてください。", true); return; }
        void perform("network-settings-form", "hub_network_settings", { expectedRevision: owner.target("network-settings-form"), gatewayPort }, () => message("ポート設定を保存しました。次回のネットワーク開始から適用します。"));
    });
    node("network-renew-certificate").addEventListener("click", () => void perform("certificate", "hub_network_renew_certificate", {}, () => message("証明書を更新しました。新しい接続から使用します。")));
    node("network-diagnose").addEventListener("click", () => {
        if (busy || !owner.snapshot?.server.running) return;
        const target = networkDiagnosticTarget(owner.snapshot);
        busy = true; requestSerial++; lock(); message("HubとGatewayの状態を確認しています。");
        void invoke<NetworkDiagnostics>("hub_network_diagnostics").then(result => {
            if (!owner.snapshot || target !== networkDiagnosticTarget(owner.snapshot) || !networkDiagnosticMatches(result, owner.snapshot)) {
                message("接続状態が変わりました。最新情報を取得し、もう一度診断してください。"); return;
            }
            diagnostic = { result, target, checkedAt: Date.now() };
            message("診断結果を表示しました。別端末からの到達はDesktop側でも確認してください。");
        }).catch(error => message(typeof error === "string" ? error : "接続状態を取得できませんでした。ネットワークの稼働状態を確認してください。", true))
            .finally(() => { busy = false; lock(); render(); });
    });
    const exportArgs = (): { hubUrl: string } | null => {
        if (!canDistribute()) { message("受信許可を確認してから接続ファイルを配布してください。", true); return null; }
        const hubUrl = owner.snapshot?.server.running ? owner.snapshot.server.bind : null;
        if (!hubUrl) { message("共通設定を取得するには、端末接続の受付を開始してください。", true); return null; }
        return { hubUrl };
    };
    function canDistribute(): boolean {
        return networkCanDistribute(owner.snapshot, firewallState, node<HTMLInputElement>("network-firewall-external-confirm").checked);
    }
    node("network-firewall-external-confirm").addEventListener("change", render);
    node("network-copy-config").addEventListener("click", () => {
        const args = exportArgs(); if (!args) return;
        void perform("export", "hub_network_export", args, result => {
            const text = (result as { toml: string }).toml;
            copiedConfigTarget = JSON.stringify([owner.snapshot!.hub_id, owner.snapshot!.server.running, owner.snapshot!.server.bind, owner.snapshot!.ca_sha256]);
            node("network-config-preview").hidden = false;
            node<HTMLTextAreaElement>("network-config-text").value = text;
            void copyText(text).then(() => message("接続設定をコピーしました。Desktopの「moyAI Hub → PCの接続」へ貼り付けてください。"), () => {
                node<HTMLDetailsElement>("network-config-preview").open = true;
                node<HTMLTextAreaElement>("network-config-text").focus(); node<HTMLTextAreaElement>("network-config-text").select();
                message("下の接続設定を選択しました。手動でコピーするか、接続ファイルを保存してください。", true);
            });
        });
    });
    node("network-save-config").addEventListener("click", () => {
        const args = exportArgs(); if (args) void perform("export", "hub_network_save_config", args, result => message(savedMessage(typeof result === "string" ? result : null)));
    });
    node("network-invite-form").addEventListener("submit", event => {
        event.preventDefault(); void perform("network-invite-form", "hub_network_invite", {
            expectedRevision: owner.target("network-invite-form"), label: value("network-invite-label").trim(), groups: groupsIn("network-invite-groups"),
        }, result => {
            const receipt = result as { code: string; expires_at_ms: number };
            node<HTMLInputElement>("network-invite-code").value = receipt.code;
            node("network-invite-expiry").textContent = `有効期限: ${when(receipt.expires_at_ms)}（1回限り）`;
            node("network-invite-receipt").hidden = false;
        });
    });
    node("network-copy-invite").addEventListener("click", () => { void copyText(value("network-invite-code")).then(() => message("参加コードをコピーしました。許可した端末へ渡してください。"), () => message("コピーできませんでした。コード欄から選択してコピーしてください。", true)); });
    node("network-group-form").addEventListener("submit", event => {
        event.preventDefault(); void perform("network-group-form", "hub_network_group", { expectedRevision: owner.target("network-group-form"), id: value("network-group-id") || null, label: value("network-group-label").trim() }, () => { node<HTMLInputElement>("network-group-label").value = ""; node<HTMLSelectElement>("network-group-id").value = ""; });
    });
    node("network-group-id").addEventListener("change", () => {
        node<HTMLInputElement>("network-group-label").value = owner.snapshot?.groups.find(group => group.id === value("network-group-id"))?.label ?? "";
    });
    node("network-rule-form").addEventListener("submit", event => {
        event.preventDefault(); const from = networkSubject(value("network-rule-from")), to = networkSubject(value("network-rule-to"));
        if (!from || !to) return;
        void perform("network-rule-form", "hub_network_rule", { expectedRevision: owner.target("network-rule-form"), rule: {
            id: null, from, to, usage: value("network-rule-usage"), effect: value("network-rule-effect"), max_depth: Number(value("network-rule-depth")), profile_id: value("network-rule-profile") || null,
        } });
    });
    node("network-rule-to").addEventListener("change", render);
    for (const root of roots) root.addEventListener("click", event => {
        const target = (event.target as Element).closest<HTMLElement>("[data-network-edit], [data-network-request], [data-network-action], [data-network-delete-rule], [data-network-delete-device]");
        if (!target || busy || !owner.snapshot) return;
        if (target.dataset.networkAction) {
            const action = JSON.parse(target.dataset.networkAction) as NetworkClientAction;
            void perform("client-action", action.command, action.args, () => message(action.command === "hub_network_approve"
                ? "端末の参加を許可しました。Desktopが登録情報を取得すると一覧へ反映されます。"
                : action.args.stopped ? "新しい利用を停止しました。実行中のタスクの停止完了とは別です。" : "端末を再許可しました。Desktop側の受付・接続先設定も確認してください。"));
            return;
        }
        if (target.dataset.networkDeleteRule) { void perform("rule-delete", "hub_network_delete_rule", { expectedRevision: target.dataset.networkRevision, id: target.dataset.networkDeleteRule }); return; }
        if (target.dataset.networkDeleteDevice) {
            const device = owner.snapshot.devices.find(device => device.device_id === target.dataset.networkDeleteDevice);
            if (!device) return;
            deletion = networkDeviceDeletion(device, target.dataset.networkRevision!);
            node("network-delete-identity").textContent = deletion.identity;
            node("network-delete-warning").textContent = deletion.warning;
            node("network-delete-error").textContent = "";
            lock(); deleteDialog.showModal(); node("network-delete-cancel").focus(); return;
        }
        if (target.dataset.networkRequest) {
            const request = owner.snapshot.join_requests.find(request => request.request_id === target.dataset.networkRequest);
            if (!request) return;
            selectedClient = { kind: "request", id: request.request_id };
            owner.saved("network-device-form"); owner.revisions.set("network-device-form", target.dataset.networkRevision!);
            node("network-device-title").textContent = "名前を確認して許可";
            node("network-device-save").textContent = "この内容で許可";
            node("network-device-id").textContent = `申請元IP: ${request.observed_ip}\n申請ID: ${request.request_id}\n公開鍵 SHA-256: ${request.key_sha256}`;
            node<HTMLInputElement>("network-device-label").value = request.label;
            node<HTMLInputElement>("network-device-revoked").checked = false;
            node("network-device-revoke-details").hidden = true;
            memberships("network-device-groups", []); node("network-device-error").textContent = request.status === "expired" ? "申請の期限が切れています。Desktopからもう一度申請してください。" : request.name_conflict ? "同名の登録または申請があります。別のPC名へ変更してから許可してください。" : "名前の変更は任意です。";
            lock(); dialog.showModal(); return;
        }
        const device = owner.snapshot.devices.find(device => device.device_id === target.dataset.networkEdit);
        if (!device) return;
        selectedClient = { kind: "device", id: device.device_id };
        owner.saved("network-device-form"); owner.revisions.set("network-device-form", target.dataset.networkRevision!);
        node("network-device-title").textContent = "PCの登録を編集";
        node("network-device-save").textContent = "変更を保存";
        node("network-device-id").textContent = `端末ID: ${device.device_id}`;
        node<HTMLInputElement>("network-device-label").value = device.label;
        node<HTMLInputElement>("network-device-revoked").checked = device.revoked;
        node("network-device-revoke-details").hidden = false;
        node<HTMLDetailsElement>("network-device-revoke-details").open = false;
        memberships("network-device-groups", device.groups); node("network-device-error").textContent = device.name_conflict ? "同名の登録があります。端末IDを確認し、別名への変更か不要登録の削除で解消してください。" : ""; lock(); dialog.showModal();
    });
    node("network-device-close").addEventListener("click", () => dialog.close());
    node("network-delete-cancel").addEventListener("click", () => { if (!busy) deleteDialog.close(); });
    deleteDialog.addEventListener("cancel", event => { if (busy) event.preventDefault(); });
    deleteDialog.addEventListener("close", () => { deletion = null; render(); });
    node("network-delete-confirm").addEventListener("click", () => {
        if (!deletion) return;
        void perform("device-delete", "hub_network_delete_device", deletion.args, () => {
            deleteDialog.close(); message("端末の登録を削除しました。履歴の帰属は保持し、同じ名前を新しい参加申請で利用できます。");
        });
    });
    dialog.addEventListener("cancel", event => { if (busy) event.preventDefault(); });
    node("network-device-form").addEventListener("submit", event => {
        event.preventDefault();
        if (selectedClient?.kind === "request") {
            void perform("network-device-form", "hub_network_approve", { expectedRevision: owner.target("network-device-form"), requestId: selectedClient.id,
                label: value("network-device-label").trim(), groups: groupsIn("network-device-groups"),
            }, () => { dialog.close(); message("端末の参加を許可しました。"); });
            return;
        }
        if (!selectedClient) return;
        void perform("network-device-form", "hub_network_device", { expectedRevision: owner.target("network-device-form"), deviceId: selectedClient.id,
            label: value("network-device-label").trim(), groups: groupsIn("network-device-groups"), revoked: node<HTMLInputElement>("network-device-revoked").checked,
        }, () => dialog.close());
    });
    lock();
    void hostingOptions();
    return { refresh };
}
