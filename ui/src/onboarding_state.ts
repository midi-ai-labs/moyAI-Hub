import type { NetworkSnapshot } from "./network_state.ts";
import type { SharedAdminSnapshot } from "./shared_admin_state.ts";

export function projectPreparation(shared: SharedAdminSnapshot, network: NetworkSnapshot | null, id: string) {
  const project = shared.projects.find(p => p.id === id && !p.archived);
  const people = shared.project_memberships.filter(m => m.project_id === id && shared.users.some(u => u.user_id === m.user_id && !u.disabled));
  const assignments = (shared.project_devices ?? []).filter(row => row.project_id === id);
  const current = (device: string) => network?.devices.some(d => d.device_id === device && !d.revoked && d.admission === "allowed");
  const controllers = assignments.filter(row => row.can_control && current(row.device_id));
  const executors = assignments.filter(row => row.can_execute && current(row.device_id));
  return { project, people, controllers, executors,
    canSubmit: people.some(p => p.role !== "reader" && shared.users.some(u => u.user_id === p.user_id && !u.setup_pending)),
    foldersPrepared: executors.filter(row => row.preparation_state === "ready").length,
    pendingRequests: network?.join_requests.filter(row => row.status === "pending").length ?? 0 };
}

export interface OnboardingNextStep {
  kind: "checking" | "network" | "project" | "legacy" | "participants" | "person_setup" | "controllers" | "executors" | "execution_setup" | "preparing" | "preparation_failed" | "sample";
  title: string;
  owner: string;
  location: string;
  detail: string;
  action: { kind: "project" | "first"; label: string } | { kind: "link"; label: string; href: string } | null;
}

/** A suggested handoff from current settings; folder preparation is not an AI test. */
export function onboardingNextStep(shared: SharedAdminSnapshot, network: NetworkSnapshot | null, id: string): OnboardingNextStep {
  const p = projectPreparation(shared, network, id);
  const projectAction = { kind: "project", label: "参加者とPC用途を設定" } as const;
  const manager = { owner: "Hub管理者", location: "このHub管理画面" };
  const pcNames = (rows: { device_id: string }[]) => rows.map(row => network?.devices.find(device => device.device_id === row.device_id)?.label ?? row.device_id).join("、");
  const userNames = (rows: { user_id: string }[]) => rows.map(row => {
    const user = shared.users.find(person => person.user_id === row.user_id);
    return user?.display_name || user?.username || row.user_id;
  }).join("、");
  if (!network) return { kind: "checking", title: "Hubの接続状態を確認しています", ...manager, detail: "接続状態が届くと、次の操作と担当者を表示します。", action: null };
  if (!network.server.running) return { kind: "network", title: "Hubへの接続を開始してください", ...manager, detail: "参加するPCと通信できるLANのIPアドレスを設定し、接続の受付を開始してください。", action: { kind: "link", href: "#network-step-start", label: "接続先を設定する" } };
  if (!p.project) return { kind: "project", title: "最初のプロジェクトを作ってください", ...manager, detail: "一緒に仕事をする人と、依頼・実行に使うPCを選びます。利用者が未登録なら、先に「利用者を追加する」から登録してください。", action: { kind: "project", label: "プロジェクトを作る" } };
  if (p.project.legacy_devices_unrestricted) return { kind: "legacy", title: "従来のプロジェクト設定を確認してください", ...manager, detail: "このプロジェクトは従来のPC設定を使っています。参加者と操作・実行PCを確認してください。この案内では従来設定での利用可否を判定していません。", action: projectAction };
  const writers = p.people.filter(person => person.role !== "reader");
  if (!writers.length) return { kind: "participants", title: "仕事を依頼する人を指定してください", ...manager, detail: "このプロジェクトに、利用中のアカウントを「仕事の依頼・実行」または「プロジェクト管理」で追加してください。閲覧だけの人は依頼できません。", action: projectAction };
  if (!p.canSubmit) return { kind: "person_setup", title: "依頼する人の初回設定を済ませてください", owner: userNames(writers), location: p.controllers.length ? `${pcNames(p.controllers)} のDesktop` : "本人が利用するPCのDesktop", detail: "Hub管理者から受け取った本人設定コードで、パスワードを設定してください。コードがない・期限が切れた場合は、管理者が「利用者」から再発行します。", action: { kind: "link", href: "#shared-administration", label: "利用者・プロジェクトの管理へ" } };
  if (!p.controllers.length) return { kind: "controllers", title: "仕事を依頼するPCを指定してください", ...manager, detail: "参加を許可したPCを、このプロジェクトの操作PCに選んで保存してください。停止中・失効したPCは操作PCとして案内できません。", action: projectAction };
  if (!p.executors.length) return { kind: "executors", title: "仕事を実行するPCを指定してください", ...manager, detail: "参加を許可したPCを、このプロジェクトの実行PCに選んで保存してください。PC側の保存先・権限は、そのPCの担当者が確認します。", action: projectAction };
  const ready = p.executors.filter(row => row.preparation_state === "ready");
  if (ready.length) {
    const unfinished = p.executors.filter(row => row.preparation_state !== "ready");
    const requesters = writers.filter(person => !shared.users.find(user => user.user_id === person.user_id)?.setup_pending);
    return { kind: "sample", title: "Desktopからサンプルを依頼してください", owner: `${userNames(requesters)}（仕事を依頼する人）`, location: `${pcNames(p.controllers)} のDesktop`, detail: `「${p.project.label}」を開き、実行先に ${pcNames(ready)} を選んで、回答と成果ファイルを確認してください。作業フォルダーは作成済みですが、AIの接続・実行結果はこのサンプルで確認します。${unfinished.length ? ` ${pcNames(unfinished)} は準備が未完了です。まず作成済みのPCで試せます。` : ""}`, action: { kind: "first", label: "サンプルの依頼手順" } };
  }
  const failed = p.executors.filter(row => row.preparation_state === "failed");
  if (failed.length) return { kind: "preparation_failed", title: "実行PCの保存先を確認してください", owner: "実行PCの担当者 → Hub管理者", location: `${pcNames(failed)} のDesktop → このHub管理画面`, detail: "PCの担当者が保存先・権限と、下の「実行するPCとAI」にあるエラーを確認します。原因を解消したら、Hub管理者が管理画面の「プロジェクト」で「フォルダー作成を再試行」を押してください。", action: { kind: "link", href: "#shared-administration", label: "利用者・プロジェクトの管理へ" } };
  const waiting = p.executors.filter(row => row.preparation_state !== "pending");
  if (waiting.length) return { kind: "execution_setup", title: "実行PCで保存先と権限を確認してください", owner: "実行PCの担当者", location: `${pcNames(waiting)} のDesktop`, detail: "AIの接続先とモデルを設定し、「PCの接続」の「このPCで仕事を実行」で保存先を選び、「この設定で実行を許可」を押してください。本人アカウントへのログインは不要です。", action: null };
  return { kind: "preparing", title: "実行PCが作業フォルダーを作成しています", owner: "実行PCの担当者", location: `${pcNames(p.executors)} のDesktop`, detail: "Hubと実行PCを起動したままお待ちください。状態が変わらない場合は、実行PCのDesktopでHubへの接続と実行設定を確認してください。作成後は、依頼する人がサンプルを送ります。", action: null };
}
