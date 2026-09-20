import { invoke } from "./management.ts";

interface Status { supported: boolean; enabled: boolean; conflict: boolean; command: string }
export function attachAutostart(host: HTMLElement) {
    const detail = document.createElement("details");
    detail.innerHTML = `<summary>サインイン時の自動起動</summary><p>このWindowsユーザーがサインインすると、Hubが自動で起動します。現在の実行ファイルとデータの保存先を使い、管理画面は開きません。サインアウト・シャットダウン・スリープ中は利用できません。AIのサーバーと実行PCも、それぞれ起動してください。</p><p>Hubの実行ファイルを移すときは、移動前に自動起動を無効にし、移動後に有効にしてください。</p><label>自動起動の実行内容<textarea id="hub-autostart-command" readonly rows="3"></textarea></label><p id="hub-autostart-status" role="status">自動起動の設定を読み込んでいます。</p><button id="hub-autostart-toggle" type="button" class="secondary" disabled>自動起動を有効にする</button> <button id="hub-autostart-refresh" type="button" class="secondary">登録を確認</button>`;
    host.append(detail);
    const command = detail.querySelector<HTMLTextAreaElement>("#hub-autostart-command")!;
    const status = detail.querySelector<HTMLElement>("#hub-autostart-status")!;
    const toggle = detail.querySelector<HTMLButtonElement>("#hub-autostart-toggle")!;
    const refreshButton = detail.querySelector<HTMLButtonElement>("#hub-autostart-refresh")!;
    let current: Status | null = null, busy = false;
    function accept(value: Status) {
        current = value; command.value = value.command;
        toggle.textContent = value.enabled ? "自動起動を無効にする" : "この内容で自動起動を有効にする";
        toggle.disabled = !value.supported || value.conflict;
        status.textContent = value.conflict ? "同じ保存先に別の開始登録があります。以前の設置先のHubで解除するか、PC管理者に確認してください。"
            : !value.supported ? "WindowsのHub PCで利用できます。" : value.enabled ? "この内容で登録済みです。" : "未登録です。上の開始内容を確認して登録してください。";
    }
    async function refresh() {
        if (busy) return;
        busy = true; toggle.disabled = true; refreshButton.disabled = true;
        try {
            const web = await invoke<{ local_url: string | null }>("hub_web_status");
            if (!web.local_url || new URL(web.local_url).origin !== window.location.origin) {
                current = null;
                status.textContent = "この設定はHub PC上のローカル管理画面で確認してください。";
                return;
            }
            accept(await invoke<Status>("hub_autostart_status"));
        }
        catch { current = null; status.textContent = "この設定はHub PC上のローカル管理画面で確認してください。登録を読み取れない場合はPC管理者に確認してください。"; }
        finally { busy = false; refreshButton.disabled = false; }
    }
    toggle.addEventListener("click", async () => {
        if (busy || !current) return;
        busy = true; toggle.disabled = true; refreshButton.disabled = true;
        try { accept(await invoke<Status>("hub_autostart_set", { enabled: !current.enabled, expectedCommand: current.command })); }
        catch (error) { current = null; status.textContent = typeof error === "string" ? error : "保存結果を確認できません。「登録を確認」で現在の状態を確認してください。"; }
        finally { busy = false; refreshButton.disabled = false; }
    });
    refreshButton.addEventListener("click", () => void refresh());
    void refresh();
}
