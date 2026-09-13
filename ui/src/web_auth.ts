import { authenticate, managementSession, onAuthenticationChange } from "./management.ts";
import type { WebSession } from "./management.ts";

export function authenticationError(error: unknown): string {
    const messages: Record<string, string> = {
        unauthorized: "ユーザー名またはパスワードを確認してください。",
        invalid_credentials: "ユーザー名またはパスワードを確認してください。",
        administrator_required: "管理者アカウントでログインしてください。",
        forbidden: "この操作は許可されていません。ログイン状態を再確認してください。",
        invalid_request: "入力内容を確認してください。",
        already_configured: "初回設定は完了しています。ログイン状態を再確認してください。",
        rate_limited: "試行回数が上限に達しました。しばらく待ってから再試行してください。",
        bootstrap_not_allowed: "初回設定はHubと同じPCで、直接接続した管理画面から行ってください。",
    };
    return typeof error === "string" && messages[error] ? messages[error]
        : typeof error === "string" && /[ぁ-んァ-ン一-龯]/.test(error) ? error
        : "認証操作を完了できませんでした。入力と接続を確認して再試行してください。";
}

/** The management module is loaded only after an authenticated administrator is known. */
export function attachWebAuthentication(
    root: HTMLElement,
    management: HTMLElement,
    startManagement: () => Promise<void>,
): { refresh: () => Promise<void> } {
    let session: WebSession | null = null;
    let configured = true;
    let bootstrapAllowed = false;
    let loginAllowed = false;
    let checking = true;
    let busy = false;
    let error = "";
    let generation = 0;
    let startedFor: string | null = null;
    let username = "";
    let displayName = "";
    const node = <T extends HTMLElement>(id: string) => root.querySelector<T>(`#${id}`)!;
    function concealManagement() {
        management.hidden = true;
        management.inert = true;
        for (const dialog of management.querySelectorAll<HTMLDialogElement>("dialog[open]")) dialog.close();
    }
    function render() {
        const user = session?.authentication.user;
        const administrator = loginAllowed && user?.administrator === true;
        root.className = administrator ? "web-auth-account" : "web-auth-gate";
        if (administrator) {
            root.innerHTML = `<div class="web-auth-account-name"><span>管理者</span><strong id="web-auth-user"></strong></div>
                <p id="web-auth-error" role="alert" hidden></p><button id="web-auth-logout" type="button" class="secondary">ログアウト</button>`;
            node("web-auth-user").textContent = user.display_name;
            node<HTMLButtonElement>("web-auth-logout").disabled = busy;
            node("web-auth-logout").addEventListener("click", () => { void submit("logout"); });
        } else {
            const bootstrap = !configured && bootstrapAllowed;
            const formAllowed = !checking && loginAllowed && !user && (configured || bootstrap);
            root.innerHTML = `<section class="web-auth-card" aria-labelledby="web-auth-title">
                <p class="eyebrow">moyAI Hub</p><h1 id="web-auth-title">${checking ? "接続を確認しています" : bootstrap ? "初回の管理者設定" : "管理者ログイン"}</h1>
                <p class="subtle" id="web-auth-help">${checking ? "ログイン状態を確認しています。" : !loginAllowed
                    ? "この接続ではログインできません。HTTPSの管理URL、またはHubと同じPCの直接接続URLを開いてください。"
                    : !configured && !bootstrapAllowed
                    ? "初回設定が必要です。Hubと同じPCで、直接接続した管理画面を開いて管理者を設定してください。"
                    : user ? "この管理画面は管理者のみ利用できます。管理者アカウントでログインしてください。"
                    : bootstrap ? "このHubを管理する最初のアカウントを作成します。"
                    : "モデル・端末・接続の設定を管理するには、管理者アカウントでログインしてください。"}</p>
                ${formAllowed ? `<form id="web-auth-form">
                    <label>ユーザー名<input id="web-auth-username" name="username" autocomplete="username" maxlength="64" autocapitalize="none" spellcheck="false" required autofocus></label>
                    ${bootstrap ? '<label>表示名<input id="web-auth-display-name" name="display_name" autocomplete="name" maxlength="256" required></label>' : ""}
                    <label>パスワード<input id="web-auth-password" name="password" type="password" autocomplete="${bootstrap ? "new-password" : "current-password"}" maxlength="1024" required></label>
                    ${bootstrap ? '<label>パスワードの確認<input id="web-auth-password-confirm" type="password" autocomplete="new-password" maxlength="1024" required></label>' : ""}
                    ${bootstrap ? '<p class="subtle">ユーザー名は半角英数字と . _ - @ が使えます。パスワードは12文字以上で設定してください。</p>' : ""}
                    <button id="web-auth-submit" type="submit" class="primary">${busy ? "確認しています…" : bootstrap ? "管理者を設定してログイン" : "ログイン"}</button>
                </form>` : ""}
                <p id="web-auth-error" class="web-auth-error" role="alert" hidden></p>
                <button id="web-auth-refresh" type="button" class="secondary" ${busy || checking ? "disabled" : ""}>ログイン状態を再確認</button>
                ${user ? '<button id="web-auth-logout" type="button" class="secondary">ログアウト</button>' : ""}
                <p class="web-auth-footnote">ブラウザーを閉じてもHubは動作を続けます。</p>
            </section>`;
            if (formAllowed) {
                node<HTMLInputElement>("web-auth-username").value = username;
                if (bootstrap) node<HTMLInputElement>("web-auth-display-name").value = displayName;
                for (const field of root.querySelectorAll<HTMLInputElement | HTMLButtonElement>("form input, form button")) field.disabled = busy;
                node("web-auth-form").addEventListener("submit", event => {
                    event.preventDefault();
                    if (busy) return;
                    username = node<HTMLInputElement>("web-auth-username").value.trim();
                    displayName = bootstrap ? node<HTMLInputElement>("web-auth-display-name").value.trim() : "";
                    const password = node<HTMLInputElement>("web-auth-password").value;
                    const confirmation = bootstrap ? node<HTMLInputElement>("web-auth-password-confirm").value : password;
                    node<HTMLInputElement>("web-auth-password").value = "";
                    if (bootstrap) node<HTMLInputElement>("web-auth-password-confirm").value = "";
                    if (password !== confirmation) { error = "確認用のパスワードが一致しません。もう一度入力してください。"; render(); return; }
                    if (bootstrap && !/^[A-Za-z0-9._@-]{1,64}$/.test(username)) {
                        error = "ユーザー名は64文字以内の半角英数字と . _ - @ で入力してください。"; render(); return;
                    }
                    if (bootstrap && (!displayName || new TextEncoder().encode(displayName).length > 256 || /\p{Cc}/u.test(displayName))) {
                        error = "表示名を入力してください。長い表示名は短くし、制御文字を除いてください。"; render(); return;
                    }
                    if ((bootstrap && [...password].length < 12) || new TextEncoder().encode(password).length > 1024) {
                        error = "パスワードは12文字以上、UTF-8で1024バイト以内にしてください。"; render(); return;
                    }
                    void submit(bootstrap ? "bootstrap" : "login", bootstrap
                        ? { username, display_name: displayName, password } : { username, password });
                });
            }
            node("web-auth-refresh").addEventListener("click", () => { void refresh(); });
            if (user) node("web-auth-logout").addEventListener("click", () => { void submit("logout"); });
        }
        node("web-auth-error").textContent = error;
        node("web-auth-error").hidden = !error;
    }
    async function accept(value: WebSession | null) {
        session = value;
        checking = false;
        if (value) {
            configured = value.authentication.configured; bootstrapAllowed = value.authentication.bootstrap_allowed;
            loginAllowed = value.authentication.login_allowed;
        }
        const user = value?.authentication.user;
        if (!value || !loginAllowed || !user?.administrator) {
            ++generation;
            startedFor = null;
            concealManagement();
            render();
            return;
        }
        render();
        const key = JSON.stringify([value.epoch, value.authentication.session_id, user.user_id]);
        if (startedFor === key) return;
        const expected = ++generation;
        startedFor = key;
        concealManagement();
        try {
            await startManagement();
            if (expected !== generation || !session?.authentication.user?.administrator) return;
            management.hidden = false;
            management.inert = false;
        } catch {
            if (expected !== generation) return;
            startedFor = null;
            error = "管理画面を読み込めませんでした。ページを再読み込みしてください。";
            render();
        }
    }
    async function submit(action: "login" | "bootstrap" | "logout", args: Record<string, string> = {}) {
        if (busy) return;
        busy = true; error = "";
        if (action === "logout") concealManagement();
        render();
        try { await authenticate(action, args); }
        catch (failure) { error = authenticationError(failure); }
        finally { busy = false; render(); }
    }
    async function refresh() {
        if (busy) return;
        checking = true; error = "";
        render();
        try { await managementSession(true); }
        catch (failure) { checking = false; error = authenticationError(failure); render(); }
    }
    onAuthenticationChange(value => { void accept(value); });
    concealManagement();
    render();
    return { refresh };
}
