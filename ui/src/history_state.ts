import type { NetworkDevice } from "./network_state.ts";

export type HistoryDirection = "instruction" | "execution";
export interface HistoryQuery { deviceId: string; direction: HistoryDirection; offset: number; anchor: string | null; id: string | null }
export interface HistoryRow {
    id: string; direction: HistoryDirection; created_at_ms: number; updated_at_ms: number | null;
    title: string; peer_label: string; target_label: string; state: string; stop_status: string;
    state_source: string; result_received: boolean; session_id: string; profile_id: string;
    job_id: string | null; root_task_id: string; device_path: string[]; can_stop: boolean;
}
export interface HistoryPage { rows: HistoryRow[]; next_offset: number | null; anchor: string | null }
export interface HistoryDetail { row: HistoryRow; markdown: string; truncated: boolean }
export interface HistorySnapshot {
    request_id: string; device_id: string; device_label: string; direction: HistoryDirection;
    offset: number; anchor: string | null; id: string | null;
    status: "pending" | "completed" | "failed" | "expired";
    requested_at_ms: number; completed_at_ms: number | null; expires_at_ms: number;
    response: HistoryPage | HistoryDetail | null; error: string | null;
    device_online: boolean; device_admission: string; cached: boolean;
}
export function historyQueryKey(query: HistoryQuery): string {
    return JSON.stringify([query.deviceId, query.direction, query.offset, query.anchor, query.id]);
}
export function historyResultMatches(query: HistoryQuery, result: HistorySnapshot): boolean {
    if (result.device_id !== query.deviceId || result.direction !== query.direction
        || result.offset !== query.offset || result.anchor !== query.anchor || result.id !== query.id) return false;
    if (result.response === null) return result.status !== "completed";
    if (query.id !== null) return "row" in result.response && result.response.row.id === query.id
        && result.response.row.direction === query.direction;
    return "rows" in result.response && result.response.rows.every(row => row.direction === query.direction);
}
export function historyDeviceCanRead(device: NetworkDevice | undefined): boolean {
    return Boolean(device?.online && !device.revoked && device.admission !== "revoked");
}
export function historySnapshotRetained(snapshot: HistorySnapshot, now = Date.now()): boolean {
    return snapshot.status === "completed" && snapshot.completed_at_ms !== null
        && Math.max(0, now - snapshot.completed_at_ms) < 30 * 60_000;
}
export function historyStateLabel(state: string): string {
    return ({ preparing: "準備中", accepted: "受付済み", awaiting_approval: "承認待ち", running: "実行中", completed: "完了", cancelling: "停止要求中",
        failed: "失敗", interrupted: "中断", cancelled: "停止", unknown: "状態未確認",
        submitting: "受付確認待ち", pending: "待機中" } as Record<string, string>)[state] ?? state;
}
/** Selection and request generation stay local; Rust owns retained snapshots. */
export class HistorySelection {
    generation = 0;
    query: HistoryQuery | null = null;
    requestId: string | null = null;
    begin(query: HistoryQuery): number {
        this.query = structuredClone(query); this.requestId = null; return ++this.generation;
    }
    clear(): void { this.query = null; this.requestId = null; this.generation += 1; }
    accept(generation: number, result: HistorySnapshot): boolean {
        if (generation !== this.generation || this.query === null || !historyResultMatches(this.query, result)
            || (this.requestId !== null && this.requestId !== result.request_id)) return false;
        this.requestId = result.request_id; return true;
    }
}
