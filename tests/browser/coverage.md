# Hub Web管理 GUI試験の範囲

2026-10-31更新。現行のWeb管理の操作と、その結果を使うDesktop/Runnerの境界を分けて確認する。以下は再利用する試験の対応表であり、全操作の受入完了表ではない。各実行のPASS/FAIL・バイナリーSHA-256・画像の目視結果は、その実行の`project_sandbox`レポートを正とする。

結合監査の現行入口は [HubとDesktopの結合監査](../../../moyAI/tests/desktop_e2e/HUB_DESKTOP_COVERAGE.md)。Hub browserの全ケース成功だけでは、Desktop・Runner・物理別PCの成功としない。通常のDesktop GUI smoke/regressionにもHub結合セットは含まれない。

## 監査の分母

画面名だけでなく、通常/空/不正入力/保存中/競合/対象削除/認証喪失/再起動を適用可能な操作に組み合わせる。以下の画面別表、公開管理操作の表、入力と副作用の境界を一緒に使用する。

- UIは `ui/index.html` と `ui/src/` のattach関数、動的行・dialog・details、`operationLabels`を照合する。buttonだけでなくフォームsubmit・select・checkbox・入力・download・clipboard・リンク・keyboardを含む。
- API/Rust tests、実ブラウザー、実Desktop、実Runner、物理別PC、手動入力/目視の列を区別する。APIを準備に使った状態を、その状態を作るGUIの成功へ読み替えない。
- 自動試験は現行の12 specファイルを列挙する。テスト件数・source line/branch coverage・機能/状態の網羅性は異なる指標。行に試験名があるだけで全負例が検査済みとはしない。
- 結果にはbinary/source identity、実行したtest、skip/失敗、目視画像、未実行条件、cleanupを付ける。未確認状態を率の分母から黙って除外しない。

## 管理画面と対応する試験

