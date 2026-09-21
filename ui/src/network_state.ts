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
    windows_username?: string | null;
    groups: string[];
    revoked: boolean;
    admission: "allowed" | "stopped" | "revoked";
    certificate_sha256: string;
    expires_at_ms: number;
    last_seen_ms: number | null;
    observed_ip: string | null;
    online: boolean;
    registered_ip?: string;
    name_conflict?: boolean;
    unsettled_jobs?: number;
    publications: { profile_id: string; name: string; enabled: boolean; mode: string; endpoint: string }[];
}
export interface NetworkJoinRequest {
    request_id: string;
    label: string;
    windows_username?: string | null;
    observed_ip: string;
    key_sha256: string;
    created_at_ms: number;
    expires_at_ms: number;
    status: "pending" | "expired";
    name_conflict?: boolean;
}
export interface NetworkSnapshot {
    revision: string;
    hub_id: string;
    ca_certificate_pem: string;
    ca_sha256: string;
    gateway_port: number;
    hosting: { configured: boolean; bind: string | null; advertise_ip: string | null; error: string | null };
    server: { running: boolean; bind: string | null; gateway_bind: string | null;
        certificate: { sha256: string; expires_at_ms: string; renew_after_ms: string; last_success_ms: string; last_error: string | null } | null };
    devices: NetworkDevice[];
    deleted_devices?: { device_id: string; label: string; deleted_at_ms: number }[];
    join_requests: NetworkJoinRequest[];
    groups: { id: string; label: string }[];
    rules: NetworkRule[];
}

/** Suggestions use observed Hub facts; an online PC is not proof of a usable LAN or AI. */
export function networkSetupStep(snapshot: NetworkSnapshot, firewall: "missing" | "matching" | "mismatch" | "conflict" | null = null): { title: string; detail: string; anchor: string; action: string } {
    if (!snapshot.server.running) return { title: "1. Hubの接続先を設定して開始", detail: "参加するPCと通信できるLANとHubポートを選びます。モデル通信用ポートは必要な場合だけ変更してください。", anchor: "network-step-start", action: "接続先を設定する" };
    if (firewall && firewall !== "matching") return { title: "2. Windowsの受信許可を確認", detail: firewall === "missing" ? "この設置先でmoyAIが管理する受信許可が未登録です。会社で設定済みの場合も含め、下の確認結果を確認してください。" : "現在のHubと受信許可の内容が一致しません。下の確認結果に沿って見直してください。", anchor: "network-step-firewall", action: "受信許可を確認する" };
    const pending = snapshot.join_requests.filter(request => request.status === "pending").length;
    if (pending) return { title: `4. 参加申請が${pending}台届いています`, detail: "申請元のPCを確かめて許可し、そのDesktopで接続を診断してください。", anchor: "clients", action: "参加申請を開く" };
    const allowed = snapshot.devices.filter(device => !device.revoked && device.admission === "allowed");
    if (allowed.length) return { title: "4. 参加したPCで接続を診断", detail: `参加を許可したPCは${allowed.length}台、現在接続中は${allowed.filter(device => device.online).length}台です。Desktopで接続を診断し、使えることを確かめたら残りのPCへ同じ接続ファイルを渡します。`, anchor: "network-step-confirm", action: "Desktopでの確認手順を見る" };
    if (firewall === "matching") return { title: "3. 接続ファイルの配布", detail: "現在のHubへの受信許可を確認しました。参加するPCへ接続ファイルを渡し、そのDesktopで接続を確認してください。", anchor: "network-step-file", action: "接続ファイルを保存する" };
    return { title: "2. 受信許可を設定してPCを接続", detail: "Hubは起動しています。Windowsの受信許可を設定し、参加するPCへ接続ファイルを渡してください。", anchor: "network-step-firewall", action: "受信許可の手順へ" };
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
            "接続するPCのDesktopで「Hubへの接続を診断」を実行してください。仕事を実行するPCでは「PCの接続 → 接続の詳細・診断 → このPCでHubのAIを使う場合の診断」も実行します。接続できない場合は、HubとGatewayの受信TCPポートがファイアウォールで許可されているか確認してください。この診断は設定を変更しません。" },
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

export function networkHostingAddress(snapshot: NetworkSnapshot): { ip: string; port: string } | null {
    if (snapshot.server.running && snapshot.server.bind) return networkServerAddress(snapshot.server.bind);
    const hosting = snapshot.hosting;
    if (!hosting.configured || !hosting.bind || !hosting.advertise_ip) return null;
    return { ip: hosting.advertise_ip, port: networkServerAddress(`https://${hosting.bind}`).port };
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
    serverAddress(): { ip: string; port: string } | null {
        return this.snapshot && !this.revisions.has("network-server-form") ? networkHostingAddress(this.snapshot) : null;
    }
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
    if (!device.online) return { label: "オフライン", color: "gray", detail: "このPCからの接続がありません。" };
    return { label: "オンライン", color: "green", detail: "このPCはHubに接続しています。依頼や実行に使うプロジェクトを設定してください。" };
}
/** Capture the exact reviewed device, including when its label is duplicated. */
export function networkDeviceDeletion(device: NetworkDevice, revision: string): {
    args: { expectedRevision: string; deviceId: string }; identity: string; warning: string;
} {
    return {
        args: { expectedRevision: revision, deviceId: device.device_id },
        identity: `${device.label}\n端末ID: ${device.device_id}\n接続時のIPアドレス: ${device.observed_ip ?? "未確認"}\n登録IP: ${device.registered_ip ?? "未確認"}\n接続: ${device.online ? "オンライン" : "オフライン"}\n証明書 SHA-256: ${device.certificate_sha256}`,
        warning: device.unsettled_jobs ? `受入済みタスク ${device.unsettled_jobs} 件の終了が未確認です。Desktopで停止または完了を確認してから再試行してください。`
            : "このPCの登録と個別の接続ルールを削除し、現在の証明書を無効にします。過去の履歴とPCの登録IDは残ります。再び接続するには、Desktopで新しい鍵を作って参加を申請し、管理者が許可する必要があります。進行中のモデル通信やPC上の処理は、この操作では止まりません。",
    };
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
    const requests = snapshot.join_requests.filter(request => (!term || [request.label, request.windows_username ?? "", request.request_id, request.observed_ip, request.key_sha256]
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
    if (client.kind === "request") return client.request.status === "pending" && !client.request.name_conflict ? {
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
    return snapshot.devices.filter(device => !term || [device.label, device.windows_username ?? "", device.device_id, device.observed_ip ?? "",
        ...device.groups.map(id => snapshot.groups.find(group => group.id === id)?.label ?? id)]
        .some(value => value.toLocaleLowerCase().includes(term)));
}
