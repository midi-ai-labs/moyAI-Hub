import { copyText, invoke, managementSession } from "./management.ts";

export interface WebStatus {
    enabled: boolean;
    local_url: string | null;
    remote_url: string | null;
    settings_error?: string | null;
    remote_error?: string | null;
    remote_certificate_expires_at_ms?: number | null;
}
export function attachWebManagement(): { refresh: () => Promise<void> } {
    const host = document.createElement("section");
    host.className = "panel web-management"; host.id = "web-management";
    document.getElementById("device-network")!.before(host);
    host.innerHTML = `<div class="section-title"><h2>管理画面のURL</h2><span id="web-enabled-state" class="badge">接続を確認中</span></div>
        <p>Hub PC、または管理を許可されたPCのDesktopから開いたブラウザーで、プロジェクト・モデル・PCを管理できます。URLを開くだけでは管理操作はできません。</p>
        <p id="web-local-management-required" hidden>このPCからはプロジェクトやPCの許可を管理できます。モデル接続先の探索・変更、Hubの待受、Windowsの設定は、Hub PCで「moyAI Team Management」から開いて操作してください。</p>
        <p>Desktopを接続する場合は<a href="#device-network">「PCの接続」で接続ファイルを保存</a>してください。管理画面のURLをDesktopに入力する必要はありません。</p>
        <div id="web-address" class="web-address" hidden><label>Hubと同じPCで開くURL<input id="web-url" readonly></label><button id="web-copy-url" type="button" class="secondary" disabled>URLをコピー</button></div>
        <div id="web-remote-address" class="web-address" hidden><label>別のPCで管理者が開くURL<input id="web-remote-url" readonly></label><button id="web-copy-remote-url" type="button" class="secondary" disabled>管理用URLをコピー</button></div>
        <p id="web-status" role="status">Web管理の接続情報を取得しています。</p><p id="web-settings-error" role="alert" hidden></p>
        <p>Web管理のURLとポートはHubの起動設定で指定します。ブラウザーを閉じてもHubは動作を続けます。</p>`;
    const node = <T extends HTMLElement>(id: string) => host.querySelector<T>(`#${id}`)!;
    let refreshing = false;
    let feedback: string | null = null;
    let statusVersion = 0;
    let addresses = "";
    async function refresh() {
        if (refreshing) return;
        refreshing = true;
        try {
            const status = await invoke<WebStatus>("hub_web_status");
            const session = await managementSession();
            node("web-local-management-required").hidden = session.authentication.local_management !== false;
            const currentAddresses = `${status.enabled}\n${status.local_url ?? ""}\n${status.remote_url ?? ""}`;
            if (currentAddresses !== addresses) { addresses = currentAddresses; ++statusVersion; feedback = null; }
            node("web-enabled-state").textContent = status.enabled ? "稼働中" : "停止中";
            for (const [region, field, button, url] of [
                ["web-address", "web-url", "web-copy-url", status.local_url],
                ["web-remote-address", "web-remote-url", "web-copy-remote-url", status.remote_url],
            ] as const) {
                const value = status.enabled ? url ?? "" : "";
                node(region).hidden = !value;
                const input = node<HTMLInputElement>(field);
                if (input.value !== value) input.value = value;
                node<HTMLButtonElement>(button).disabled = !value;
            }
            const expires = status.remote_certificate_expires_at_ms;
            const certificateNotice = expires != null && expires <= Date.now() + 7 * 24 * 60 * 60 * 1000
                ? "別PCからのWeb管理に使う証明書の有効期限は " + new Date(expires).toLocaleString("ja-JP") + " です。証明書を交換してHubを再起動してください。"
                : "";
            const error = status.settings_error ?? (status.remote_error
                ? "別PCからのWeb管理は停止中です。" + status.remote_error + " 設定を確認してHubを再起動してください。"
                : certificateNotice);
            node("web-settings-error").hidden = !error;
            node("web-settings-error").textContent = error ?? "";
            node("web-status").textContent = feedback ?? (!status.enabled ? "Web管理は停止中です。Hubの起動状態を確認してください。"
                : status.remote_url?.startsWith("http:") ? "他のPCからの管理にはHTTPSの管理URLが必要です。現在のHTTPのURLでは利用できません。"
                : status.remote_url ? "他の端末からもWeb管理を利用できます。" : "Web管理はHubと同じPCから利用できます。");
        } catch {
            node("web-enabled-state").textContent = "応答を確認できません";
            node("web-status").textContent = "Hub本体への接続を確認し、画面下部の「Hub本体へ再接続」を押してください。";
        } finally { refreshing = false; }
    }
    for (const [button, field] of [["web-copy-url", "web-url"], ["web-copy-remote-url", "web-remote-url"]]) {
        node(button).addEventListener("click", async () => {
            const value = node<HTMLInputElement>(field).value;
            if (!value) return;
            const expectedVersion = statusVersion;
            let message: string;
            try { await copyText(value); message = "URLをコピーしました。"; }
            catch { message = "コピーできませんでした。URL欄から選択してコピーしてください。"; }
            if (expectedVersion === statusVersion) { feedback = message; node("web-status").textContent = message; }
        });
    }
    return { refresh };
}