| 画面／操作・状態 | 現在のブラウザーシナリオで確認する内容 | 残るGUI確認 |
| --- | --- | --- |
| チームの初期設定 | `onboarding.spec.mjs`で利用者作成・password入力なしのproject作成、保存済みproject・PC用途から準備状態を再表示 | 実Desktopから最初の仕事までの結合はDesktop側のシナリオ |
| 実行PCで使えるソフト・機器の登録 | `shared-management.spec.mjs`でプロジェクトカードから編集し、別の読取要求で保存値と画面への反映を確認 | 宣言した機能の動作・ライセンス可用性は実行PCとDesktop側で確認 |
| 競合したプロジェクトの編集 | `shared-management.spec.mjs`で入力保持→最新値と比較→明示継続→通常保存、再競合、削除対象、遅延比較後の利用終了と新ticketで再開 | 実行結果はtask-local reportを参照。破壊的・世代依存の操作は対象を開き直す |
| 最初のPCへの接続手順 | `onboarding.spec.mjs`で設定・開始→受信許可案内→ファイル取得→実TLS申請・承認→Desktop診断案内、停止時の戻り、Gateway変更が任意で開始前に見えること | このケースはloopbackのprotocol fixture。実UAC・Firewall書込み・物理LAN到達・実AI応答を確認した意味ではない |
| PC参加と用途の一括判断 | `onboarding.spec.mjs`でWindowsユーザー名の参考表示、同じPCの操作／実行用途、project未選択時の用途無効化・選択解除後のPCのみの保存、非管理者の専用actor発行とPWなしのproject閲覧、並行更新後の部分保存と同一PC再開 | 複数の物理PC、申請元と現物の照合 |
| 自動更新とフォーカス | `device-management.spec.mjs`で手動更新なしの参加許可・PC名・用途・online・次の申請、native SELECTのクリック／Alt+ArrowDown／focus跨ぎTab、一覧／導入案内のfocus保持、削除された操作targetの退役。`stable-rows.spec.mjs`で存続行の移動時focus。`management-workflows.spec.mjs`で入力draft・caret保持と競合拒否 | headlessの結果と実画面操作は各証跡で区別。物理PCでのIME・ブラウザーのnative操作は実環境でも確認 |
| LAN候補・固定Gateway・受信許可 | 候補読み取り、固定Gateway、`firewall.spec.mjs`のloopback拒否・起動時の現在ルール照合・LAN候補の明示入力・取得済み社内IPv4での実ルール読取preview・入力変更時の確認無効化。試験はapply/remove/UACを操作しない | 実Firewall書込み・UAC取消／権限拒否・GPO・物理別PC到達は専用実環境の受入。模擬NetSecurityのPS5.1検証とは区別 |
| 最初に使う共有モデル | `management-workflows.spec.mjs`でMain/Sub/Approveの明示選択・atomic保存、Sub/Approveへの通常chatモデル指定、poll中のdraft/focus、再読込、別画面とのrevision競合、最後のモデル削除後の指定解除 | 新規Desktopへの初期選択と保存済み選択維持はconsumer側の結合。生成成功は別確認 |
| Hubログオン開始 | コマンド生成unit、remote management拒否、unitで登録状態のread-only表示 | Windowsユーザーの実Run登録／解除・再ログオン・設置先変更は未自動化。ブラウザー試験は実PCの登録を変更しない |
| ワンクリック起動（ブラウザー外の入口） | `tests/launch.rs` のCLI結合試験で初回・保存復帰・同時起動・ポート競合と復旧・IP変更を確認。ブラウザー試験の件数には含めない | CMD、スタートメニュー、既定ブラウザー、保存先移行は当該配布物の実機証跡で判定する。過去の操作ツール制約を現行仕様や恒久的な未対応と扱わない |
| 起動済みHubの案内・本体の稼働表示 | `launch-status.spec.mjs`で複数の起動要求が同じPIDを再利用し、ブラウザーに起動済みの案内を表示。モデル接続停止中も本体は稼働と表示し、本体終了後は応答未確認へ切替。案内は再読込で繰り返さない | 既定ブラウザーが開くタブの選び方はOS／ブラウザーに従う。タブ数を本体の多重起動とみなさない |
| ページ内移動、概要、接続状態 | 直接URL起動、ネットワーク・モデル・接続PC・履歴・Web管理への移動、モデル数とrevision反映、再接続、Tab／Enterでモデルリンクへ移動 | その他のリンクのkeyboard操作、再起動を伴うepoch変更・通信途絶からの回復、稼働中Gatewayの実通信表示 |
| 管理者認証と再開 | `authentication.spec.mjs`で起動ticket交換、利用終了、ticketの期限・再使用拒否、Hub再起動後のcookie保持と既存タブへの新ticket、遅延操作・別タブのsession置換を確認。未許可画面に再開方法を表示 | 管理cookieに旧8時間制限を想定しない。実時間の長期放置、実Desktopから外部HTTPS管理画面を開く経路、物理別PC/proxy/複数OS利用者は別 |
| Web管理 | 認証済み管理者へのローカルURL・稼働状態、760px幅、実clipboardへのローカルURLコピーと読取一致。既存HTTP公開URLは案内として表示 | clipboard拒否時のGUI、公開設定エラー表示。remote HTTPS／proxyは別の起動設定の試験が必要 |
| 従来の手動サーバー | 詳細展開、bind入力、token生成、開始・停止、Web管理の継続、required／短いtoken／不正文字・不正bind、256文字token、Gateway checkbox両値と実起動・停止 | token全境界、使用中ポート・Gateway起動失敗の表示 |
| モデル探索 | 四つのprofile切替、候補選択、HTTP(S)以外の拒否、404／空一覧と回復、前の接続先の遅延応答を破棄。blur/clickの重複探索・逆順応答も検査 | timeout・不正JSON・巨大一覧、実oMLX／LM Studioとの組合せ |
| AIホストのAPIキー | `upstream-authentication.spec.mjs`で登録時の認証、保存済み値の保持/置換/取消/明示解除、別host/profileへの送信前の再探索、遅い旧候補の無効化 | 実provider製品の認証、Hub Gatewayの生成要求、Desktopへの非漏洩は各層の別検査。入力欄非表示だけで秘密保持完了としない |
| モデル登録・一覧 | 表示名、tools明示許可、capacity最小値、登録、存在／モデルなし／メンテナンス表示、状態再確認、revision履歴 | 多数の配置での可読性、実際に実行中／待ちの表示、一覧にある未ロードモデルの実生成 |
| モデルの論理名・共有容量 | 既存の論理モデルと容量プールへ追加、capacity入力の有効無効、既存→新規への選択復帰、共有プール編集、4 profileで新規登録、capacity1024／1025、再読込 | 異なるホストの共有プール、能力の異なる配置、実並列要求による容量上限 |
| モデル編集dialog | 共通の表示名と登録先名を分けて保存、capacity・有効checkbox、背景クリックを無視、入力とfocusをpolling中に維持、削除の二段階確認／取消、Tab／Shift+Tabのfocus範囲・Escape取消と復帰・必須値／capacity検証・Space変更とキーボード保存。Desktopへの名称伝播は結合scenario | 二つの管理ページでの同一モデル編集・削除競合、IME、長い名前・狭い画面 |
| 端末ネットワーク開始・停止 | IPv4・port入力、開始、設定保存有効化、停止してもWebは稼働、停止中の操作disable、使用中portのエラー・入力保持・修正後の再開始 | IPv6、不正bind、Gatewayだけの起動失敗、他端末からの到達、Windows FW |
| 共通config | GUIダウンロード、URL・公開CA含有、秘密鍵非含有、実clipboardコピーとダウンロード内容の一致 | clipboard拒否時のGUI、ダウンロード取消、Desktop GUIのimportはcombined試験側 |
| 接続詳細・診断・証明書 | Gateway portの範囲検証と自動設定、固定port保存→ネットワーク再開→画面reload後の再開で実Gateway URL一致、実Hub／Gateway診断、FW項目の未実施表示、3回pollingで展開と入力を維持、証明書更新、旧診断の無効化、760px幅 | Hubプロセスの再起動後適用、証明書期限切れ・更新失敗、既存通信を持ったままの更新、手動長時間ちらつき観察 |
| 退役したグループ・方向付きルール | `control-audit.spec.mjs`でフォームの不存在を確認。保存済み所属・ルールの互換性はRust試験で確認 | 新規共有仕事はプロジェクトとPC用途を使用 |
| 接続PCの絞り込み | 全端末／承認待ち、空結果、PC名検索のdraft・focus、保存済みグループ／IP／識別ID検索とclear | 数百行の可読性・keyboard選択・scroll維持 |
| 参加承認 | 実TLS参加要求、PC名・プロジェクトの用途を確認して許可、承認済み表示、別端末とのID分離 | 期限切れ申請、承認待ちの名前競合が後から発生する場合、承認と他ページの削除競合 |
| PC名・詳細 | 空白／大文字小文字を含む同名変更の拒否、rename、required、close／Escape取消とreopen、review後のdraft保持、照合ID展開維持、背景クリックを無視。旧所属欄の非表示 | 160文字境界、dialogでの他ページ同時変更 |
| 端末の稼働・停止・削除 | mTLS生存確認／公開申告の差、停止・再許可、恒久失効、削除前の端末ID確認・取消・確定、別端末保持、削除済み一覧とreload | 実行中・終了未確認の削除拒否、実際のタスク停止、オフライン化の時間推移、失効後のDesktop GUI |
| 退役した参加コード | `control-audit.spec.mjs`でフォームの不存在を確認。保存形式の互換性はRust試験で確認 | 通常は共通接続ファイルの配布を使用 |
| MCP履歴の取得 | 端末行のshortcutが自動取得、種類選択、取得中disable、失敗から再取得、一覧・前後page・空結果、遅い旧種類の応答を破棄 | 別端末への選択変更と応答の競合、60秒timeout、100件超・大量保持・30分expiry |
| MCP履歴の詳細・保存 | 詳細ボタン、取得済み表示、truncated警告、HTMLを文字列として表示、Markdown保存、160行本文のTab／Shift+Tab・PageDown／Ctrl+End／Ctrl+Home・通常poll後のfocus/scroll保持、種類変更時のclear、停止端末から取得、削除済み端末のcached詳細 | 実Desktopの指示／実行履歴との一致、64KiB上限・全文案内、OS保存先選択の取消、再起動後のcache消失 |
| モデル接続の生存確認 | 現在のHub単独試験は空表示まで | Main/Sub/Approveの選択承認、待機・処理・再確認・切断表示は実Desktopの結合試験側。3用途すべての実生成まで確認した意味ではない |

