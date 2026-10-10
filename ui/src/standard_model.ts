import { invoke } from "./management.ts";
import type { Snapshot } from "./state.ts";

export type StandardModelRole = "main" | "side_chat" | "approve";
const roles = ["main", "side_chat", "approve"] as const;
const labels = { main: "Main", side_chat: "Sub（サイドチャット）", approve: "Approve（承認判定）" };
const fields = { main: "team_default_model_id", side_chat: "team_default_side_model_id", approve: "team_default_approve_model_id" } as const;
const ids = { main: "team-standard-model", side_chat: "team-standard-side-model", approve: "team-standard-approve-model" };

export function standardModelOptions(snapshot: Snapshot, role: StandardModelRole) {
    return snapshot.store.catalog.models.filter(model => role !== "main" || model.capabilities.includes("tools")).map(model => ({
        id: model.id, label: model.label,
        enabled: snapshot.deployments.some(row => row.deployment.logical_model_id === model.id && row.deployment.enabled),
    }));
}

export function attachStandardModel(host: HTMLElement, refresh: () => Promise<void>) {
    const section = document.createElement("div"); section.className = "standard-model";
    section.innerHTML = `<h3>最初に使う共有モデル</h3><p>初めてHubのAIを使うPC向けに、用途ごとのモデルを設定します。SubとApproveにはツールを使わないモデルも選べます。利用者がすでに選んだモデルは変わりません。</p>${roles.map(role => `<label>${labels[role]}<select id="${ids[role]}"></select></label>`).join("")}<button id="team-standard-save" class="secondary" type="button" disabled>標準モデルの設定を保存</button><p id="team-standard-status" role="status"></p>`;
    host.prepend(section);
    const selects = Object.fromEntries(roles.map(role => [role, section.querySelector<HTMLSelectElement>(`#${ids[role]}`)!])) as Record<StandardModelRole, HTMLSelectElement>;
    const button = section.querySelector<HTMLButtonElement>("button")!;
    const status = section.querySelector<HTMLElement>("#team-standard-status")!;
    let snapshot: Snapshot | null = null, draftRevision: string | null = null, busy = false;
    function update(value: Snapshot) {
        snapshot = value;
        if (draftRevision || busy || roles.some(role => document.activeElement === selects[role])) return;
        const descriptions = roles.map(role => {
            const chosen = value.store.catalog[fields[role]] ?? "";
            const models = standardModelOptions(value, role);
            selects[role].replaceChildren(new Option(role === "main" ? "指定しない（登録済みの候補から自動選択）" : "指定しない", ""), ...models.map(model => {
                const option = new Option(model.label + (model.enabled ? "" : "（有効な登録先なし）"), model.id);
                option.disabled = !model.enabled; return option;
            }));
            selects[role].value = chosen;
            return `${labels[role]}：${chosen ? models.find(model => model.id === chosen)?.label ?? chosen : "未指定"}`;
        });
        status.textContent = descriptions.join(" / ") + "。";
        button.disabled = true;
    }
    for (const select of Object.values(selects)) select.addEventListener("change", () => { if (!snapshot || busy) return; draftRevision ??= snapshot.store.revision; button.disabled = false; status.textContent = "選択は未保存です。"; });
    button.addEventListener("click", async () => {
        if (!draftRevision || busy) return;
        busy = true; button.disabled = true; for (const select of Object.values(selects)) select.disabled = true;
        try {
            await invoke("hub_set_standard_models", { revision: draftRevision, mainModelId: selects.main.value || null, sideModelId: selects.side_chat.value || null, approveModelId: selects.approve.value || null });
            draftRevision = null; busy = false; await refresh();
        } catch (error) {
            status.textContent = (typeof error === "string" ? error : "保存結果を確認できません。") + " ページを読み直して保存済みの選択を確認してください。";
        } finally { busy = false; for (const select of Object.values(selects)) select.disabled = false; button.disabled = !draftRevision; }
    });
    return { update };
}
