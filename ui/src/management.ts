export interface AdminPrincipal {
    user_id: string;
    display_name: string;
    administrator: boolean;
}
export interface WebSession {
    csrf: string;
    epoch: string;
    authentication: {
        configured: boolean;
        user: AdminPrincipal | null;
        session_id: string | null;
        bootstrap_allowed: boolean;
        login_allowed: boolean;
        local_management?: boolean;
    };
}
let sessionRequest: Promise<WebSession> | null = null;
let currentSession: WebSession | null = null;
let authenticationGeneration = 0;
let authenticationRequired = false;
const authenticationListeners = new Set<(session: WebSession | null) => void>();
const staleAuthentication = "管理画面の利用許可が変わりました。Hub PCから管理画面を開き直してください。";

export function onAuthenticationChange(listener: (session: WebSession | null) => void): () => void {
    authenticationListeners.add(listener);
    return () => authenticationListeners.delete(listener);
}
function authenticationIdentity(session: WebSession): string {
    return JSON.stringify([session.epoch, session.authentication.session_id, session.authentication.user?.user_id,
        session.authentication.user?.administrator, session.authentication.login_allowed, session.authentication.local_management]);
}
function readSession(value: unknown): WebSession {
    const session = value as WebSession | null;
    const auth = session?.authentication;
    const user = auth?.user;
    if (!session || typeof session.csrf !== "string" || !session.csrf || typeof session.epoch !== "string"
        || !auth || typeof auth.configured !== "boolean" || typeof auth.bootstrap_allowed !== "boolean"
        || typeof auth.login_allowed !== "boolean"
        || !(auth.local_management === undefined || typeof auth.local_management === "boolean")
        || !(user === null ? auth.session_id === null : typeof auth.session_id === "string" && auth.session_id.length > 0)
        || !(user === null || (user && typeof user.user_id === "string" && typeof user.display_name === "string"
            && typeof user.administrator === "boolean"))) {
        throw "Hubの認証情報を確認できません。Hubを更新して再接続してください。";
    }
    return session;
}
function acceptSession(value: WebSession, expectedGeneration: number): WebSession {
    if (expectedGeneration !== authenticationGeneration) throw staleAuthentication;
    if (currentSession && authenticationIdentity(currentSession) !== authenticationIdentity(value)) {
        ++authenticationGeneration;
    }
    currentSession = value;
    authenticationRequired = false;
    for (const listener of authenticationListeners) listener(value);
    return value;
}
function invalidateAuthentication(): void {
    ++authenticationGeneration;
    authenticationRequired = true;
    sessionRequest = null;
    currentSession = null;
    for (const listener of authenticationListeners) listener(null);
}
async function fetchSession(): Promise<WebSession> {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 35_000);
    try {
        const response = await fetch("/admin/session", { credentials: "same-origin", cache: "no-store", signal: controller.signal });
        if (!response.ok) throw "Hubに接続できません。サーバーの起動状態を確認し、再接続してください。";
        return readSession(await response.json());
    } finally { clearTimeout(timer); }
}
export async function managementSession(refresh = false): Promise<WebSession> {
    if (refresh) {
        ++authenticationGeneration;
        sessionRequest = null;
    }
    if (!sessionRequest) {
        const expectedGeneration = authenticationGeneration;
        const pending = fetchSession().then(value => acceptSession(value, expectedGeneration));
        sessionRequest = pending;
        void pending.catch(() => { if (sessionRequest === pending) sessionRequest = null; });
    }
    return sessionRequest;
}
export async function authenticate(
    action: "access" | "logout", args: Record<string, string> = {},
): Promise<WebSession> {
    const priorSession = currentSession;
    // Retain only the CSRF value needed to revoke the cookie. Old management
    // requests must lose their owner before logout is sent or another login starts.
    invalidateAuthentication();
    const expectedGeneration = authenticationGeneration;
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 35_000);
    try {
        // A CSRF refresh during sign-in must not briefly restore a previous user
        // whose cookie may still exist after an uncertain logout.
        const session = priorSession ?? await fetchSession();
        if (expectedGeneration !== authenticationGeneration) throw staleAuthentication;
        if (action !== "logout" && !session.authentication.login_allowed) {
            throw "Hub PCの「moyAI Team Management」から管理画面を開いてください。";
        }
        const response = await fetch(`/admin/${action}`, {
            method: "POST", credentials: "same-origin", cache: "no-store", signal: controller.signal,
            headers: { "Content-Type": "application/json", "X-Moyai-Csrf": session.csrf }, body: JSON.stringify(args),
        });
        const value = await response.json();
        if (expectedGeneration !== authenticationGeneration) throw staleAuthentication;
        if (!response.ok) throw typeof value.error === "string" ? value.error : "管理画面の利用許可を確認できませんでした。";
        const accepted = acceptSession(readSession(value), expectedGeneration);
        sessionRequest = Promise.resolve(accepted);
        return accepted;
    } catch (error) {
        throw typeof error === "string" ? error : "管理画面の利用許可を確認できません。Hub PCから管理画面を開き直してください。";
    } finally { clearTimeout(timer); }
}
export class ManagementFailure extends Error {
    readonly status: number;
    readonly code: string;
    constructor(message: string, status: number, code: string) { super(message); this.status = status; this.code = code; }
}
async function browserInvoke<T>(command: string, args: Record<string, unknown>, detailed = false): Promise<T> {
    if (authenticationRequired) throw staleAuthentication;
    const expectedGeneration = authenticationGeneration;
    const session = await managementSession();
    if (expectedGeneration !== authenticationGeneration) throw staleAuthentication;
    if (!session.authentication.login_allowed || !session.authentication.user?.administrator) throw "Hub PCから管理画面を開いてください。";
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), command === "hub_firewall_apply" || command === "hub_firewall_remove" ? 130_000 : 35_000);
    try {
        const response = await fetch("/admin/command", {
            method: "POST", credentials: "same-origin", cache: "no-store", signal: controller.signal,
            headers: { "Content-Type": "application/json", "X-Moyai-Csrf": session.csrf,
                "X-Moyai-Session": session.authentication.session_id! }, body: JSON.stringify({ command, args }),
        });
        const value = await response.json();
        if (expectedGeneration !== authenticationGeneration) throw staleAuthentication;
        if (!response.ok) {
            if (response.status === 401 || response.status === 403 && value.code !== "requires_hub_pc") invalidateAuthentication();
            if (detailed) throw new ManagementFailure(typeof value.error === "string" ? value.error : "管理操作を完了できませんでした。", response.status, typeof value.code === "string" ? value.code : "unavailable");
            throw typeof value.error === "string" ? value.error : "Web管理操作を完了できませんでした。";
        }
        if (detailed && value.result?.reauthenticate === true) {
            invalidateAuthentication();
            throw new ManagementFailure("設定を保存しました。Hub PCから管理画面を開き直してください。", 401, "unauthorized");
        }
        return value.result as T;
    } catch (error) {
        if (error instanceof ManagementFailure) throw error;
        throw typeof error === "string" ? error : "操作結果を確認できません。最新状態を取得して確認してください。操作は自動で再実行されません。";
    } finally { clearTimeout(timer); }
}
export function invokeSharedAdmin<T>(command: "hub_shared_snapshot" | "hub_shared_command" | "hub_shared_retention_preview" | "hub_shared_maintenance" | "hub_join_project", args: Record<string, unknown> = {}): Promise<T> {
    return browserInvoke<T>(command, args, true);
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
    await managementSession(true);
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
        return download(value.toml, "hub-config.moyai-join", "application/toml;charset=utf-8") as T;
    }
    if (command === "hub_history_export") {
        const value = await browserInvoke<{ device_label: string; device_id: string; completed_at_ms: number | null; requested_at_ms: number; response: { markdown?: string } | null }>("hub_history_result", args);
        if (typeof value.response?.markdown !== "string") throw "履歴の詳細を取得してから保存してください。";
        const markdown = `# Hubから取得したMCP履歴\n\n- 端末: ${value.device_label.replace(/[\r\n]/g, " ")}\n- 端末ID: \`${value.device_id}\`\n- 取得日時（Unix ms）: ${value.completed_at_ms ?? value.requested_at_ms}\n- 取得時点の記録です。現在の実行状態はDesktopで確認してください。\n\n${value.response.markdown}`;
        return download(markdown, "moyai-mcp-history.md", "text/markdown;charset=utf-8") as T;
    }
    return browserInvoke<T>(command, args);
}