## 公開されているプロジェクト・保守操作

`ui/src/shared_admin_state.ts`の公開操作を一つずつ対応付ける。ここで「不足」とするものは、backend testがないという意味ではなく、実GUIの当該効果まで確認するシナリオが不足しているという意味。

| 操作 | GUIで必要な状態と期待結果 | 現行の自動GUI検査 / 不足 |
| --- | --- | --- |
| `save_project` | 名称/概要/保管状態/PC用途、空/境界/競合/再競合/対象消失。別projectや既存個人権限を変更しない | shared-management、management-workflows、onboarding、control-audit。実行中の権限解除はD/R結合で別確認 |
| `retry_project_device` | failed対象だけを再試行。古いproject/device/generationを拒否し、未停止の実行を再開しない | 管理画面からの実再試行とRunner側の回復までのGUI検査が不足 |
| `set_environment_capabilities` | 既存値/空/重複/32件/UTF-8長、別環境への漏れなし | shared-managementで保存と再読取。境界値はunit、実機ソフトの利用可否は宣言の保存と別 |
| `delete_project` | 取消/確定、別画面の編集中、実行中/保持サービスあり | shared-managementで削除と古い編集の拒否。停止・ファイル保持までのD/R結合は不足 |
| `bind_device_principal` | 端末と利用者の対応変更/取消、保存直後の旧session失効 | Desktop shared-work結合で一部操作。Hub単独の全負例・実行中D/Rへの伝播は不足 |
| `update_user` | 表示名/管理者/無効化、最後の管理者、自分の認証喪失、競合 | 管理API/identity検査と別に、編集・失効の実GUI通しが不足 |
| `delete_user` | 確認/取消、最後の管理者、所属・端末・進行中仕事の保持/失効 | 実GUIからの削除とDesktop側の反映確認が不足 |
| `group_member` | 既存groupへの追加/解除、別利用者/別group、並行更新 | shared-managementで所属追加。解除と権限喪失中のD/R伝播は不足。group作成はfixture準備であり現行GUIにない |
| `project_member` | reader/contributor/manager/解除、別project、既存PC用途との分離 | shared-managementでmanager保存。全役割と解除後のD/R伝播は不足 |
| `group_project_role` | 既存groupの役割/解除、個人権限と合成、並行更新 | shared-managementでreader保存。合成と解除の実Desktop通しは不足 |
| `settle_attempt` | 停止確認・理由・正しいgeneration、取消、ページ送り、旧世代拒否 | shared-managementで確認なし拒否→確認保存→次の仕事の割当、旧試行報告拒否。Runnerはprotocol fixtureで実processを起動しない。実Runner停止・ページ送りは別 |
| `prune_shared_data` | preview、日付変更で確認解除、非空/参照中/未完了/再実行、実削除後の説明 | shared-managementで0件/非空整理、削除後の404、待機中入力の同内容再取得。日時は停止中に過去へ設定する明示fixture。Desktop保存画面は別 |

