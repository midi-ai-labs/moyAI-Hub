import { copyText, invoke, onAuthenticationChange } from "./management.ts";

interface Scope { remote_addresses: string[]; profiles: string[] }
interface Rule { name: string; program: string; local_address: string; local_ports: number[]; remote_addresses: string[]; profile_mask: number }
interface Preview { spec: Rule; binary_sha256: string; previous: Rule | null; conflict: boolean; matching: boolean; fingerprint: string }
interface Receipt { status: "verified" | "unchanged" | "cancelled" | "unconfirmed"; preview: Preview | null }
export interface FirewallStatus { state: "missing" | "matching" | "mismatch" | "conflict"; program: string; local_address: string; local_ports: number[]; active_profile?: string | null; current: Rule | null; other_programs: string[]; suggestion: Scope | null; suggestion_source: "existing" | "lan" }

export function firewallStatusMessage(value: FirewallStatus): string {
    if (value.state === "matching") return "現在のHub・Gatewayに対する受信許可を確認しました。別PCからの接続成功は、接続するDesktopで確認してください。";
    if (value.state === "mismatch") return value.active_profile === "Public" ? "このLANはWindowsでPublicに分類されており、登録済みの社内LAN向け受信許可が適用されません。PC管理者へネットワークの扱いを確認してください。" : `受信許可のIP・ポート・プロファイルが、現在のHub・Gatewayと一致しません。${value.active_profile ? `現在のWindows分類は${value.active_profile === "DomainAuthenticated" ? "Domain" : value.active_profile}です。` : ""}下で変更内容を確認して更新してください。`;
    if (value.state === "conflict") return "このHubのルールが手動変更されています。上書きせずPC管理者へ確認してください。";
    return value.other_programs.length ? "現在の実行ファイルに対するmoyAIの受信許可は未登録です。別の設置先を指すmoyAI Hubのルールがあります。固定の導入先から起動するか、現在の設置先で受信許可を確認してください。" : "この設置先でmoyAIが管理する受信許可は未登録です。会社で設定済みの場合は管理者へ確認し、必要な場合は下で設定してください。";
}

