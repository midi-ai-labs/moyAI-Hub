export interface NetworkSubject { kind: "device" | "group"; id: string }
export interface NetworkRule {
    id: string;
    from: NetworkSubject;
    to: NetworkSubject;
    usage: "direct" | "delegated";
    effect: "allow" | "deny";
    max_depth: number;
    profile_id: string | null;
}
export interface NetworkDevice {
    device_id: string;
    label: string;
    groups: string[];
    revoked: boolean;
    admission: "allowed" | "stopped" | "revoked";
    certificate_sha256: string;
    expires_at_ms: number;
    last_seen_ms: number | null;
    observed_ip: string | null;
    online: boolean;
    publications: { profile_id: string; name: string; enabled: boolean; mode: string; endpoint: string }[];
}
export interface NetworkJoinRequest {
    request_id: string;
    label: string;
    observed_ip: string;
    key_sha256: string;
    created_at_ms: number;
    expires_at_ms: number;
    status: "pending" | "expired";
}
export interface NetworkSnapshot {
    revision: string;
    hub_id: string;
    ca_certificate_pem: string;
    ca_sha256: string;
    gateway_port: number;
    server: { running: boolean; bind: string | null; gateway_bind: string | null;
        certificate: { sha256: string; expires_at_ms: string; renew_after_ms: string; last_success_ms: string; last_error: string | null } | null };
    devices: NetworkDevice[];
    join_requests: NetworkJoinRequest[];
    groups: { id: string; label: string }[];
    rules: NetworkRule[];
}

export interface NetworkDiagnostics {
    hub_id: string;
    metadata_endpoint: string;
    gateway_endpoint: string | null;
    gateway_ready: boolean;
    certificate: NonNullable<NetworkSnapshot["server"]["certificate"]>;
    gateway_certificate: NetworkSnapshot["server"]["certificate"];
}

export function networkDiagnosticTarget(snapshot: NetworkSnapshot): string {
    return JSON.stringify([snapshot.hub_id, snapshot.server.running, snapshot.server.bind,
        snapshot.server.gateway_bind, snapshot.server.certificate?.sha256 ?? null]);
}

export function networkDiagnosticMatches(result: NetworkDiagnostics, snapshot: NetworkSnapshot): boolean {
    return snapshot.server.running && result.hub_id === snapshot.hub_id
        && result.metadata_endpoint === snapshot.server.bind
        && result.gateway_endpoint === snapshot.server.gateway_bind
        && result.certificate.sha256 === snapshot.server.certificate?.sha256;
}

export function networkDiagnosticStages(result: NetworkDiagnostics, checkedAt: number): {
    label: string; status: "pass" | "fail" | "skipped"; detail: string;
}[] {
    const hubValid = BigInt(result.certificate.expires_at_ms) > BigInt(checkedAt);
    const gatewayValid = result.gateway_certificate !== null
        && BigInt(result.gateway_certificate.expires_at_ms) > BigInt(checkedAt);
    return [
        { label: "Hubの待受と証明書", status: hubValid ? "pass" : "fail", detail: hubValid
            ? `${result.metadata_endpoint} の待受と、有効なサーバー証明書を確認しました。`
            : "Hubの証明書の期限が切れています。証明書の更新を再試行してください。" },
        { label: "Gatewayとの内部接続", status: result.gateway_ready && gatewayValid ? "pass" : "fail", detail: result.gateway_ready && gatewayValid
            ? `${result.gateway_endpoint} の受付と、有効なサーバー証明書を確認しました。`
            : "Gatewayの受付または有効な証明書を確認できません。稼働状態を確認してください。" },
        { label: "別端末からの到達とWindowsファイアウォール", status: "skipped", detail:
            "Hub内の確認だけでは別端末から届くかは判定できません。Desktopの「Hubへの接続を診断」を実行してください。Windowsのファイアウォールでは、このHubの受信TCPポートとGatewayの受信TCPポートを確認してください。設定は自動で変更しません。" },
    ];
}

export function networkGatewayPort(value: string): number | null {
    const trimmed = value.trim();
    if (!trimmed) return 0;
    const port = Number(trimmed);
    return /^\d+$/.test(trimmed) && Number.isSafeInteger(port) && port >= 0 && port <= 65535 ? port : null;
}

export function networkCertificateStatus(certificate: NetworkSnapshot["server"]["certificate"], nowMs: number): { label: string; warning: boolean } {
    if (!certificate) return { label: "ネットワーク開始時に証明書を作成します。", warning: false };
    if (BigInt(certificate.expires_at_ms) <= BigInt(Math.trunc(nowMs))) return { label: "証明書の有効期限が切れています。更新状況を確認してください。", warning: true };
    if (certificate.last_error) return { label: "証明書を更新できませんでした。現在の証明書は有効期限まで使用できます。接続状態を確認し、更新を再試行してください。", warning: true };
    return { label: "証明書は有効です。期限の7日前から自動で更新します。", warning: false };
}

