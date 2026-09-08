import { invoke } from "@tauri-apps/api/core";
import type { NetworkSnapshot } from "./network_state.ts";
import { HistorySelection, historyDeviceCanRead, historyQueryKey, historySnapshotRetained, historyStateLabel } from "./history_state.ts";
import type { HistoryDetail, HistoryDirection, HistoryPage, HistoryQuery, HistorySnapshot } from "./history_state.ts";

const escape = (value: unknown) => String(value ?? "").replace(/[&<>"']/g, char => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[char]!);
const when = (value: number | null) => value === null ? "未取得" : new Date(value).toLocaleString("ja-JP");

export function attachHistory(host: HTMLElement): { updateNetwork: (snapshot: NetworkSnapshot) => void; showDevice: (id: string) => void } {
    host.innerHTML = `<div class="section-title"><div><p class="eyebrow">端末の作業を確認する</p><h2 id="history-heading">MCP履歴</h2></div><span class="badge blue">読み取り専用</span></div>
      <p class="subtle">端末に保存された「MCP指示」と「MCP実行」を取得します。受付OFFの端末も確認できます。ここからタスクを実行・停止することはありません。</p>
      <div class="history-toolbar"><label>履歴を確認する端末<select id="history-device"><option value="">端末を選択してください</option></select></label>
        <label>履歴の種類<select id="history-direction"><option value="instruction">MCP指示 — この端末からの依頼</option><option value="execution">MCP実行 — この端末が受けた依頼</option></select></label>
        <button id="history-request" class="primary" type="button">履歴を取得</button></div>
      <p id="history-connection" class="footnote"></p><p id="history-notice" role="status" aria-live="polite" class="field-status"></p>
      <p id="history-acquired" class="footnote">端末を選んで「履歴を取得」を押してください。</p>
      <div class="table-wrap"><table class="history-table"><thead><tr><th>依頼・対象</th><th>相手端末・経路</th><th>状態・結果</th><th>記録日時</th><th>詳細</th></tr></thead><tbody id="history-rows"></tbody></table></div>
      <p id="history-empty" class="subtle" hidden>この種類の履歴はありません。</p>
      <div class="history-pagination"><button id="history-previous" class="secondary" type="button" disabled>前のページ</button><span id="history-page-label"></span><button id="history-next" class="secondary" type="button" disabled>次のページ</button></div>
      <section id="history-detail" class="history-detail" hidden aria-labelledby="history-detail-heading"><div class="section-title"><h3 id="history-detail-heading">履歴の詳細</h3><button id="history-export" class="secondary" type="button">Markdownで保存</button></div>
        <p id="history-detail-acquired" class="footnote"></p><p id="history-truncated" class="history-warning" hidden>この詳細は一部を省略しています。全体の確認・保存は元のDesktopの「MCP履歴」で行ってください。</p>
        <pre id="history-markdown" tabindex="0" aria-label="MCP履歴の詳細本文"></pre><p class="footnote">表示中の取得結果を、新しい .md ファイルへ保存します。同名の既存ファイルへは上書きしません。</p></section>
      <p class="footnote">取得結果はHubのメモリに最大64件・30分間保持します。Hub再起動で消えますが、各Desktopの履歴は残ります。未接続・旧バージョンの端末から取得できない場合は、その端末のDesktopで確認してください。</p>`;
    const node = <T extends HTMLElement = HTMLElement>(id: string) => host.querySelector<T>(`#${id}`)!;
    const deviceSelect = node<HTMLSelectElement>("history-device"), directionSelect = node<HTMLSelectElement>("history-direction");
    const owner = new HistorySelection(), cache = new Map<string, HistorySnapshot>();
    let network: NetworkSnapshot | null = null;
    let currentPage: { snapshot: HistorySnapshot; page: HistoryPage } | null = null;
    let currentDetail: { snapshot: HistorySnapshot; detail: HistoryDetail } | null = null;
    let previousOffsets: number[] = [], busy = false, exporting = false;
    let pollTimer: ReturnType<typeof setTimeout> | null = null;
    const query = (offset = 0, anchor: string | null = null, id: string | null = null): HistoryQuery =>
        ({ deviceId: deviceSelect.value, direction: directionSelect.value as HistoryDirection, offset, anchor, id });
    const message = (text: string, error = false) => { node("history-notice").textContent = text; node("history-notice").classList.toggle("error", error); };
    const stopPolling = () => { if (pollTimer !== null) clearTimeout(pollTimer); pollTimer = null; };
    function cachedResult(target: HistoryQuery): HistorySnapshot | undefined {
        const key = historyQueryKey(target), result = cache.get(key);
        if (result && !historySnapshotRetained(result)) { cache.delete(key); return undefined; }
        return result;
    }
    function expireSnapshots() {
        for (const [key, result] of cache) if (!historySnapshotRetained(result)) cache.delete(key);
        let expired = false;
        if (currentPage && !historySnapshotRetained(currentPage.snapshot)) {
            currentPage = null; previousOffsets = []; node("history-rows").replaceChildren();
            node("history-empty").hidden = true; node("history-page-label").textContent = "";
            node("history-acquired").textContent = "取得結果の保持期限が切れました。"; expired = true;
        }
        if (currentDetail && !historySnapshotRetained(currentDetail.snapshot)) {
            currentDetail = null; node("history-detail").hidden = true;
            node("history-markdown").textContent = ""; expired = true;
        }
        if (expired) message("取得結果の保持期限（30分）が切れました。端末が接続中であれば再取得してください。");
    }
    function controls() {
        expireSnapshots();
        const selected = network?.devices.find(device => device.device_id === deviceSelect.value), canRead = historyDeviceCanRead(selected);
        node<HTMLButtonElement>("history-request").disabled = busy || !canRead;
        node<HTMLButtonElement>("history-next").disabled = busy || !canRead || currentPage?.page.next_offset == null;
        node<HTMLButtonElement>("history-previous").disabled = busy || !canRead || previousOffsets.length === 0;
        node<HTMLButtonElement>("history-export").disabled = exporting || currentDetail === null;
        node("history-connection").textContent = selected ? `${selected.label} · ${selected.online ? "接続中" : "未接続"} · ${selected.revoked ? "恒久失効" : selected.admission === "stopped" ? "新規利用は停止中（履歴は確認可能）" : "参加を許可"}` : "";
        for (const button of node("history-rows").querySelectorAll<HTMLButtonElement>("button"))
            button.disabled = busy || (!canRead && !cachedResult(query(0, null, button.dataset.historyDetail!)));
    }
    function acquired(snapshot: HistorySnapshot): string {
        const online = network?.devices.find(device => device.device_id === snapshot.device_id)?.online ?? snapshot.device_online;
        return `取得日時: ${when(snapshot.completed_at_ms)} · ${snapshot.device_label} · ${online ? "端末は接続中" : "端末は未接続"} · 取得済みのスナップショット`;
    }
    function show(snapshot: HistorySnapshot) {
        if (snapshot.response === null) return;
        if ("rows" in snapshot.response) {
            currentPage = { snapshot, page: snapshot.response }; currentDetail = null; node("history-detail").hidden = true;
            const rows = snapshot.response.rows;
            node("history-rows").innerHTML = rows.map(row => `<tr data-history-id="${escape(row.id)}"><td><strong>${escape(row.title)}</strong><small>${escape(row.target_label)}</small><small class="network-mono">${escape(row.id)}</small></td><td>${escape(row.peer_label)}<small>${escape(row.device_path.join(" → "))}</small></td><td><span class="badge ${row.state === "completed" ? "green" : row.state === "failed" ? "red" : "gray"}">${escape(historyStateLabel(row.state))}</span><small>${row.state_source === "last_observed" ? "指示側に保存された最終確認" : "実行側の保存記録"}</small><small>${row.direction === "execution" ? "指示側の結果受信は、この端末では未確認" : row.result_received ? "結果を受信済み" : "結果の受信は未記録"}</small></td><td>${row.direction === "instruction" ? "指示" : "受付"}: ${when(row.created_at_ms)}<small>最終記録: ${when(row.updated_at_ms)}</small></td><td><button class="tiny-button" type="button" data-history-detail="${escape(row.id)}" aria-label="${escape(row.title)}の詳細を見る">詳細を見る</button></td></tr>`).join("");
            node("history-empty").hidden = rows.length !== 0; node("history-acquired").textContent = acquired(snapshot);
            node("history-page-label").textContent = rows.length ? `${snapshot.offset + 1}〜${snapshot.offset + rows.length} 件` : "0 件";
        } else {
            currentDetail = { snapshot, detail: snapshot.response }; node("history-detail").hidden = false;
            node("history-detail-heading").textContent = snapshot.response.row.title || "履歴の詳細";
            node("history-detail-acquired").textContent = acquired(snapshot); node("history-truncated").hidden = !snapshot.response.truncated;
            node("history-markdown").textContent = snapshot.response.markdown;
        }
        controls();
    }
    async function accept(generation: number, response: HistorySnapshot, offsets: number[]): Promise<void> {
        if (generation !== owner.generation) return;
        if (!owner.accept(generation, response)) { busy = false; message("取得した履歴の対象が一致しません。もう一度選択してください。", true); controls(); return; }
        if (response.status === "pending") {
            message("端末から履歴を取得しています。最大60秒お待ちください。");
            pollTimer = setTimeout(() => {
                pollTimer = null;
                void invoke<HistorySnapshot>("hub_history_result", { requestId: response.request_id }).then(next => accept(generation, next, offsets))
                    .catch(error => { if (owner.generation !== generation) return; busy = false;
                        message(typeof error === "string" ? error : "取得状態を確認できません。履歴を再取得してください。", true); controls(); });
            }, 2000); return;
        }
        busy = false;
        if (response.status === "completed") {
            const key = historyQueryKey(owner.query!); cache.delete(key); cache.set(key, response);
            if (cache.size > 64) cache.delete(cache.keys().next().value!);
            if (response.response && "rows" in response.response) previousOffsets = offsets;
            show(response); message("履歴を取得しました。");
        } else message(response.status === "expired" ? "端末から時間内に応答がありませんでした。接続とDesktopのバージョンを確認して再取得してください。" : "端末で履歴を取得できませんでした。端末のDesktopでも状態を確認してください。", true);
        controls();
    }
    async function request(target: HistoryQuery, offsets = previousOffsets) {
        stopPolling(); const generation = owner.begin(target); busy = true; controls();
        try { await accept(generation, await invoke<HistorySnapshot>("hub_history_request", { ...target }), [...offsets]); }
        catch (error) { if (owner.generation !== generation) return; busy = false; message(typeof error === "string" ? error : "履歴を取得できませんでした。", true); controls(); }
    }
    function changeSelection() {
        stopPolling(); owner.clear(); busy = false; currentPage = null; currentDetail = null; previousOffsets = [];
        node("history-rows").replaceChildren(); node("history-detail").hidden = true; node("history-empty").hidden = true;
        node("history-page-label").textContent = ""; node("history-acquired").textContent = "「履歴を取得」でこの端末の記録を確認します。"; message("");
        const cached = cachedResult(query());
        if (cached) { show(cached); message("以前に取得した履歴を表示しています。必要に応じて再取得してください。"); }
        controls();
    }
    deviceSelect.addEventListener("change", changeSelection); directionSelect.addEventListener("change", changeSelection);
    node("history-request").addEventListener("click", () => { void request(query(), []); });
    node("history-next").addEventListener("click", () => { if (currentPage && currentPage.page.next_offset !== null)
        void request(query(currentPage.page.next_offset, currentPage.page.anchor), [...previousOffsets, currentPage.snapshot.offset]); });
    node("history-previous").addEventListener("click", () => { const previous = previousOffsets.at(-1);
        if (previous !== undefined && currentPage) void request(query(previous, currentPage.page.anchor), previousOffsets.slice(0, -1)); });
    node("history-rows").addEventListener("click", event => {
        const button = (event.target as HTMLElement).closest<HTMLButtonElement>("button[data-history-detail]");
        if (!button || button.disabled) return;
        const target = query(0, null, button.dataset.historyDetail!);
        const cached = cachedResult(target);
        if (!historyDeviceCanRead(network?.devices.find(device => device.device_id === target.deviceId)) && cached) {
            stopPolling(); owner.begin(target); show(cached); message("端末は未接続のため、以前に取得した詳細を表示しています。");
        } else void request(target);
    });
    node("history-export").addEventListener("click", () => {
        expireSnapshots();
        if (!currentDetail || exporting) return;
        const target = currentDetail.snapshot.request_id, generation = owner.generation;
        exporting = true; controls();
        void invoke<string | null>("hub_history_export", { requestId: target }).then(saved => {
            if (generation === owner.generation && currentDetail?.snapshot.request_id === target) message(saved ? `保存しました: ${saved}` : "保存をキャンセルしました。");
        }).catch(error => { if (generation === owner.generation) message(typeof error === "string" ? error : "履歴を保存できません。", true); })
            .finally(() => { exporting = false; controls(); });
    });
    controls();
    return {
        updateNetwork(snapshot) {
            network = snapshot;
            if (deviceSelect !== document.activeElement) {
                const html = '<option value="">端末を選択してください</option>' + snapshot.devices.map(device => `<option value="${escape(device.device_id)}">${escape(device.label)}${device.online ? "" : "（未接続）"}</option>`).join("");
                if (deviceSelect.innerHTML !== html) { const selected = deviceSelect.value; deviceSelect.innerHTML = html; deviceSelect.value = selected;
                    if (selected && deviceSelect.value !== selected) changeSelection(); }
            }
            if (currentPage) node("history-acquired").textContent = acquired(currentPage.snapshot);
            if (currentDetail) node("history-detail-acquired").textContent = acquired(currentDetail.snapshot);
            controls();
        },
        showDevice(id) {
            if (!network?.devices.some(device => device.device_id === id)) return;
            deviceSelect.value = id; changeSelection(); location.hash = "mcp-history";
            if (historyDeviceCanRead(network.devices.find(device => device.device_id === id))) void request(query(), []);
        },
    };
}
