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
    certificate_sha256: string;
    expires_at_ms: number;
    last_seen_ms: number | null;
    observed_ip: string | null;
    online: boolean;
    publications: { profile_id: string; name: string; enabled: boolean; mode: string; endpoint: string }[];
}
export interface NetworkSnapshot {
    revision: string;
    hub_id: string;
    ca_certificate_pem: string;
    ca_sha256: string;
    server: { running: boolean; bind: string | null };
    devices: NetworkDevice[];
    groups: { id: string; label: string }[];
    rules: NetworkRule[];
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
    if (device.revoked) return { label: "失効", color: "red", detail: "新しい接続・受付は許可しません。実行中の停止完了とは別です。" };
    if (!device.online) return { label: "オフライン", color: "gray", detail: "端末の生存確認が届いていません。" };
    return device.publications.some(profile => profile.enabled)
        ? { label: "オンライン", color: "green", detail: "受付を公開中。実際の利用には接続許可と送信側の選択が必要です。" }
        : { label: "オンライン", color: "blue", detail: "受付は停止中。送信側としての利用は可能です。" };
}
export function networkDevices(snapshot: NetworkSnapshot, search: string): NetworkDevice[] {
    const term = search.trim().toLocaleLowerCase();
    return snapshot.devices.filter(device => !term || [device.label, device.device_id, device.observed_ip ?? "",
        ...device.groups.map(id => snapshot.groups.find(group => group.id === id)?.label ?? id)]
        .some(value => value.toLocaleLowerCase().includes(term)));
}
