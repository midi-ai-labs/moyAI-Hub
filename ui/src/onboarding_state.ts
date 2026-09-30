import type { NetworkSnapshot } from "./network_state.ts";
import type { SharedAdminSnapshot } from "./shared_admin_state.ts";

export function projectPreparation(shared: SharedAdminSnapshot, network: NetworkSnapshot | null, id: string) {
  const project = shared.projects.find(p => p.id === id && !p.archived);
  const assignments = (shared.project_devices ?? []).filter(row => row.project_id === id);
  const current = (device: string) => network?.devices.some(d => d.device_id === device && !d.revoked && d.admission === "allowed");
  const controllers = assignments.filter(row => row.can_control && current(row.device_id));
  const executors = assignments.filter(row => row.can_execute && current(row.device_id));
  const requesters = controllers.filter(controller => {
    const principal = shared.device_principals?.find(row => row.device_id === controller.device_id);
    if (!principal) return false;
    const user = shared.users.find(row => row.user_id === principal.user_id);
    if (!user || user.disabled || user.setup_pending) return false;
    const direct = shared.project_memberships.filter(row => row.project_id === id && row.user_id === user.user_id).map(row => row.role);
    const groups = shared.group_memberships.filter(row => row.user_id === user.user_id && shared.groups.some(group => group.id === row.group_id && !group.disabled)).map(row => row.group_id);
    const roles = [...direct, ...shared.group_project_roles.filter(row => row.project_id === id && groups.includes(row.group_id)).map(row => row.role)];
    return !roles.length || roles.some(role => role !== "reader");
  });
  return { project, controllers, executors, requesters, canSubmit: requesters.length > 0,
    foldersPrepared: executors.filter(row => row.preparation_state === "ready").length,
    pendingRequests: network?.join_requests.filter(row => row.status === "pending").length ?? 0 };
}

export interface OnboardingNextStep {
  kind: "checking" | "network" | "project" | "legacy" | "controller_access" | "controllers" | "executors" | "execution_setup" | "preparing" | "preparation_failed" | "models" | "sample";
  title: string;
  owner: string;
  location: string;
  detail: string;
  action: { kind: "project" | "first"; label: string } | { kind: "link"; label: string; href: string } | null;
}

