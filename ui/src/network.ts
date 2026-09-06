import { invoke } from "@tauri-apps/api/core";
import { NetworkEdits, networkDeviceStatus, networkDevices, networkServerAddress, networkSubject } from "./network_state.ts";
import type { NetworkSnapshot, NetworkSubject } from "./network_state.ts";
import "./network.css";

const escape = (value: unknown) => String(value ?? "").replace(/[&<>"']/g, char => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[char]!);
const when = (value: number | null) => value ? new Date(value).toLocaleString("ja-JP") : "未確認";

export function attachDeviceNetwork(host: HTMLElement, interactionActive: () => boolean): { refresh: () => Promise<void> } {
    host.innerHTML = `<div class="section-title"><div><p class="eyebrow">端末間のタスク委任</p><h2 id="network-heading">端末ネットワーク</h2></div><button id="network-refresh" class="secondary">最新情報を取得</button></div>
      <p class="subtle">端末を登録し、どの端末からどの端末へ依頼できるかを管理します。一つの端末で送信と受付の両方を使えます。</p>
      <p id="network-notice" class="field-status" role="status" aria-live="polite"></p>
      <div class="network-grid"><section class="network-card"><h3>1. ネットワークを開く <span id="network-server-status" class="badge gray">停止中</span></h3>
        <form id="network-server-form"><div class="network-fields"><label>このHubのIP<input id="network-ip" placeholder="例: 192.168.1.10" required autocomplete="off" spellcheck="false" /></label><label>ポート<input id="network-port" value="9471" inputmode="numeric" required /></label></div>
        <p class="footnote">端末から届く、このPCの具体的なIPを指定します。TLS証明書を自動で作成し、モデル接続とGatewayもまとめて開始・停止します。従来のモデルサーバーが稼働中の場合は先に停止してください。</p><div class="network-actions"><button id="network-start" class="primary">ネットワークを開始</button><button id="network-stop" type="button" class="danger" hidden>ネットワークを停止</button></div></form>
        <div class="network-actions"><button id="network-copy-config" type="button" class="secondary">共通設定をコピー</button><button id="network-save-config" type="button" class="secondary">設定ファイルを保存</button></div>
        <p class="footnote">共通設定にはHubのURLと公開CAだけが含まれます。各Desktopの「moyAI Hub → 端末連携」で読み込みます。</p>
        <details><summary>接続と証明書の詳細</summary><p id="network-identity" class="network-mono"></p><p id="network-ca" class="network-mono"></p></details></section>
      <section class="network-card"><h3>2. 端末の参加を許可する</h3><form id="network-invite-form"><label>登録する端末名<input id="network-invite-label" placeholder="例: Win19" maxlength="160" required autocomplete="off" /></label>
        <fieldset><legend>所属グループ（複数選択可）</legend><div id="network-invite-groups" class="network-memberships"></div></fieldset><button class="primary">参加コードを発行</button></form>
        <div id="network-invite-receipt" hidden><label>参加コード<input id="network-invite-code" readonly autocomplete="off" /></label><p id="network-invite-expiry" class="footnote"></p><button id="network-copy-invite" class="secondary">コードをコピー</button></div>
        <p class="footnote">管理者が許可した端末だけが参加できます。参加コードは10分以内に1回だけ使えます。失効した端末を自動で再登録しません。</p></section></div>
      <section class="network-card"><div class="section-title"><h3>登録端末</h3><label class="network-search">端末を検索<input id="network-search" type="search" placeholder="端末名・IP・グループ" /></label></div>
        <div class="table-wrap"><table class="network-table"><thead><tr><th>端末</th><th>状態・受付</th><th>所属グループ</th><th>最終確認</th><th>操作</th></tr></thead><tbody id="network-device-rows"></tbody></table></div><p id="network-devices-empty" class="subtle">登録端末はありません。参加コードを発行し、Desktopから参加してください。</p></section>
      <div class="network-grid"><section class="network-card"><h3>グループ</h3><p class="footnote">所属は排他的ではありません。例:「開発」「GPU搭載」の両方へ同じ端末を追加できます。</p>
        <form id="network-group-form"><label>編集するグループ<select id="network-group-id"><option value="">新しいグループを追加</option></select></label><label>グループ名<input id="network-group-label" maxlength="160" required /></label><button class="secondary">グループを保存</button></form><ul id="network-group-list" class="network-group-list"></ul></section>
      <section class="network-card"><h3>3. 方向付きの接続許可</h3><form id="network-rule-form"><div class="network-fields"><label>送信元<select id="network-rule-from" required></select></label><label>受入先<select id="network-rule-to" required></select></label></div>
        <div class="network-fields"><label>依頼の経路<select id="network-rule-usage"><option value="direct">直接の依頼</option><option value="delegated">再委任の宛先</option></select></label><label>扱い<select id="network-rule-effect"><option value="allow">許可する</option><option value="deny">拒否する（個別例外）</option></select></label></div>
        <label>公開対象<select id="network-rule-profile"><option value="">この宛先の公開対象すべて</option></select></label><label>最大の委任段数<select id="network-rule-depth"><option value="1">1段</option><option value="2">2段（例: 00 → 19 → 20）</option><option value="3">3段</option></select></label><button class="primary">接続ルールを追加</button></form>
        <p class="footnote">矢印は一方向です。逆向きは別に許可します。再委任には直前の端末からの直接許可と、最初の依頼元から宛先への再委任許可の両方が必要です。</p></section></div>
      <section class="network-card"><h3>接続ルールと個別例外</h3><div id="network-rule-list"></div><p class="footnote">端末の失効やルール変更は新しい受付を止めます。既に実行しているタスクの停止完了は、Desktopの委任経路と停止状況で確認してください。</p></section>
      <dialog id="network-device-dialog" aria-labelledby="network-device-title"><form id="network-device-form"><div class="section-title"><h2 id="network-device-title">端末を管理</h2><button id="network-device-close" type="button" class="icon-button" aria-label="閉じる">×</button></div>
        <p id="network-device-id" class="network-mono"></p><label>端末名<input id="network-device-label" maxlength="160" required /></label><fieldset><legend>所属グループ（複数選択可）</legend><div id="network-device-groups" class="network-memberships"></div></fieldset>
        <label class="network-check"><input id="network-device-revoked" type="checkbox" /><span>この端末の認証を失効する</span></label><p class="footnote">失効は元に戻せません。実行中のタスクの停止完了とは別です。再参加には新しい参加コードが必要です。</p><p id="network-device-error" class="field-status" role="status"></p><div class="network-actions"><button class="primary" id="network-device-save">変更を保存</button></div></form></dialog>`;
    const node = <T extends HTMLElement = HTMLElement>(id: string) => host.querySelector<T>(`#${id}`)!;
    const value = (id: string) => node<HTMLInputElement | HTMLSelectElement>(id).value;
    const message = (text: string, error = false) => { node("network-notice").textContent = text; node("network-notice").classList.toggle("error", error); };
    const owner = new NetworkEdits();
    let busy = false;
    let polling: Promise<boolean> | null = null;
    let requestSerial = 0;
    let selectedDevice: string | null = null;
    const dialog = node<HTMLDialogElement>("network-device-dialog");
    const groupsIn = (id: string) => [...node(id).querySelectorAll<HTMLInputElement>("input:checked")].map(input => input.value);
    const memberships = (id: string, selected: string[] = groupsIn(id)) => {
        const container = node(id);
        if (container.contains(document.activeElement)) return;
        const html = owner.snapshot!.groups.map(group => `<label class="network-check"><input type="checkbox" value="${escape(group.id)}" ${selected.includes(group.id) ? "checked" : ""}/><span>${escape(group.label)}</span></label>`).join("") || '<p class="footnote">グループは後から追加できます。</p>';
        if (container.innerHTML !== html) container.innerHTML = html;
    };
    const options = (id: string) => {
        const select = node<HTMLSelectElement>(id);
        if (select === document.activeElement) return;
        const snapshot = owner.snapshot!;
        const html = '<option value="">選択してください</option>' + snapshot.groups.map(group => `<option value="group:${escape(group.id)}">グループ: ${escape(group.label)}</option>`).join("")
            + snapshot.devices.filter(device => !device.revoked).map(device => `<option value="device:${escape(device.device_id)}">端末: ${escape(device.label)}</option>`).join("");
        if (select.innerHTML !== html) { const old = select.value; select.innerHTML = html; select.value = old; }
    };
    const subjectLabel = (subject: NetworkSubject) => subject.kind === "group"
        ? `グループ: ${owner.snapshot?.groups.find(group => group.id === subject.id)?.label ?? subject.id}`
        : `端末: ${owner.snapshot?.devices.find(device => device.device_id === subject.id)?.label ?? subject.id}`;
    function lock(): void {
        host.setAttribute("aria-busy", String(busy));
        for (const control of host.querySelectorAll<HTMLButtonElement | HTMLInputElement | HTMLSelectElement>("button,input,select")) control.disabled = busy || !owner.snapshot;
        node<HTMLButtonElement>("network-refresh").disabled = busy;
        node<HTMLInputElement>("network-ip").disabled = busy || !owner.snapshot || Boolean(owner.snapshot.server.running);
        node<HTMLInputElement>("network-port").disabled = busy || !owner.snapshot || Boolean(owner.snapshot.server.running);
        node<HTMLButtonElement>("network-device-revoked").disabled = busy || !owner.snapshot || Boolean(owner.snapshot.devices.find(device => device.device_id === selectedDevice)?.revoked);
    }
    function render(): void {
        const snapshot = owner.snapshot;
        if (!snapshot) return;
        node("network-server-status").textContent = snapshot.server.running ? "稼働中" : "停止中";
        node("network-server-status").className = `badge ${snapshot.server.running ? "green" : "gray"}`;
        node("network-start").hidden = snapshot.server.running;
        node("network-stop").hidden = !snapshot.server.running;
        if (snapshot.server.running && snapshot.server.bind && !value("network-ip")) {
            const address = networkServerAddress(snapshot.server.bind);
            node<HTMLInputElement>("network-ip").value = address.ip;
            node<HTMLInputElement>("network-port").value = address.port;
        }
        node("network-identity").textContent = `Hub: ${snapshot.hub_id} / ${snapshot.server.bind ?? "停止中"} / 更新 ${snapshot.revision}`;
        node("network-ca").textContent = `公開CA SHA-256: ${snapshot.ca_sha256}`;
        memberships("network-invite-groups");
        const groupSelect = node<HTMLSelectElement>("network-group-id");
        if (groupSelect !== document.activeElement) {
            const html = '<option value="">新しいグループを追加</option>' + snapshot.groups.map(group => `<option value="${escape(group.id)}">${escape(group.label)}</option>`).join("");
            if (groupSelect.innerHTML !== html) { const old = groupSelect.value; groupSelect.innerHTML = html; groupSelect.value = old; }
        }
        options("network-rule-from"); options("network-rule-to");
        const profileSelect = node<HTMLSelectElement>("network-rule-profile");
        if (profileSelect !== document.activeElement) {
            const to = networkSubject(value("network-rule-to"));
            const devices = snapshot.devices.filter(device => to?.kind === "device" ? device.device_id === to.id : to?.kind === "group" && device.groups.includes(to.id));
            const html = '<option value="">この宛先の公開対象すべて</option>' + devices.flatMap(device => device.publications.map(profile => `<option value="${escape(profile.profile_id)}">${escape(device.label)}: ${escape(profile.name)}</option>`)).join("");
            if (profileSelect.innerHTML !== html) { const old = profileSelect.value; profileSelect.innerHTML = html; profileSelect.value = old; }
        }
        const rows = networkDevices(snapshot, value("network-search"));
        const body = node("network-device-rows");
        if (!body.contains(document.activeElement)) body.innerHTML = rows.map(device => {
            const status = networkDeviceStatus(device);
            return `<tr><td><strong>${escape(device.label)}</strong><small>${escape(device.observed_ip ?? "IP未確認")}</small><details><summary>端末ID</summary><code>${escape(device.device_id)}</code></details></td><td><span class="badge ${status.color}">${status.label}</span><small>${escape(status.detail)}</small></td><td>${device.groups.map(id => `<span class="network-tag">${escape(snapshot.groups.find(group => group.id === id)?.label ?? id)}</span>`).join("") || "所属なし"}</td><td>${when(device.last_seen_ms)}</td><td><button type="button" class="tiny-button" data-network-edit="${escape(device.device_id)}" aria-label="${escape(device.label)}を管理">管理</button></td></tr>`;
        }).join("");
        node("network-devices-empty").hidden = rows.length > 0;
        node("network-devices-empty").textContent = snapshot.devices.length ? "検索に一致する端末はありません。" : "登録端末はありません。参加コードを発行し、Desktopから参加してください。";
        node("network-group-list").innerHTML = snapshot.groups.map(group => `<li><strong>${escape(group.label)}</strong><span>${snapshot.devices.filter(device => device.groups.includes(group.id)).length} 台</span></li>`).join("");
        const rules = node("network-rule-list");
        if (!rules.contains(document.activeElement)) rules.innerHTML = snapshot.rules.map(rule => `<article class="network-rule"><div><strong>${escape(subjectLabel(rule.from))} → ${escape(subjectLabel(rule.to))}</strong><p>${rule.usage === "direct" ? "直接の依頼" : "再委任の宛先"} · <span class="${rule.effect === "deny" ? "error" : ""}">${rule.effect === "allow" ? "許可" : "拒否"}</span> · 最大${rule.max_depth}段 · ${rule.profile_id ? escape(snapshot.devices.flatMap(device => device.publications).find(profile => profile.profile_id === rule.profile_id)?.name ?? rule.profile_id) : "公開対象すべて"}</p></div><button class="secondary" data-network-delete-rule="${escape(rule.id)}" data-network-revision="${escape(snapshot.revision)}" aria-label="${escape(subjectLabel(rule.from))}から${escape(subjectLabel(rule.to))}のルールを削除">削除</button></article>`).join("") || '<p class="subtle">接続ルールはありません。端末を参加させただけでは、ほかの端末へ依頼できません。</p>';
        lock();
    }
    async function readSnapshot(): Promise<boolean> {
        const serial = requestSerial;
        try {
            const snapshot = await invoke<NetworkSnapshot>("hub_network_snapshot");
            if (serial !== requestSerial || !owner.accept(snapshot)) return false;
            if (!interactionActive() && !dialog.open) render();
            return true;
        } catch { message("端末ネットワークの状態を取得できません。最新情報を取得してください。", true); return false; }
    }
    async function acquireSnapshot(): Promise<boolean> {
        if (busy) return false;
        if (!polling) polling = readSnapshot().finally(() => { polling = null; });
        return polling;
    }
    async function refresh(): Promise<void> {
        await acquireSnapshot();
    }
    async function perform(editor: string, name: string, args: Record<string, unknown>, receipt?: (result: unknown) => void): Promise<void> {
        if (busy || !owner.snapshot) return;
        ++requestSerial;
        busy = true; lock();
        try {
            const result = await invoke(name, args);
            owner.saved(editor);
            if (result && typeof result === "object" && "devices" in result) owner.accept(result as NetworkSnapshot);
            message("設定を更新しました。");
            receipt?.(result);
            await readSnapshot();
        } catch (error) {
            const text = typeof error === "string" ? error : "操作を完了できませんでした。入力内容を確認してください。";
            message(text, true); if (dialog.open) node("network-device-error").textContent = text;
        } finally { busy = false; lock(); render(); }
    }
    for (const id of ["network-invite-form", "network-group-form", "network-rule-form", "network-device-form"]) {
        node(id).addEventListener("input", () => owner.touch(id));
        node(id).addEventListener("change", () => owner.touch(id));
    }
    node("network-refresh").addEventListener("click", () => { void acquireSnapshot().then(accepted => { if (accepted) { owner.reviewLatest(); render(); message("最新の設定を取得しました。入力中の内容は保持しています。接続ルールを確認してから保存してください。"); } }); });
    node("network-search").addEventListener("input", render);
    node("network-server-form").addEventListener("submit", event => {
        event.preventDefault(); const ip = value("network-ip").trim();
        const bind = `${ip.includes(":") ? `[${ip.replace(/^\[|\]$/g, "")}]` : ip}:${value("network-port").trim()}`;
        void perform("server", "hub_network_start", { bind, advertiseIp: ip });
    });
    node("network-stop").addEventListener("click", () => void perform("server", "hub_network_stop", {}));
    const hubUrl = () => {
        const ip = value("network-ip").trim();
        return `https://${ip.includes(":") ? `[${ip.replace(/^\[|\]$/g, "")}]` : ip}:${value("network-port").trim()}`;
    };
    node("network-copy-config").addEventListener("click", () => void perform("export", "hub_network_export", { hubUrl: hubUrl() }, result => {
        const text = (result as { toml: string }).toml;
        void navigator.clipboard.writeText(text).then(() => message("公開情報だけの共通設定をコピーしました。"), () => message("コピーできませんでした。設定ファイルを保存してください。", true));
    }));
    node("network-save-config").addEventListener("click", () => void perform("export", "hub_network_save_config", { hubUrl: hubUrl() }, result => message(result ? "公開情報だけの共通設定を保存しました。Desktopへ渡してください。" : "設定ファイルの保存をキャンセルしました。")));
    node("network-invite-form").addEventListener("submit", event => {
        event.preventDefault(); void perform("network-invite-form", "hub_network_invite", {
            expectedRevision: owner.target("network-invite-form"), label: value("network-invite-label").trim(), groups: groupsIn("network-invite-groups"),
        }, result => {
            const receipt = result as { code: string; expires_at_ms: number };
            node<HTMLInputElement>("network-invite-code").value = receipt.code;
            node("network-invite-expiry").textContent = `有効期限: ${when(receipt.expires_at_ms)}（1回限り）`;
            node("network-invite-receipt").hidden = false;
        });
    });
    node("network-copy-invite").addEventListener("click", () => { void navigator.clipboard.writeText(value("network-invite-code")).then(() => message("参加コードをコピーしました。許可した端末へ渡してください。"), () => message("コードをコピーできませんでした。", true)); });
    node("network-group-form").addEventListener("submit", event => {
        event.preventDefault(); void perform("network-group-form", "hub_network_group", { expectedRevision: owner.target("network-group-form"), id: value("network-group-id") || null, label: value("network-group-label").trim() }, () => { node<HTMLInputElement>("network-group-label").value = ""; node<HTMLSelectElement>("network-group-id").value = ""; });
    });
    node("network-group-id").addEventListener("change", () => {
        node<HTMLInputElement>("network-group-label").value = owner.snapshot?.groups.find(group => group.id === value("network-group-id"))?.label ?? "";
    });
    node("network-rule-form").addEventListener("submit", event => {
        event.preventDefault(); const from = networkSubject(value("network-rule-from")), to = networkSubject(value("network-rule-to"));
        if (!from || !to) return;
        void perform("network-rule-form", "hub_network_rule", { expectedRevision: owner.target("network-rule-form"), rule: {
            id: null, from, to, usage: value("network-rule-usage"), effect: value("network-rule-effect"), max_depth: Number(value("network-rule-depth")), profile_id: value("network-rule-profile") || null,
        } });
    });
    node("network-rule-to").addEventListener("change", render);
    host.addEventListener("click", event => {
        const target = (event.target as Element).closest<HTMLElement>("[data-network-edit], [data-network-delete-rule]");
        if (!target || busy || !owner.snapshot) return;
        if (target.dataset.networkDeleteRule) { void perform("rule-delete", "hub_network_delete_rule", { expectedRevision: target.dataset.networkRevision, id: target.dataset.networkDeleteRule }); return; }
        const device = owner.snapshot.devices.find(device => device.device_id === target.dataset.networkEdit);
        if (!device) return;
        selectedDevice = device.device_id; owner.saved("network-device-form"); owner.touch("network-device-form");
        node("network-device-id").textContent = `端末ID: ${device.device_id}`;
        node<HTMLInputElement>("network-device-label").value = device.label;
        node<HTMLInputElement>("network-device-revoked").checked = device.revoked;
        memberships("network-device-groups", device.groups); node("network-device-error").textContent = ""; lock(); dialog.showModal();
    });
    node("network-device-close").addEventListener("click", () => dialog.close());
    dialog.addEventListener("cancel", event => { if (busy) event.preventDefault(); });
    node("network-device-form").addEventListener("submit", event => {
        event.preventDefault(); if (!selectedDevice) return;
        void perform("network-device-form", "hub_network_device", { expectedRevision: owner.target("network-device-form"), deviceId: selectedDevice,
            label: value("network-device-label").trim(), groups: groupsIn("network-device-groups"), revoked: node<HTMLInputElement>("network-device-revoked").checked,
        }, () => dialog.close());
    });
    lock();
    return { refresh };
}