## 入力・副作用で追加する境界

| 面 | 必須の追加観点 | 現在の限界 |
| --- | --- | --- |
| 旧配置のWindows受信許可 | 旧ルール一覧/再読取/対象選択/確認/解除、キャンセル/拒否/競合/一部欠損。現在のHub以外を消さない | `firewall.spec.mjs`は現行preview中心。`admin/firewall/retired.rs`とPS試験、当該版の実UAC証跡を別に紐付ける |
| 自動起動 | 登録内容の照合、有効/無効、別配置競合、再ログオン後の起動先 | read-only表示やunit成功を実registry変更の証拠にしない |
| project編集のretry/比較 | 通信断で結果不明、同じrequest IDの受付確認、比較後再競合、認証切替 | 通常保存・revision競合以外に、結果不明retryのGUI試験が不足 |
| 保存と停止状態 | maintenanceの次page/最新へ、停止未確認の解放、非空整理、backup/restore案内 | 表示だけ/0件だけで実効果を合格にしない。復元後のD/Rは結合監査へ |
| 画面操作 | 各consumerのdownload/clipboard失敗と取消、長い名称/日本語IME/狭幅/高DPI | 一つのclipboard成功・760px assertion・Tab成功から他consumerへ外挿しない |

## 現行ブラウザーspecの索引

