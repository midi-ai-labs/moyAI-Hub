import { authenticate, managementSession, onAuthenticationChange } from "./management.ts";
import type { WebSession } from "./management.ts";

/** Remove the one-use ticket before any request, management import, or UI update. */
export function consumeAccessTicket(
    location: Pick<Location, "hash" | "pathname" | "search">,
    history: Pick<History, "replaceState">,
): string | null {
    const fragment = new URLSearchParams(location.hash.replace(/^#/, ""));
    if (!fragment.has("access")) return null;
    const ticket = fragment.get("access") ?? "";
    const notice = fragment.get("notice");
    const retained = fragment.get("section") === "network-step-firewall" ? "#network-step-firewall"
        : notice === "hub-started" || notice === "hub-already-running" ? `#${notice}` : "";
    history.replaceState(null, "", `${location.pathname}${location.search}${retained}`);
    return ticket;
}

export function authenticationError(error: unknown): string {
    const messages: Record<string, string> = {
        unauthorized: "管理画面の利用許可が切れました。Hub PCから開き直してください。",
        invalid_credentials: "この管理画面の起動情報は使えません。Hub PCから開き直してください。",
        invalid_ticket: "この管理画面の起動情報は使用済み、または期限切れです。Hub PCから開き直してください。",
        administrator_required: "Hub PCの「moyAI Team Management」から管理画面を開いてください。",
        forbidden: "この管理画面からは操作できません。Hub PCから開き直してください。",
        invalid_request: "管理画面の起動情報を確認できません。Hub PCから開き直してください。",
        rate_limited: "しばらく待ってから、Hub PCで管理画面を開き直してください。",
    };
    return typeof error === "string" && messages[error] ? messages[error]
        : typeof error === "string" && /[ぁ-んァ-ン一-龯]/.test(error) ? error
        : "Hubに接続できません。Hubの起動状態を確認し、管理画面を開き直してください。";
}

/** The management module is loaded only after the Hub accepts its launch ticket. */
export function attachWebAuthentication(
    root: HTMLElement,
    management: HTMLElement,
    startManagement: () => Promise<void>,
): { refresh: () => Promise<void> } {
    let ticket = consumeAccessTicket(window.location, window.history);
    let session: WebSession | null = null;
    let checking = true;
    let busy = false;
    let error = "";
    let generation = 0;
    let refreshGeneration = 0;
    let startedFor: string | null = null;
    const node = <T extends HTMLElement>(id: string) => root.querySelector<T>(`#${id}`)!;
    function concealManagement() {
        management.hidden = true;
        management.inert = true;
        for (const dialog of management.querySelectorAll<HTMLDialogElement>("dialog[open]")) dialog.close();
    }
    function render() {
        const administrator = session?.authentication.login_allowed && session.authentication.user?.administrator;
        root.className = administrator ? "web-auth-account" : "web-auth-gate";
        if (administrator) {
            root.innerHTML = `<div class="web-auth-account-name"><span>Hub管理</span><strong id="web-auth-user"></strong></div>
                <p id="web-auth-error" role="alert" hidden></p><button id="web-auth-logout" type="button" class="secondary">この画面の利用を終了</button>`;
            node("web-auth-user").textContent = session!.authentication.user!.display_name;
            node<HTMLButtonElement>("web-auth-logout").disabled = busy;
            node("web-auth-logout").addEventListener("click", () => { void closeAccess(); });
        } else {
            root.innerHTML = `<section class="web-auth-card" aria-labelledby="web-auth-title">
                <p class="eyebrow">moyAI Hub</p><h1 id="web-auth-title">${checking ? "管理画面を開いています" : "Hub PCから管理画面を開いてください"}</h1>
                <p class="subtle" id="web-auth-help">${checking ? "この画面の利用許可を確認しています。" : "管理を許可されたPCのDesktopで「Hubの管理画面を開く」を押してください。Hubを設置したPCでは、スタートメニューの「moyAI Team Management」からも開けます。IDやパスワードの入力は不要です。"}</p>
                <p id="web-auth-error" class="web-auth-error" role="alert" hidden></p>
                <button id="web-auth-refresh" type="button" class="secondary" ${busy || checking ? "disabled" : ""}>利用許可を再確認</button>
                <p class="web-auth-footnote">ブラウザーを閉じてもHubは動作を続けます。</p>
            </section>`;
            node("web-auth-refresh").addEventListener("click", () => { void refresh(); });
        }
        node("web-auth-error").textContent = error;
        node("web-auth-error").hidden = !error;
    }
    async function accept(value: WebSession | null) {
        session = value;
        checking = false;
        const user = value?.authentication.user;
        if (!value?.authentication.login_allowed || !user?.administrator) {
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
    async function closeAccess() {
        if (busy) return;
        const expected = ++refreshGeneration;
        busy = true; error = "";
        concealManagement(); render();
        try { await authenticate("logout"); }
        catch (failure) { if (expected === refreshGeneration) error = authenticationError(failure); }
        finally { if (expected === refreshGeneration) { busy = false; render(); } }
    }
    async function refresh(replaceAccess = false) {
        if (busy && !replaceAccess) return;
        const expected = ++refreshGeneration;
        busy = true; checking = true; error = ""; render();
        const pendingTicket = ticket;
        ticket = null;
        try {
            if (pendingTicket !== null) await authenticate("access", { ticket: pendingTicket });
            else await managementSession(true);
        } catch (failure) {
            if (expected === refreshGeneration) { checking = false; error = authenticationError(failure); }
        } finally {
            if (expected === refreshGeneration) { busy = false; render(); }
        }
    }
    window.addEventListener("hashchange", () => {
        const replacement = consumeAccessTicket(window.location, window.history);
        if (replacement === null) return;
        ticket = replacement;
        // A launcher may navigate an existing tab by fragment only. Begin a
        // new auth generation now so an older access response cannot reveal it.
        void refresh(true);
    });
    onAuthenticationChange(value => { void accept(value); });
    concealManagement(); render();
    return { refresh };
}
