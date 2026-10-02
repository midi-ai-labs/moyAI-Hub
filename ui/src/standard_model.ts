import { invoke } from "./management.ts";
import type { Snapshot } from "./state.ts";

export function attachStandardModel(host: HTMLElement, refresh: () => Promise<void>) {
    const section = document.createElement("div"); section.className = "standard-model";
    section.innerHTML = `<h3>最初に使う共有モデル</h3><p>初めてHubのAIを使うPC向けのモデルです。利用者がすでに選んだモデルは変わりません。</p><label>初めて接続するPCのモデル<select id="team-standard-model"></select></label><button id="team-standard-save" class="secondary" type="button" disabled>標準モデルの設定を保存</button><p id="team-standard-status" role="status"></p>`;
    host.prepend(section);
    const select = section.querySelector<HTMLSelectElement>("select")!;
    const button = section.querySelector<HTMLButtonElement>("button")!;
    const status = section.querySelector<HTMLElement>("#team-standard-status")!;
    let snapshot: Snapshot | null = null, draftRevision: string | null = null, busy = false;
    function update(value: Snapshot) {
        snapshot = value;
        if (draftRevision || busy || document.activeElement === select) return;
        const chosen = value.store.catalog.team_default_model_id ?? "";
        const models = value.store.catalog.models.filter(model => model.capabilities.includes("tools"));
        select.replaceChildren(new Option("指定しない（登録済みの候補から自動選択）", ""), ...models.map(model => {
            const enabled = value.deployments.some(row => row.deployment.logical_model_id === model.id && row.deployment.enabled);
            const option = new Option(model.label + (enabled ? "" : "（有効な登録先なし）"), model.id);
            option.disabled = !enabled; return option;
        }));
        select.value = chosen;
        status.textContent = chosen ? `初めて接続するPCのモデル：${models.find(model => model.id === chosen)?.label ?? chosen}。` : "標準のモデルは未指定です。接続時に登録済みの候補から自動で選ばれます。";
        button.disabled = true;
    }
    select.addEventListener("change", () => { if (!snapshot || busy) return; draftRevision ??= snapshot.store.revision; button.disabled = false; status.textContent = "選択は未保存です。"; });
    button.addEventListener("click", async () => {
        if (!draftRevision || busy) return;
        busy = true; button.disabled = true; select.disabled = true;
        try {
            await invoke("hub_set_standard_model", { revision: draftRevision, modelId: select.value || null });
            draftRevision = null; busy = false; await refresh();
        } catch (error) {
            status.textContent = (typeof error === "string" ? error : "保存結果を確認できません。") + " ページを読み直して保存済みの選択を確認してください。";
        } finally { busy = false; select.disabled = false; button.disabled = !draftRevision; }
    });
    return { update };
}
