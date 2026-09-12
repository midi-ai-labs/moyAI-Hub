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
    model_present: boolean;
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
    gateway?: { enabled: boolean; ready: boolean; uncertain: boolean; stopping?: boolean; reserved: number; running: number | null; responding: number | null; draining: number | null; quarantined_pools: string[]; scope: "gateway_requests_only" };
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
        if (gateway?.stopping) return { label: "Hub のモデル中継を停止中", detail: "この Hub の通信を終了しています。モデルのロード・生成管理はホスト側で行います。", attention: true };
        return { label: "Hub のモデル中継は停止中", detail: "ネットワークを開始すると、この Hub が中継する通信の状況を表示します。", attention: false };
    }
    if (!gateway.ready || gateway.running === null) {
        return { label: "Hub の中継状態を確認できません", detail: "Hub の中継プロセスとの通信を確認してください。モデルホストの状態はホスト側で確認できます。", attention: true };
    }
    return { label: `中継中 ${gateway.running} 件 / 予約 ${gateway.reserved} 件`,
        detail: "この Hub 経由の通信件数です。モデルのロード・生成管理は oMLX や LM Studio などのホスト側で行います。", attention: false };
}

export function modelCounts(rows: Pick<DeploymentRow, "model_present">[]): { present: number; unconfirmed: number } {
    const present = rows.filter(row => row.model_present).length;
    return { present, unconfirmed: rows.length - present };
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
        label: "一覧あり",
        color: "green",
        reason: "ホストのモデル一覧で確認"
    },
    jit_pending: {
        label: "一覧あり",
        color: "green",
        reason: "ホストのモデル一覧で確認"
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
        label: "未確認",
        color: "gray",
        reason: "ホストのモデル一覧をまだ確認できていません"
    },
    maintenance: {
        label: "メンテナンス",
        color: "amber",
        reason: "管理者が割当候補から除外"
    },
};

export function deploymentStatus(row: Pick<DeploymentRow, "availability" | "model_present">): typeof availability[Availability] {
    if (row.availability === "maintenance") return availability.maintenance;
    if (row.model_present) return availability.ready;
    // Fresh model-list presence is the source for membership, not the legacy load hint.
    return row.availability === "ready" || row.availability === "jit_pending"
        ? availability.unknown : availability[row.availability];
}
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
        parts.push("同時接続数・割当ルールの変更");
    if (change.software_changed)
        parts.push("Hub の更新");
    return parts.join(" / ");
}