/** A suggested handoff from current settings; folder preparation is not an AI test. */
export function onboardingNextStep(shared: SharedAdminSnapshot, network: NetworkSnapshot | null, id: string, modelCount: number | null): OnboardingNextStep {
  const p = projectPreparation(shared, network, id);
  const projectAction = { kind: "project", label: "PCの用途を設定" } as const;
  const manager = { owner: "Hub管理者", location: "このHub管理画面" };
  const pcNames = (rows: { device_id: string }[]) => rows.map(row => network?.devices.find(device => device.device_id === row.device_id)?.label ?? row.device_id).join("、");
  if (!network) return { kind: "checking", title: "Hubの接続状態を確認しています", ...manager, detail: "接続状態が届くと、次の操作と担当者を表示します。", action: null };
  if (!network.server.running) return { kind: "network", title: "Hubへの接続を開始してください", ...manager, detail: "参加するPCと通信できるLANのIPアドレスを設定し、接続の受付を開始してください。", action: { kind: "link", href: "#network-step-start", label: "接続先を設定する" } };
  if (!p.project) return { kind: "project", title: "最初のプロジェクトを作ってください", ...manager, detail: "プロジェクトに名前を付け、依頼・実行に使うPCを選びます。PCがまだ接続されていなくても、先にプロジェクトを作っておけます。", action: { kind: "project", label: "プロジェクトを作る" } };
  if (p.project.legacy_devices_unrestricted) return { kind: "legacy", title: "従来のプロジェクト設定を確認してください", ...manager, detail: "このプロジェクトは従来のPC設定を使っています。操作・実行PCを確認して保存してください。既存の利用者の権限は保持します。この案内では従来設定での利用可否を判定していません。", action: projectAction };
  if (!p.controllers.length) return { kind: "controllers", title: "仕事を依頼するPCを指定してください", ...manager, detail: "参加を許可したPCを、このプロジェクトの操作PCに選んで保存してください。停止中・失効したPCは操作PCとして案内できません。", action: projectAction };
  if (!p.canSubmit) return { kind: "controller_access", title: "操作PCの利用権限を確認してください", ...manager, detail: "操作PCの利用者が未登録、利用停止中、またはこのプロジェクトの閲覧だけに設定されています。「既存の利用者・権限（詳細）」で確認してください。", action: { kind: "link", href: "#shared-administration", label: "プロジェクトの管理へ" } };
  if (!p.executors.length) return { kind: "executors", title: "仕事を実行するPCを指定してください", ...manager, detail: "参加を許可したPCを、このプロジェクトの実行PCに選んで保存してください。PC側の保存先・権限は、そのPCの担当者が確認します。", action: projectAction };
  const ready = p.executors.filter(row => row.preparation_state === "ready");
  if (ready.length) {
    if (modelCount === null) return { kind: "checking", title: "Hubの登録モデルを確認しています", ...manager, detail: "モデルの一覧が届くと、次の操作を表示します。", action: null };
    if (modelCount === 0) return { kind: "models", title: "チームで使うAIをHubに登録してください", ...manager, detail: "作業フォルダーは準備済みですが、仕事に使うAIがHubに登録されていません。モデルを登録し、「最初に使う共有モデル」を選んでから、Desktopでサンプルを依頼してください。", action: { kind: "link", href: "#models", label: "共有AIを登録する" } };
    const unfinished = p.executors.filter(row => row.preparation_state !== "ready");
    return { kind: "sample", title: "Desktopからサンプルを依頼してください", owner: "操作PCを使う人", location: `${pcNames(p.requesters)} のDesktop`, detail: `「${p.project.label}」を開き、通常のチャット欄から依頼してください。使うPCはAIが準備済みのPC（${pcNames(ready)}）から判断します。特定のPCを使いたい場合は依頼文にPC名を書きます。作業フォルダーは準備済みですが、AIの接続・実行結果はこのサンプルで確認します。${unfinished.length ? ` ${pcNames(unfinished)} は準備が未完了です。` : ""}`, action: { kind: "first", label: "サンプルの依頼手順" } };
  }
  const failed = p.executors.filter(row => row.preparation_state === "failed");
  if (failed.length) return { kind: "preparation_failed", title: "実行PCの保存先を確認してください", owner: "実行PCの担当者 → Hub管理者", location: `${pcNames(failed)} のDesktop → このHub管理画面`, detail: "PCの担当者が保存先・権限と、下の「実行するPCとAI」にあるエラーを確認します。原因を解消したら、Hub管理者が管理画面の「プロジェクト」で「フォルダー作成を再試行」を押してください。", action: { kind: "link", href: "#shared-administration", label: "プロジェクトの管理へ" } };
  const waiting = p.executors.filter(row => row.preparation_state !== "pending");
  const localFolders = waiting.filter(row => row.preparation_state === "waiting_local_folder");
  if (localFolders.length) return { kind: "execution_setup", title: "実行PCでプロジェクトの作業フォルダーを選んでください", owner: "実行PCの担当者", location: `${pcNames(localFolders)} のDesktop`, detail: `Desktopの「moyAI Hub」→「PCの接続」を開きます。実行許可が未設定なら「1. このPCの実行許可」を設定し、「2. プロジェクトの作業フォルダー」で「${p.project.label}」の「作業フォルダーを選ぶ」を押してください。既存の成果物が入ったフォルダーも選べます。`, action: null };
  if (waiting.length) return { kind: "execution_setup", title: "実行PCで保存先と権限を確認してください", owner: "実行PCの担当者", location: `${pcNames(waiting)} のDesktop`, detail: "Hubのモデルを確認し、「PCの接続」の「このPCで仕事を実行」で保存先を選び、「この設定で実行を許可」を押してください。", action: null };
  return { kind: "preparing", title: "実行PCが作業フォルダーを作成しています", owner: "実行PCの担当者", location: `${pcNames(p.executors)} のDesktop`, detail: "Hubと実行PCを起動したままお待ちください。状態が変わらない場合は、実行PCのDesktopでHubへの接続と実行設定を確認してください。作成後は、依頼する人がサンプルを送ります。", action: null };
}
