import { invoke, reconnect } from "./management.ts";
import { capabilityLabel, changeSummary, deploymentStatus, DiscoveryOwner, gatewayStatus, modelCounts, reviewStatus } from "./state.ts";
import type { Discovery, DeploymentRow, Snapshot } from "./state.ts";
import { reconcileModelRows } from "./model_rows.ts";
import { attachDeviceNetwork } from "./network.ts";
import { attachHistory } from "./history.ts";
import { attachWebManagement } from "./web_admin.ts";
import { attachSharedAdmin } from "./shared_admin.ts";
import "./history.css";
import "./web_admin.css";
import "./style.css";
function el<T extends HTMLElement = HTMLElement>(id: string): T {
    const node = document.getElementById(id);
    if (!node)
        throw new Error(`Missing ${id}`);
    return node as T;
}
const input = (id: string) => el<HTMLInputElement>(id);
const select = (id: string) => el<HTMLSelectElement>(id);
const button = (id: string) => el<HTMLButtonElement>(id);
const escape = (text: unknown) => String(text ?? "").replace(/[&<>"']/g, char => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;"
})[char]!);
const time = (value: number | null) => value ? new Date(value).toLocaleTimeString("ja-JP", {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit"
}) : "未確認";
function setText(id: string, value: string | number) {
    el(id).textContent = String(value);
}
function markup(id: string, html: string) {
    if (el(id).innerHTML !== html)
        el(id).innerHTML = html;
}
function notify(message: string, error = false) {
    const target = el("notice");
    target.hidden = false;
    target.classList.toggle("error", error);
    target.textContent = message;
}
function errorText(error: unknown): string {
    return typeof error === "string" ? error : "操作を完了できませんでした。もう一度確認してください。";
}
let snapshot: Snapshot | null = null;
let pending: Snapshot | null = null;
let pollInFlight = false;
let launchNotice = window.location.hash === "#hub-already-running" ? "moyAI Hub は起動済みです。稼働中の Hub の管理画面を開きました。"
    : window.location.hash === "#hub-started" ? "moyAI Hub を起動しました。ブラウザーを閉じても Hub は動作を続けます。" : null;
