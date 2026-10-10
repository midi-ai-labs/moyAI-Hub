# Hub Web管理 GUI試験の範囲

2026-10-10時点のHub Web管理にある現役の操作を対象にする。以下は再利用する試験の対応表であり、全操作の受入完了表ではない。各実行のPASS/FAIL・バイナリーSHA-256・画像の目視結果は、その実行の`project_sandbox`レポートを正とする。

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
| ワンクリック起動（ブラウザー外の入口） | `tests/launch.rs` のCLI結合試験で初回・保存復帰・同時起動・ポート競合と復旧・IP変更を確認。ブラウザー試験の件数には含めない | 起動アイコンからEdgeが開きHubが稼働するところまで実操作。初回入力→停止→アイコンで再開→ブラウザー終了後の継続は追加GUI受入待ち（2026-09-12はComputer UseのURL検査で停止） |
| 起動済みHubの案内・本体の稼働表示 | `launch-status.spec.mjs`で複数の起動要求が同じPIDを再利用し、ブラウザーに起動済みの案内を表示。モデル接続停止中も本体は稼働と表示し、本体終了後は応答未確認へ切替。案内は再読込で繰り返さない | 既定ブラウザーが開くタブの選び方はOS／ブラウザーに従う。タブ数を本体の多重起動とみなさない |
| ページ内移動、概要、接続状態 | 直接URL起動、ネットワーク・モデル・接続PC・履歴・Web管理への移動、モデル数とrevision反映、再接続、Tab／Enterでモデルリンクへ移動 | その他のリンクのkeyboard操作、再起動を伴うepoch変更・通信途絶からの回復、稼働中Gatewayの実通信表示 |
| 管理者認証 | `authentication.spec.mjs`で起動ticket交換、利用終了、期限・再使用の拒否、遅延操作・別タブのsession置換を確認。未許可画面にpassword入力を出さない | 物理別PCのHTTPS/proxyと複数OS利用者の運用 |
| Web管理 | 認証済み管理者へのローカルURL・稼働状態、760px幅、実clipboardへのローカルURLコピーと読取一致。既存HTTP公開URLは案内として表示 | clipboard拒否時のGUI、公開設定エラー表示。remote HTTPS／proxyは別の起動設定の試験が必要 |
| 従来の手動サーバー | 詳細展開、bind入力、token生成、開始・停止、Web管理の継続、required／短いtoken／不正文字・不正bind、256文字token、Gateway checkbox両値と実起動・停止 | token全境界、使用中ポート・Gateway起動失敗の表示 |
| モデル探索 | 四つのprofile切替、候補選択、HTTP(S)以外の拒否、404／空一覧と回復、前の接続先の遅延応答を破棄 | 認証が必要なprovider、timeout・不正JSON・巨大一覧、実oMLX／LM Studioとの組合せ |
| モデル登録・一覧 | 表示名、tools明示許可、capacity最小値、登録、存在／モデルなし／メンテナンス表示、状態再確認、revision履歴 | 多数の配置での可読性、実際に実行中／待ちの表示、一覧にある未ロードモデルの実生成 |
| モデルの論理名・共有容量 | 既存の論理モデルと容量プールへ追加、capacity入力の有効無効、既存→新規への選択復帰、共有プール編集、4 profileで新規登録、capacity1024／1025、再読込 | 異なるホストの共有プール、能力の異なる配置、実並列要求による容量上限 |
| モデル編集dialog | 表示名・capacity・有効checkbox、保存、背景クリックを無視、入力とfocusをpolling中に維持、削除の二段階確認／取消、Tab／Shift+Tabのfocus範囲・Escape取消と復帰・必須値／capacity検証・Space変更とキーボード保存 | 二つの管理ページでの同一モデル編集・削除競合、IME、長い名前・狭い画面 |
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
| モデル接続の生存確認 | 現在のHub単独試験は空表示まで | Main／Sideの選択承認、待機・処理・再確認・切断表示はactual Desktopを使うcombined試験側 |

## 証拠の読み方

`control-audit.spec.mjs`の範囲は、全ページ内リンク／説明details、手動Gateway ON/OFFと入力検証、4 profile登録、logical/pool既存→新規、PC名dialog取消・review、照合ID・恒久失効detailsのpoll後の再展開、プロジェクトのPC用途、退役フォームの不存在、network不正bind、条件付き外部Web URLです。旧形式のinvite・rule・MCP受付を新規作成する準備は使用しません。上表の「残るGUI確認」は実GUI証跡が得られるまで自動で完了にしません。

Desktopとの結合は [Desktop E2E](../../../moyAI/tests/desktop_e2e/README.md) の現行登録シナリオと、その実行レポートを参照します。Hub単独のprotocol fixtureと実Desktopの操作、物理別PC、実LLMの品質は区別します。

- `management-workflows.spec.mjs`、`device-management.spec.mjs`、`keyboard-recovery.spec.mjs`の操作は実ブラウザーから実Rust管理APIへ到達する。独立した`hub.command`は読取確認専用。シナリオを追加しただけではGUI合格とはしない。
- provider fixtureはモデルのメタデータだけを返す。ツール呼出や生成の品質・応答時間の証拠にはならない。
- participant fixtureは実CSR署名・CA検証・証明書取得・mTLSを通すが、Desktopの設定画面・MCP実行・実際の履歴生成は操作していない。履歴本文は明示的なfixture。
- screenshot添付だけでは目視済みとはしない。各実行のRESULTSに目視した画像名と判定範囲を書く。DOMの横幅assertionも、全viewportで見た目が良いことの証明ではない。
- 通常の成功時にもtraceを残すには`--trace on`を指定する。fixtureは終了時のHub・取得済み全listenerの停止を検査し、後始末が失敗したら試験もFAILにする。