export function firewallScope(addresses: string, domain: boolean, privateNetwork: boolean): Scope {
    return { remote_addresses: addresses.split(/[\s,]+/).filter(Boolean), profiles: [...(domain ? ["Domain"] : []), ...(privateNetwork ? ["Private"] : [])] };
}
export function firewallSummary(rule: Rule): string {
    return `実行ファイル: ${rule.program}\n受信先: ${rule.local_address}\nTCPポート: ${rule.local_ports.join(", ")}\n接続元: ${rule.remote_addresses.join(", ")}\nプロファイル: ${[...(rule.profile_mask & 1 ? ["Domain"] : []), ...(rule.profile_mask & 2 ? ["Private"] : [])].join(", ")}\nルール名: ${rule.name}`;
}
export function attachFirewall(host: HTMLElement, onStatus: (state: FirewallStatus["state"] | null) => void = () => {}) {
    host.innerHTML = `<details id="hub-firewall"><summary>このHub PCで受信許可を設定する</summary><p>HubとGatewayを開始した後、社内の接続元IPv4範囲と信頼するプロファイルを指定します。Public、すべての接続元、Web管理画面、外向き通信の許可は追加しません。</p><label>許可する接続元PCまたはサブネット（最大16件）<textarea id="hub-firewall-addresses" rows="2" placeholder="192.168.10.0/24 または 192.168.10.21" spellcheck="false"></textarea></label><div class="network-memberships"><label class="network-check"><input id="hub-firewall-domain" type="checkbox"><span>Domain（社内ドメイン）</span></label><label class="network-check"><input id="hub-firewall-private" type="checkbox"><span>Private（信頼する社内LAN）</span></label></div><button id="hub-firewall-review" type="button" class="secondary">変更内容と現在のルールを確認</button><p id="hub-firewall-status" role="status">設定はHub PC上のローカル管理画面で行います。</p><div id="hub-firewall-preview" hidden><h4>今回の受信許可</h4><pre id="hub-firewall-target"></pre><h4>現在のこのHubのルール</h4><pre id="hub-firewall-previous"></pre><label class="network-check"><input id="hub-firewall-confirm" type="checkbox"><span>上の実行ファイル・接続元・ポート・プロファイルと変更前の内容を確認した</span></label><div class="network-actions"><button id="hub-firewall-apply" type="button" class="primary" disabled>この内容でWindowsへ設定を依頼</button><button id="hub-firewall-remove" type="button" class="danger" disabled>表示中の既存ルールを解除</button><button id="hub-firewall-copy-request" type="button" class="secondary">PC管理者への依頼をコピー</button></div></div><p class="footnote">Windowsの管理者確認（UAC）が表示されます。60秒以内に確認できなければ再確認してください。確認中は接続の開始・停止を待ちます。会社の管理ポリシーや他の拒否ルールは変更しません。登録後も参加Desktopから接続を診断してください。設置先を移す前は、このHubを開始した状態で既存ルールを解除します。</p></details>`;
    host.innerHTML = `<p id="hub-firewall-health" role="status">Hubを開始すると、現在の受信許可を確認します。</p><p>通常はSetupで導入した同じ場所からHubを起動してください。更新で設置先を変えると受信許可が引き継がれません。</p><button id="hub-firewall-health-refresh" class="secondary" type="button">受信許可を再確認</button><p id="hub-firewall-suggestion-text"></p><button id="hub-firewall-suggestion" class="secondary" type="button" hidden>このLANの設定を入力する</button>` + host.innerHTML;
    const node = <T extends HTMLElement = HTMLElement>(id: string) => host.querySelector<T>(`#${id}`)!;
    const addresses = node<HTMLTextAreaElement>("hub-firewall-addresses");
    const domain = node<HTMLInputElement>("hub-firewall-domain"), privateNetwork = node<HTMLInputElement>("hub-firewall-private");
    const confirm = node<HTMLInputElement>("hub-firewall-confirm"), review = node<HTMLButtonElement>("hub-firewall-review");
    const apply = node<HTMLButtonElement>("hub-firewall-apply"), remove = node<HTMLButtonElement>("hub-firewall-remove");
    const status = node("hub-firewall-status");
    let current: Preview | null = null, reviewedScope: Scope | null = null, busy = false, serial = 0, disposed = false;
    let lastTarget = "", running = false, healthSerial = 0, healthBusy = false, observed: FirewallStatus | null = null;
    const scope = () => firewallScope(addresses.value, domain.checked, privateNetwork.checked);
    function lock() {
        for (const input of [addresses, domain, privateNetwork, confirm, review]) input.disabled = busy || disposed;
        apply.disabled = busy || disposed || !current || current.conflict || current.matching || !confirm.checked;
        remove.disabled = busy || disposed || !current?.previous || current.conflict || !confirm.checked;
        node<HTMLButtonElement>("hub-firewall-health-refresh").disabled = busy || disposed || healthBusy || !running;
        node<HTMLButtonElement>("hub-firewall-suggestion").disabled = busy || disposed || healthBusy || !observed?.suggestion;
    }
    function accept(value: Preview | null) {
        current = value; confirm.checked = false; node("hub-firewall-preview").hidden = !value;
        if (value) {
            node("hub-firewall-target").textContent = firewallSummary(value.spec);
            node("hub-firewall-previous").textContent = value.previous ? firewallSummary(value.previous) : "未登録";
        }
        lock();
    }
    function changed() { ++serial; reviewedScope = null; accept(null); status.textContent = "入力が変わりました。変更内容を再確認してください。"; }
    async function inspect() {
        if (disposed || busy || !running) return;
        const expected = ++healthSerial; healthBusy = true; lock();
        node("hub-firewall-health").textContent = "現在の受信許可を確認しています…";
        try {
            const web = await invoke<{local_url: string|null}>("hub_web_status");
            if (disposed || expected !== healthSerial) return;
            if (!web.local_url || new URL(web.local_url).origin !== window.location.origin) {
                node("hub-firewall-health").textContent = "受信許可はHub PC上のローカル管理画面で確認してください。"; return;
            }
            const value = await invoke<FirewallStatus>("hub_firewall_status");
            if (disposed || expected !== healthSerial) return;
            observed = value; onStatus(value.state);
            // The parent may render a newer snapshot synchronously in this
            // callback. Recheck before projecting details for the old target.
            if (disposed || expected !== healthSerial) return;
            node("hub-firewall-health").textContent = firewallStatusMessage(value);
            node("hub-firewall-suggestion").hidden = !value.suggestion;
            node("hub-firewall-suggestion").textContent = value.suggestion_source === "existing" ? "登録済みの接続元とプロファイルを入力する" : "このLANの設定を入力する";
            node("hub-firewall-suggestion-text").textContent = value.suggestion ? `${value.suggestion_source === "existing" ? "登録済み" : "現在のLANから取得"}: ${value.suggestion.remote_addresses.join(", ")} / ${value.suggestion.profiles.join(", ")}。入力後に範囲を確認してください。` : "LANから設定候補を取得できません。接続元と信頼するプロファイルを下で指定してください。";
        } catch (error) { if (!disposed && expected === healthSerial) { observed = null; onStatus(null); if (disposed || expected !== healthSerial) return; node("hub-firewall-suggestion").hidden = true; node("hub-firewall-suggestion-text").textContent = ""; node("hub-firewall-health").textContent = typeof error === "string" ? error : "受信許可を確認できません。下の手順で確認してください。"; } }
        finally { if (expected === healthSerial) { healthBusy = false; lock(); } }
    }
    node("hub-firewall-health-refresh").addEventListener("click", () => void inspect());
    node("hub-firewall-suggestion").addEventListener("click", () => {
        if (busy || disposed || !observed?.suggestion) return;
        addresses.value = observed.suggestion.remote_addresses.join(", ");
        domain.checked = observed.suggestion.profiles.includes("Domain"); privateNetwork.checked = observed.suggestion.profiles.includes("Private");
        changed(); node<HTMLDetailsElement>("hub-firewall").open = true;
        status.textContent = "設定候補を入力しました。接続元の範囲を確認し、「変更内容と現在のルールを確認」へ進んでください。";
    });
    addresses.addEventListener("input", changed); domain.addEventListener("change", changed); privateNetwork.addEventListener("change", changed);
    confirm.addEventListener("change", lock);
    review.addEventListener("click", async () => {
        if (busy || disposed) return;
        const expected = ++serial; busy = true; accept(null); status.textContent = "Hubの接続先とファイアウォールのルールを確認しています…";
        try {
            const web = await invoke<{local_url: string|null}>("hub_web_status");
            if (!web.local_url || new URL(web.local_url).origin !== window.location.origin) {
                status.textContent = "この設定はHub PC上のローカル管理画面で行ってください。下の管理者向け依頼は別PCからもコピーできます。";
                return;
            }
            const selected = scope();
            const value = await invoke<Preview>("hub_firewall_preview", { scope: selected });
            if (disposed || expected !== serial) return;
            reviewedScope = selected; accept(value);
            status.textContent = value.conflict ? "既存ルールがこのHubの管理内容と一致しません。上書き・解除せずPC管理者へ確認してください。" : value.matching ? "表示した受信許可は設定済みです。接続するPCのDesktopでも接続を診断してください。会社の設定によって通信が制限される場合があります。" : "変更前と変更後を確認し、Windowsへ設定を依頼してください。";
        } catch (error) { if (!disposed && expected === serial) status.textContent = typeof error === "string" ? error : "ルールを確認できません。PC管理者へ設定を依頼してください。"; }
        finally { busy = false; lock(); }
    });
    async function change(removeRule: boolean) {
        if (busy || disposed || !current || !reviewedScope || !confirm.checked || current.conflict) return;
        const expected = ++serial, fingerprint = current.fingerprint, selected = reviewedScope;
        busy = true; lock(); status.textContent = "Windowsの管理者確認を待っています。結果が表示されるまで他の設定操作を待ってください。";
        try {
            const receipt = await invoke<Receipt>(removeRule ? "hub_firewall_remove" : "hub_firewall_apply", { scope:selected, expectedFingerprint:fingerprint });
            if (disposed || expected !== serial) return;
            accept(receipt.preview);
            status.textContent = receipt.status === "verified" || receipt.status === "unchanged"
                ? removeRule ? "表示したルールの解除を確認しました。ほかのルールは変更していません。" : "表示した受信許可の設定を確認しました。参加Desktopから接続を診断してください。"
                : receipt.status === "cancelled" ? "Windowsの管理者確認が取り消されました。表示中の現在のルールを確認してください。" : "操作の完了を確認できません。部分的な変更の可能性があるため、現在のルールを再確認してからPC管理者へ相談してください。";
        } catch (error) { if (!disposed && expected === serial) { accept(null); status.textContent = `${typeof error === "string" ? error : "結果を確認できません。"} 自動で再実行しません。現在のルールを再確認してください。`; } }
        finally { busy = false; lock(); void inspect(); }
    }
    apply.addEventListener("click", () => void change(false)); remove.addEventListener("click", () => void change(true));
    node("hub-firewall-copy-request").addEventListener("click", () => {
        if (!current || busy) return;
        void copyText(`moyAI Hub 受信TCP許可の依頼\n${firewallSummary(current.spec)}\n設定後は参加Desktopで到達を確認してください。`).then(() => { status.textContent = "設定依頼をコピーしました。"; }).catch(() => { status.textContent = "コピーできません。表示内容を選択してPC管理者へ渡してください。"; });
    });
    onAuthenticationChange(session => { if (!session?.authentication.user) { disposed = true; ++serial; ++healthSerial; current = null; reviewedScope = null; host.replaceChildren(); } });
    return { refresh(target: string, active: boolean) {
        if (disposed || target === lastTarget) return;
        lastTarget = target; running = active; ++healthSerial; healthBusy = false; observed = null; onStatus(null);
        ++serial; reviewedScope = null; accept(null); node("hub-firewall-suggestion").hidden = true; node("hub-firewall-suggestion-text").textContent = "";
        if (active) void inspect(); else node("hub-firewall-health").textContent = "Hubを開始すると、現在の受信許可を確認します。";
    } };
}