let pointerActive = false;
const keys = new Set<string>();
let composing = false;
let editTarget: {
    row: DeploymentRow;
    revision: string;
} | null = null;
const dialog = el<HTMLDialogElement>("edit-dialog");
function interactionActive() {
    return pointerActive || keys.size > 0 || composing || dialog.open || document.activeElement instanceof HTMLSelectElement;
}
function flush() {
    if (pending && !interactionActive()) {
        const next = pending;
        pending = null;
        render(next);
    }
}
document.addEventListener("pointerdown", () => {
    pointerActive = true;
}, true);
for (const name of ["pointerup", "pointercancel", "lostpointercapture"]) {
    document.addEventListener(name, () => {
        pointerActive = false;
        queueMicrotask(flush);
    }, true);
}
document.addEventListener("keydown", event => keys.add(event.code), true);
document.addEventListener("keyup", event => {
    keys.delete(event.code);
    queueMicrotask(flush);
}, true);
document.addEventListener("compositionstart", () => {
    composing = true;
}, true);
document.addEventListener("compositionend", () => {
    composing = false;
    queueMicrotask(flush);
}, true);
document.addEventListener("focusout", () => queueMicrotask(flush));
window.addEventListener("blur", () => {
    pointerActive = false;
    keys.clear();
    composing = false;
    flush();
});
function options(id: string, rows: {
    id: string;
    label: string;
}[], placeholder: string) {
    const target = select(id);
    const html = `<option value="">${escape(placeholder)}</option>` + rows.map(row => `<option value="${escape(row.id)}">${escape(row.label)}</option>`).join("");
    if (target.innerHTML !== html) {
        const old = target.value;
        target.innerHTML = html;
        target.value = rows.some(row => row.id === old) ? old : "";
    }
}
function render(next: Snapshot) {
    pending = null;
    snapshot = next;
    const models = next.store.catalog.models, pools = next.store.catalog.capacity_pools;
    setText("metric-models", models.length);
    setText("nav-model-count", models.length);
    setText("metric-hosts", `${next.deployments.length} 件の登録先`);
    const confirmed = modelCounts(next.deployments);
    setText("metric-present", confirmed.present);
    setText("metric-unconfirmed", confirmed.unconfirmed);
    setText("metric-revision", next.store.revision);
    setText("server-badge", next.server.running ? "稼働中" : "停止中");
    el("server-badge").className = `badge ${next.server.running ? "green" : "gray"}`;
    button("start-server").hidden = next.server.running;
    button("stop-server").hidden = !next.server.running;
    input("bind").disabled = next.server.running;
    input("token").disabled = next.server.running;
    button("generate-token").disabled = next.server.running;
    input("enable-gateway").disabled = next.server.running;
    const gateway = next.gateway;
    const execution = gatewayStatus(gateway);
    setText("gateway-status", execution.label);
    setText("gateway-detail", execution.detail);
    el("gateway-summary").classList.toggle("attention", execution.attention);
    setText("hub-identity", `Hub ${next.store.hub_id}`);
    options("logical-model", models, "新しいモデルとして登録");
    options("capacity-pool", pools, "このホストに新しいグループを作成");
    reconcileModelRows(el<HTMLTableSectionElement>("model-rows"), next.deployments.map(row => {
        const d = row.deployment;
        const state = deploymentStatus(row);
        const logical = models.find(model => model.id === d.logical_model_id);
        const pool = pools.find(pool => pool.id === d.capacity_pool_id);
        return `<tr data-id="${escape(d.id)}"><td><strong>${escape(d.label)}</strong><small>${escape(d.provider_model)}<br/>${escape(d.endpoint)}</small></td><td><span class="badge ${state.color}"><i class="dot ${state.color}"></i>${state.label}</span><small class="status-reason">${state.reason}</small></td><td>${escape(logical?.capabilities.map(capabilityLabel).join(", ") || "未確認")}<small>${escape(logical?.label)}</small></td><td>${escape(pool?.max_concurrent ?? "—")} 枠<small>${escape(pool?.label)}</small></td><td>${time(row.last_observed_ms)}</td><td><button class="tiny-button" data-edit="${escape(d.id)}" aria-label="${escape(d.label)} を編集">編集</button></td></tr>`;
    }).join(""), button("refresh-models"));
    el("models-empty").hidden = next.deployments.length > 0;
    markup("client-rows", next.clients.map(client => {
        const stale = client.connection === "stale";
        const label = stale ? "切断" : ({
            idle: "操作待ち",
            waiting: "空き待ち",
            running: "処理中"
        })[client.activity];
        const color = stale ? "gray" : client.activity === "running" ? "blue" : client.activity === "waiting" ? "amber" : "green";
        const contextText = (context: string) => `${reviewStatus(client.contexts, context)}${client.contexts.find(row => row.context_id === context)?.active_model_id ? " / 割当中" : ""}`;
        return `<tr><td><strong>${escape(client.label)}</strong><small>${escape(client.id)}</small></td><td>${escape(client.observed_ip)}</td><td><span class="badge ${color}"><i class="dot ${color}"></i>${label}</span></td><td><small>Main: ${escape(contextText("main"))}</small><small>Side Chat: ${escape(contextText("side_chat"))}</small></td><td>${time(client.last_seen_ms)}</td></tr>`;
    }).join(""));
    el("clients-empty").hidden = next.clients.length > 0;
    markup("revision-rows", [...next.store.changes].reverse().map(change => `<div class="revision-row"><b>r${escape(change.revision)}</b><span>${escape(changeSummary(change))}</span><small>${new Date(change.changed_at_ms).toLocaleString("ja-JP")}</small></div>`).join(""));
    el("revisions-empty").hidden = next.store.changes.length > 0;
}
async function poll() {
    if (pollInFlight)
        return;
    pollInFlight = true;
    try {
        await deviceNetwork.refresh();
        await webManagement.refresh();
        await sharedAdministration.refresh();
        const next = await invoke<Snapshot>("hub_snapshot");
        setText("header-status", "Hub本体は稼働中");
        el("header-dot").className = "dot green";
        setText("management-status", "Hub本体に接続中");
        if (launchNotice) {
            notify(launchNotice);
            launchNotice = null;
            if (["#hub-started", "#hub-already-running"].includes(window.location.hash))
                window.history.replaceState(null, "", window.location.pathname + window.location.search);
        }
        if (interactionActive())
            pending = next;
        else
            render(next);
    }
    catch (error) {
        setText("header-status", "Hub本体の応答を確認できません");
        el("header-dot").className = "dot gray";
        setText("management-status", "Hub本体の応答を確認できません");
        notify(errorText(error), true);
    }
    finally {
        pollInFlight = false;
    }
}
interface Evidence {
    id: string;
    discovery: Discovery;
    observed_at_ms: number;
}
const discoveryOwner = new DiscoveryOwner();
let evidence: Evidence | null = null;
let timer: ReturnType<typeof setTimeout> | undefined;
let discovering: number | null = null;
function discoveryStatus(message: string, error = false) {
    const target = el("discovery-status");
    target.textContent = message;
    target.classList.toggle("error", error);
}
function invalidateDiscovery() {
    input("allow-tools").checked = false;
    discoveryOwner.invalidate();
    evidence = null;
    clearTimeout(timer);
    select("model").replaceChildren(new Option("モデル候補", ""));
    select("model").disabled = true;
    button("register").disabled = true;
    discoveryStatus("入力したホストを確認します…");
    if (input("endpoint").value.trim() && !composing)
        timer = setTimeout(() => void discover(), 650);
    else
        discoveryStatus("ホストを指定してください。");
}
input("endpoint").addEventListener("input", invalidateDiscovery);
select("profile").addEventListener("change", invalidateDiscovery);
input("endpoint").addEventListener("blur", () => {
    clearTimeout(timer);
    if (!evidence)
        void discover();
});
input("endpoint").addEventListener("compositionend", () => {
    clearTimeout(timer);
    timer = setTimeout(() => void discover(), 650);
});
button("discover").addEventListener("click", () => {
    clearTimeout(timer);
    discoveryOwner.invalidate();
    void discover();
});
async function discover() {
    const endpoint = input("endpoint").value.trim(), providerProfile = select("profile").value;
    if (!endpoint || composing)
        return;
    const generation = discoveryOwner.capture();
    if (discovering === generation)
        return;
    discovering = generation;
    evidence = null;
    button("register").disabled = true;
    select("model").disabled = true;
    discoveryStatus("ホストからモデル候補を取得しています…");
    try {
        const result = await invoke<Evidence>("hub_discover", {
            endpoint,
            providerProfile
        });
        if (!discoveryOwner.accepts(generation))
            return;
        evidence = result;
        select("model").replaceChildren(new Option("モデルを選択", ""), ...result.discovery.models.map(model => new Option(`${model.label} — ${model.id}`, model.id)));
        select("model").disabled = result.discovery.models.length === 0;
        discoveryStatus(result.discovery.models.length ? `${result.discovery.models.length} 件のモデルが見つかりました。モデルを選択してください。` : "利用できるモデルがありません。ホストにモデルを配置してください。");
    }
    catch (error) {
        if (discoveryOwner.accepts(generation))
            discoveryStatus(errorText(error), true);
    }
    finally {
        if (discovering === generation)
            discovering = null;
    }
}
select("model").addEventListener("change", () => {
    input("allow-tools").checked = false;
    const model = evidence?.discovery.models.find(model => model.id === select("model").value);
    button("register").disabled = !model;
    if (model && !input("label").value.trim())
        input("label").value = model.label;
});
select("capacity-pool").addEventListener("change", () => {
    input("capacity").disabled = Boolean(select("capacity-pool").value);
});
el<HTMLFormElement>("register-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    if (!evidence || !snapshot)
        return;
    button("register").disabled = true;
    const generation = discoveryOwner.capture();
    try {
        await invoke("hub_register", {
            request: {
                revision: snapshot.store.revision,
                evidence_id: evidence.id,
                endpoint: input("endpoint").value,
                provider_profile: select("profile").value,
                model_id: select("model").value,
                label: input("label").value,
                logical_model_id: select("logical-model").value || null,
                capacity_pool_id: select("capacity-pool").value || null,
                capacity: Number(input("capacity").value),
                allow_tools: input("allow-tools").checked
            }
        });
        notify("モデルを登録しました。カタログの変更は接続端末の再確認対象になります。");
        if (discoveryOwner.accepts(generation)) {
            select("model").value = "";
            input("label").value = "";
            input("allow-tools").checked = false;
        }
        await poll();
    }
    catch (error) {
        discoveryStatus(errorText(error), true);
    }
    finally {
        button("register").disabled = !evidence || !select("model").value;
    }
});
button("refresh-models").addEventListener("click", async () => {
    button("refresh-models").disabled = true;
    setText("refresh-models", "確認しています…");
    try {
        await invoke("hub_refresh");
        await poll();
        notify("モデルの状態を更新しました。");
    }
    catch (error) {
        notify(errorText(error), true);
    }
    finally {
        button("refresh-models").disabled = false;
        setText("refresh-models", "↻ 状態を確認");
    }
});
button("generate-token").addEventListener("click", () => {
    const bytes = crypto.getRandomValues(new Uint8Array(32));
    input("token").value = [...bytes].map(value => value.toString(16).padStart(2, "0")).join("");
    input("token").type = "text";
    notify("接続用トークンを生成しました。必要な端末へ安全に共有してください。起動後は画面から消去します。");
});
el<HTMLFormElement>("server-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    button("start-server").disabled = true;
    try {
        await invoke("hub_start", {
            bind: input("bind").value,
            token: input("token").value,
            enableGateway: input("enable-gateway").checked
        });
        input("token").value = "";
        input("token").type = "password";
        notify("Hub サーバーを開始しました。");
        await poll();
    }
    catch (error) {
        notify(errorText(error), true);
    }
    finally {
        button("start-server").disabled = false;
    }
});
button("stop-server").addEventListener("click", async () => {
    button("stop-server").disabled = true;
    notify("Hub の受付とモデル中継を停止しています…");
    try {
        await invoke("hub_stop");
        notify("サーバーを停止しました。接続トークンと端末登録は失効しています。");
        await poll();
    }
    catch (error) {
        notify(errorText(error), true);
        await poll();
    }
    finally {
        button("stop-server").disabled = false;
    }
});
el("model-rows").addEventListener("click", event => {
    const target = (event.target as Element).closest<HTMLButtonElement>("[data-edit]");
    if (!target || !snapshot)
        return;
    const row = snapshot.deployments.find(row => row.deployment.id === target.dataset.edit);
    if (!row)
        return;
    editTarget = {
        row,
        revision: snapshot.store.revision
    };
    input("edit-label").value = row.deployment.label;
    input("edit-enabled").checked = row.deployment.enabled;
    input("edit-capacity").value = String(snapshot.store.catalog.capacity_pools.find(pool => pool.id === row.deployment.capacity_pool_id)?.max_concurrent ?? 1);
    setText("edit-target", `${row.deployment.provider_model} · ${row.deployment.endpoint}`);
    setText("edit-error", "");
    setText("remove-model", "配置を削除");
    button("remove-model").dataset.confirm = "";
    dialog.showModal();
});
button("close-edit").addEventListener("click", () => dialog.close());
dialog.addEventListener("close", () => {
    editTarget = null;
    flush();
});
async function saveEdit(remove: boolean) {
    if (!editTarget)
        return;
    button("save-edit").disabled = true;
    button("remove-model").disabled = true;
    try {
        await invoke("hub_edit", {
            request: {
                revision: editTarget.revision,
                deployment_id: editTarget.row.deployment.id,
                label: input("edit-label").value,
                capacity: Number(input("edit-capacity").value),
                enabled: input("edit-enabled").checked,
                remove
            }
        });
        dialog.close();
        notify(remove ? "モデル配置を削除しました。" : "配置設定を保存しました。カタログの revision を更新しました。");
        await poll();
    }
    catch (error) {
        setText("edit-error", errorText(error));
    }
    finally {
        button("save-edit").disabled = false;
        button("remove-model").disabled = false;
    }
}
el<HTMLFormElement>("edit-form").addEventListener("submit", event => {
    event.preventDefault();
    void saveEdit(false);
});
button("remove-model").addEventListener("click", () => {
    if (button("remove-model").dataset.confirm === "yes")
        void saveEdit(true);
    else {
        button("remove-model").dataset.confirm = "yes";
        setText("remove-model", "削除を確定");
        setText("edit-error", "この配置を削除します。最後の配置の場合は論理モデルも削除され、端末の再設定が必要になります。");
    }
});
const history = attachHistory(el("mcp-history"));
const webManagement = attachWebManagement();
const sharedAdministrationHost = document.createElement("section");
sharedAdministrationHost.id = "shared-administration";
sharedAdministrationHost.className = "panel";
el("device-network").before(sharedAdministrationHost);
const sharedAdministration = attachSharedAdmin(sharedAdministrationHost);
document.addEventListener("click", event => {
    const target = (event.target as HTMLElement).closest<HTMLButtonElement>("button[data-network-history]");
    if (target && !target.disabled) history.showDevice(target.dataset.networkHistory!);
});
const deviceNetwork = attachDeviceNetwork(el("device-network"), el("network-clients-management"), interactionActive, network => {
    history.updateNetwork(network);
    sharedAdministration.updateDevices(network.devices);
    const pendingCount = network.join_requests.filter(request => request.status === "pending").length;
    setText("metric-clients", network.devices.length);
    setText("metric-clients-detail", `承認待ち ${pendingCount} 台 / オンライン ${network.devices.filter(device => device.online).length} 台`);
    setText("nav-client-count", network.devices.length + pendingCount);
    setText("nav-pending-count", `承認待ち ${pendingCount}`);
    el("nav-pending-count").hidden = pendingCount === 0;
});
button("management-reconnect").addEventListener("click", async () => {
    button("management-reconnect").disabled = true;
    setText("management-status", "Hub本体に再接続しています…");
    try {
        await reconnect();
        notify("Hub本体に再接続しました。直前の操作は自動で再実行されません。");
        await poll();
    } catch (error) {
        setText("management-status", "Hub本体の応答を確認できません");
        notify(errorText(error), true);
    } finally {
        button("management-reconnect").disabled = false;
    }
});
void poll();
setInterval(() => void poll(), 2000);
