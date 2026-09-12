let csrf: Promise<string> | null = null;
async function sessionToken(): Promise<string> {
    if (!csrf) csrf = (async () => {
        const controller = new AbortController();
        const timer = setTimeout(() => controller.abort(), 35_000);
        try {
            const response = await fetch("/admin/session", { credentials: "same-origin", cache: "no-store", signal: controller.signal });
            const value = await response.json();
            if (!response.ok || typeof value.csrf !== "string") throw "Hubに接続できません。サーバーの起動状態を確認し、再接続してください。";
            return value.csrf as string;
        } finally { clearTimeout(timer); }
    })().catch(error => { csrf = null; throw error; });
    return csrf;
}
async function browserInvoke<T>(command: string, args: Record<string, unknown>): Promise<T> {
    const token = await sessionToken();
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 35_000);
    try {
        const response = await fetch("/admin/command", {
            method: "POST", credentials: "same-origin", cache: "no-store", signal: controller.signal,
            headers: { "Content-Type": "application/json", "X-Moyai-Csrf": token }, body: JSON.stringify({ command, args }),
        });
        const value = await response.json();
        if (!response.ok) {
            if (response.status === 401 || response.status === 403) csrf = null;
            throw typeof value.error === "string" ? value.error : "Web管理操作を完了できませんでした。";
        }
        return value.result as T;
    } catch (error) {
        throw typeof error === "string" ? error : "操作結果を確認できません。最新状態を取得して確認してください。操作は自動で再実行されません。";
    } finally { clearTimeout(timer); }
}
function download(text: string, name: string, mime: string): string {
    const url = URL.createObjectURL(new Blob([text], { type: mime }));
    const link = document.createElement("a");
    link.href = url; link.download = name; document.body.append(link); link.click(); link.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1_000);
    return name;
}
export function savedMessage(saved: string | null): string {
    return saved ? `ダウンロードを開始しました: ${saved}` : "保存をキャンセルしました。";
}
export async function reconnect(): Promise<void> {
    csrf = null;
    await sessionToken();
}
/** LAN HTTP may not provide clipboard access; callers offer download or selection. */
export async function copyText(text: string): Promise<void> {
    if (!navigator.clipboard?.writeText) throw new Error("このブラウザーではクリップボードを利用できません。");
    return navigator.clipboard.writeText(text);
}
/** Browser downloads stay local; application commands use the Rust server owner. */
export async function invoke<T = unknown>(command: string, args: Record<string, unknown> = {}): Promise<T> {
    if (command === "hub_network_save_config") {
        const value = await browserInvoke<{ toml: string }>("hub_network_export", args);
        return download(value.toml, "hub-participation.toml", "application/toml;charset=utf-8") as T;
    }
    if (command === "hub_history_export") {
        const value = await browserInvoke<{ device_label: string; device_id: string; completed_at_ms: number | null; requested_at_ms: number; response: { markdown?: string } | null }>("hub_history_result", args);
        if (typeof value.response?.markdown !== "string") throw "履歴の詳細を取得してから保存してください。";
        const markdown = `# Hubから取得したMCP履歴\n\n- 端末: ${value.device_label.replace(/[\r\n]/g, " ")}\n- 端末ID: \`${value.device_id}\`\n- 取得日時（Unix ms）: ${value.completed_at_ms ?? value.requested_at_ms}\n- 取得済みのスナップショットです。現在の実行状態を保証しません。\n\n${value.response.markdown}`;
        return download(markdown, "moyai-mcp-history.md", "text/markdown;charset=utf-8") as T;
    }
    return browserInvoke<T>(command, args);
}