| spec | 主対象 |
| --- | --- |
| `authentication.spec.mjs` | ticket・cookie・再起動・失効・remote初回案内 |
| `control-audit.spec.mjs` | 画面リンク、動的control、退役フォーム、入力代替経路 |
| `device-management.spec.mjs` | PC承認/検索/用途/停止/削除、旧MCP履歴、poll中select |
| `firewall.spec.mjs` | 現在の受信許可preview、実LAN候補読取、配布前の確認 |
| `keyboard-recovery.spec.mjs` | model dialog、clipboard、port競合回復 |
| `launch-status.spec.mjs` | Hostの生存・既起動案内・多重起動要求 |
| `management-workflows.spec.mjs` | model探索/登録/編集、3用途標準、競合、network診断 |
| `onboarding.spec.mjs` | 初回案内、PC参加と用途、部分保存の再試行 |
| `shared-management.spec.mjs` | project/既存権限/保守、編集比較・削除・失効 |
| `stable-rows.spec.mjs` | 行の保持とfocus（単独DOMの検査。Hub API結合ではない） |
| `upstream-authentication.spec.mjs` | AIホストAPIキーと探索の世代管理 |
| `web-admin.spec.mjs` | 管理画面の基本操作、networkと公開config |

## 証拠の読み方

`control-audit.spec.mjs`の範囲は、全ページ内リンク／説明details、手動Gateway ON/OFFと入力検証、4 profile登録、logical/pool既存→新規、PC名dialog取消・review、照合ID・恒久失効detailsのpoll後の再展開、プロジェクトのPC用途、退役フォームの不存在、network不正bind、条件付き外部Web URLです。旧形式のinvite・rule・MCP受付を新規作成する準備は使用しません。上表の「残るGUI確認」は実GUI証跡が得られるまで自動で完了にしません。

Desktopとの結合は [Desktop E2E](../../../moyAI/tests/desktop_e2e/README.md) の現行登録シナリオと、その実行レポートを参照します。Hub単独のprotocol fixtureと実Desktopの操作、物理別PC、実LLMの品質は区別します。

- `management-workflows.spec.mjs`、`device-management.spec.mjs`、`keyboard-recovery.spec.mjs`の操作は実ブラウザーから実Rust管理APIへ到達する。独立した`hub.command`は読取確認専用。シナリオを追加しただけではGUI合格とはしない。
- provider fixtureはモデルのメタデータだけを返す。ツール呼出や生成の品質・応答時間の証拠にはならない。
- participant fixtureは実CSR署名・CA検証・証明書取得・mTLSを通すが、Desktopの設定画面・MCP実行・実際の履歴生成は操作していない。履歴本文は明示的なfixture。
- screenshot添付だけでは目視済みとはしない。各実行のRESULTSに目視した画像名と判定範囲を書く。DOMの横幅assertionも、全viewportで見た目が良いことの証明ではない。
- 通常の成功時にもtraceを残すには`--trace on`を指定する。fixtureは終了時のHub・取得済み全listenerの停止を検査し、後始末が失敗したら試験もFAILにする。
