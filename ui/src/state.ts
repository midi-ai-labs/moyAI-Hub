export type Availability = "unknown" | "offline" | "model_missing" | "ready" | "jit_pending" | "maintenance";
export interface DiscoveredModel {
    id: string;
    label: string;
    availability: Availability;
    capabilities: string[];
}
export interface Discovery {
    endpoint: string;
    provider_profile: string;
    models: DiscoveredModel[];
}
export interface LogicalModel {
    id: string;
    label: string;
    capabilities: string[];
}
export interface Pool {
    id: string;
    label: string;
    max_concurrent: number;
}
export interface Deployment {
    id: string;
    logical_model_id: string;
    capacity_pool_id: string;
    label: string;
    endpoint: string;
    provider_model: string;
    provider_profile: string;
    enabled: boolean;
}
export interface DeploymentRow {
    deployment: Deployment;
    availability: Availability;
    last_observed_ms: number | null;
    active_permits: number;
}
export interface ClientRow {
    id: string;
    label: string;
    observed_ip: string;
    last_seen_ms: number;
    connection: "connected" | "stale";
    activity: "idle" | "waiting" | "running";
    contexts: RouteContextRow[];
}
export interface RouteContextRow {
    context_id: string;
    review_required: boolean;
    reviewed_revision: string;
    active_model_id: string | null;
}

export function reviewStatus(contexts: RouteContextRow[], contextId: string): string {
    const context = contexts.find(value => value.context_id === contextId);
    if (!context) return "未確認";
    return context.review_required ? "再確認が必要" : `確認済み r${context.reviewed_revision}`;
}
export interface CatalogChange {
    revision: string;
    changed_at_ms: number;
    added_model_ids: string[];
    removed_model_ids: string[];
    changed_model_ids: string[];
    deployments_changed: boolean;
    policy_changed: boolean;
    software_changed: boolean;
}
export interface Snapshot {
    gateway?: { enabled: boolean; ready: boolean; uncertain: boolean; reserved: number; running: number | null; responding: number | null; draining: number | null; quarantined_pools: string[]; scope: "gateway_requests_only" };
    store: {
        hub_id: string;
        revision: string;
        catalog: {
            models: LogicalModel[];
            capacity_pools: Pool[];
        };
        changes: CatalogChange[];
    };
    deployments: DeploymentRow[];
    clients: ClientRow[];
    server: {
        bind: string | null;
        running: boolean;
    };
}

export function gatewayStatus(gateway: Snapshot["gateway"]): { label: string; detail: string; attention: boolean } {
    if (!gateway?.enabled) {
        return gateway?.uncertain
            ? { label: "実行を保留中", detail: "前回の処理終了を確認できません。接続先の実行終了を確認してから保留を解除してください。", attention: true }
            : { label: "モデル実行は停止中", detail: "モデル実行を有効にしてサーバーを開始すると、ここに応答と終了確認の状況を表示します。", attention: false };
    }
    if (gateway.running === null || gateway.responding === null || gateway.draining === null) {
        return { label: "実行状態を確認できません", detail: "終了が未確認のため、新しい実行を保留しています。", attention: true };
    }
    const counts = `応答中 ${gateway.responding} 件 / 上流終了確認待ち ${gateway.draining} 件 / 予約 ${gateway.reserved} 件`;
    if (gateway.uncertain) {
        return { label: counts, detail: `処理終了を確認できない実行枠グループ ${gateway.quarantined_pools.length} 件は、新しい実行を保留しています。`, attention: true };
    }
    return { label: counts, detail: gateway.draining > 0
        ? "端末への応答接続は閉じています。接続先の処理終了を確認するまで、実行枠を保持しています。"
        : "応答中は端末との接続を維持している要求です。停止や切断の後も、接続先の処理が終わるまで実行枠を保持します。", attention: gateway.draining > 0 };
}

export function loadCounts(rows: Pick<DeploymentRow, "availability">[]): { ready: number; unknown: number } {
    return { ready: rows.filter(row => row.availability === "ready").length,
        unknown: rows.filter(row => row.availability === "unknown").length };
}

export function capabilityLabel(capability: string): string {
    return capability === "tools" ? "ツール利用" : capability;
}
export const availability: Record<Availability, {
    label: string;
    color: string;
    reason: string;
}> = {
    ready: {
        label: "ロード済み",
        color: "blue",
        reason: "ホストがモデルのロード済み状態を報告"
    },
    jit_pending: {
        label: "未ロード",
        color: "green",
        reason: "モデルあり・未ロード。要求時の自動ロード設定は未確認"
    },
    model_missing: {
        label: "モデルなし",
        color: "red",
        reason: "ホストのモデル一覧に見つかりません"
    },
    offline: {
        label: "接続・確認不可",
        color: "gray",
        reason: "直近のモデル確認に失敗しました"
    },
    unknown: {
        label: "ロード状態不明",
        color: "gray",
        reason: "未確認、またはホストにロード情報なし"
    },
    maintenance: {
        label: "メンテナンス",
        color: "amber",
        reason: "管理者が割当候補から除外"
    },
};
// Each identity edit invalidates candidate ownership immediately, including during IME.
export class DiscoveryOwner {
    private generation = 0;
    invalidate(): number {
        return ++this.generation;
    }
    capture(): number {
        return this.generation;
    }
    accepts(generation: number): boolean {
        return generation === this.generation;
    }
}
export function changeSummary(change: CatalogChange): string {
    const parts: string[] = [];
    if (change.added_model_ids.length)
        parts.push(`モデル追加 ${change.added_model_ids.length} 件`);
    if (change.removed_model_ids.length)
        parts.push(`モデル削除 ${change.removed_model_ids.length} 件`);
    if (change.changed_model_ids.length)
        parts.push(`モデル設定変更 ${change.changed_model_ids.length} 件`);
    if (change.deployments_changed)
        parts.push("配置の変更");
    if (change.policy_changed)
        parts.push("実行枠・割当ルールの変更");
    if (change.software_changed)
        parts.push("Hub の更新");
    return parts.join(" / ");
}