export function networkServerAddress(bind: string): { ip: string; port: string } {
    const url = new URL(bind);
    return { ip: url.hostname.replace(/^\[|\]$/g, ""), port: url.port || "443" };
}

/** Each editor keeps the revision the user actually began reviewing. */
export class NetworkEdits {
    snapshot: NetworkSnapshot | null = null;
    readonly revisions = new Map<string, string>();
    accept(snapshot: NetworkSnapshot): boolean {
        if (this.snapshot && BigInt(snapshot.revision) < BigInt(this.snapshot.revision)) return false;
        this.snapshot = snapshot;
        return true;
    }
    touch(editor: string): void {
        if (this.snapshot && !this.revisions.has(editor)) this.revisions.set(editor, this.snapshot.revision);
    }
    target(editor: string): string | null {
        return this.revisions.get(editor) ?? this.snapshot?.revision ?? null;
    }
    saved(editor: string): void { this.revisions.delete(editor); }
    reviewLatest(): void {
        if (this.snapshot) for (const editor of this.revisions.keys()) this.revisions.set(editor, this.snapshot.revision);
    }
}
export function networkSubject(value: string): NetworkSubject | null {
    const delimiter = value.indexOf(":");
    const kind = value.slice(0, delimiter);
    const id = value.slice(delimiter + 1);
    return delimiter > 0 && id && (kind === "device" || kind === "group") ? { kind, id } : null;
}
export function networkDeviceStatus(device: NetworkDevice): { label: string; color: string; detail: string } {
    if (!device.online) return { label: "オフライン", color: "gray", detail: "有効な端末接続を確認できません。" };
    return device.publications.some(profile => profile.enabled)
        ? { label: "オンライン", color: "green", detail: "受付申告あり。実際の利用には承認と接続ルールが必要です。" }
        : { label: "オンライン", color: "blue", detail: "受付の申告なし。端末の生存確認は届いています。" };
}
export function networkAdmissionStatus(device: NetworkDevice): { label: string; color: string; detail: string } {
    if (device.revoked || device.admission === "revoked") return { label: "恒久失効", color: "red", detail: "この端末の認証は元に戻せません。" };
    if (device.admission === "stopped") return { label: "停止", color: "amber", detail: "新規利用を停止中。登録情報は保持し、再許可できます。" };
    return { label: "許可", color: "green", detail: "Hubへの参加を許可しています。" };
}
export type NetworkClient = { kind: "request"; request: NetworkJoinRequest } | { kind: "device"; device: NetworkDevice };
export type NetworkClientFilter = "all" | "pending";
export function networkClients(snapshot: NetworkSnapshot, search: string, filter: NetworkClientFilter = "all"): NetworkClient[] {
    const term = search.trim().toLocaleLowerCase();
    const requests = snapshot.join_requests.filter(request => (!term || [request.label, request.request_id, request.observed_ip, request.key_sha256]
        .some(value => value.toLocaleLowerCase().includes(term))) && (filter !== "pending" || request.status === "pending"));
    return [
        ...requests.filter(request => request.status === "pending").map(request => ({ kind: "request" as const, request })),
        ...(filter === "pending" ? [] : networkDevices(snapshot, search).map(device => ({ kind: "device" as const, device }))),
        ...requests.filter(request => request.status === "expired").map(request => ({ kind: "request" as const, request })),
    ];
}
export type NetworkClientAction = {
    command: "hub_network_approve";
    label: "許可";
    args: { expectedRevision: string; requestId: string; label: string; groups: string[] };
} | {
    command: "hub_network_admission";
    label: "停止" | "再許可";
    args: { expectedRevision: string; deviceId: string; stopped: boolean };
};
/** The row action carries the exact revision and identity that were displayed. */
export function networkClientAction(client: NetworkClient, revision: string): NetworkClientAction | null {
    if (client.kind === "request") return client.request.status === "pending" ? {
        command: "hub_network_approve", label: "許可",
        args: { expectedRevision: revision, requestId: client.request.request_id, label: client.request.label, groups: [] },
    } : null;
    const device = client.device;
    if (device.revoked || device.admission === "revoked") return null;
    return { command: "hub_network_admission", label: device.admission === "stopped" ? "再許可" : "停止",
        args: { expectedRevision: revision, deviceId: device.device_id, stopped: device.admission !== "stopped" } };
}
export function networkDevices(snapshot: NetworkSnapshot, search: string): NetworkDevice[] {
    const term = search.trim().toLocaleLowerCase();
    return snapshot.devices.filter(device => !term || [device.label, device.device_id, device.observed_ip ?? "",
        ...device.groups.map(id => snapshot.groups.find(group => group.id === id)?.label ?? id)]
        .some(value => value.toLocaleLowerCase().includes(term)));
}
