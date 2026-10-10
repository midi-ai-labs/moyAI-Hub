# Hubのブラウザー試験

Playwrightで実際のHubサーバーとブラウザーを起動します。共有fixtureは隔離したHubのnative Hostで一度限りの起動ticketを発行し、実際の `/admin/access` で管理cookieへ交換します。認証専用ケースはこの準備を無効にし、起動URL・利用終了・再起動ticket・旧sessionの失効を画面から操作します。管理APIのmock、保存データの直接編集、証明書の検証無効化は行いません。モデルホストのメタデータと参加端末の応答は、明示した試験用provider・participantを使います。

## 準備と実行

Hubリポジトリで実行します。CIと同じNode.js 24、およびRustのビルド環境が必要です。Windowsではインストール済みのEdgeを使うため、通常は試験用ブラウザーの追加ダウンロードは不要です。

```powershell
npm ci
npm run verify:gui
npm run verify:gui -- --suite regression
```

`verify:gui`は単体試験（`npm test`）、TypeScript／Viteビルド、Rustバイナリービルド、実ブラウザーE2Eを順に実行します。途中で失敗すると後続段階は実行しません。画面資産はRustビルド時に同梱されるため、この順番を固定し、Cargoが報告した同じ実行ファイルを試験します。Rustの単体・結合試験はこの入口に含まず、変更面に応じた`cargo test`を別に実行してください。

既定の`smoke`は`@smoke`を付けた3ケースです。タイトルの部分一致による選択は使いません。

| suite | 実行する範囲 |
| --- | --- |
| `smoke` | Web起動・ネットワーク開始停止・共通config保存、モデル探索・登録・編集・削除、PC承認・名称重複検証・用途・停止・削除 |
| `regression` | 現行の全ケース。smokeに加え、ticket認証、更新競合、履歴、キーボード、clipboard、ポート競合、各種controlの追加操作 |

単体件数をGUI件数に加算しません。GUIは0件、skip、flaky、失敗、smokeの3件不足・過剰を成功にしません。全段階が終わる前の要約は`incomplete`で、単体やビルドだけの成功を全体成功にしません。既存fixtureの終了確認に失敗した場合もPlaywrightが失敗を返します。新しいケースを追加したら、smokeへ昇格する必要があるかを判断し、通常はregressionで実行します。

現在のビルドを意図的に再利用する場合は、E2Eだけの入口を使えます。

```powershell
npm run test:gui
npm run test:gui -- --suite regression
```

`test:gui`は**ソースと実行ファイルの一致を検証しません**。UI／Rust変更後は`verify:gui`を使ってください。既定は`target/debug/moyai-hub`（Windowsでは`.exe`）で、`MOYAI_HUB_TEST_BINARY`に別の既存ファイルの絶対パスを指定できます。`verify:gui`はこの環境変数より、今回Cargoが生成・確認したファイルを優先します。レポートには実行ファイルのSHA-256と、今回ビルドか既存ビルドかを残します。

両入口は既定で画面を表示します。CIなどで表示が不要なら`--headless`を明示します。ブラウザーは実際に起動し、管理画面を操作しますが、この成功は画像の目視判定とは別です。

```powershell
npm run verify:gui -- --suite smoke --headless
npm run test:gui -- --artifact-parent C:\path\to\project_sandbox\hub-gui
```

既存の低水準Playwright入口も、個別ケースの調査や成功時のtrace保存に使えます。これはunit／buildや段階別要約を実行しません。既定は全regressionです。

```powershell
npm run test:browser:headed -- tests/browser/keyboard-recovery.spec.mjs
npm run test:browser -- --trace on
```

ブラウザーは`MOYAI_HUB_BROWSER_CHANNEL`で`msedge`、`chrome`、`chromium`を選べます。Windows以外の既定値は`chromium`です。Playwright用Chromiumを使う場合だけ、次の追加準備を行います。

```powershell
npx playwright install chromium
$env:MOYAI_HUB_BROWSER_CHANNEL = "chromium"
npm run verify:gui -- --headless
```

## 確認範囲

現行画面の操作・状態と未確認項目は [coverage.md](coverage.md) に整理しています。次のシナリオを実行します。

- 公開CLIの`--data-dir <新しい保存先> --web-port 0`でHubを起動し、ブラウザーから管理画面を直接開く。
- ブラウザー操作でloopbackの端末ネットワークとGatewayを開始する。
- プロジェクトと操作・実行PCの用途を保存し、再読込後の画面と同じ実Hubへの別の読取要求で結果を確認する。
- 共通configをダウンロードし、案内先URLと公開証明書が含まれることを確認する。
- ブラウザー操作で端末ネットワークを停止し、その後もWeb管理画面を利用できることを確認する。
- 再接続、従来の手動サーバー開始・停止、760px幅での表示と横スクロールの有無を確認する。
- 管理者の認証CookieがHttpOnly・SameSite=Strictで、JavaScriptから読み取れないことと、JavaScriptの未処理エラーがないことを確認する。
- 四つのprovider profileでモデルを探索し、登録・編集・メンテナンス・削除・catalog revision・共有容量を確認する。空の一覧、接続失敗からの回復、接続先変更後に遅れて返る旧ホストの候補も確認する。
- Main/Sub/Approveの標準モデルを実画面からまとめて保存し、Sub/Approveへの通常チャットモデル指定、poll中の下書きとfocus、再読込、別画面とのrevision競合を確認する。
- プロジェクトを二つのページで同時編集し、revision競合と更新中の入力・focus・展開状態の維持を確認する。退役したグループ・方向付きルール・参加コードのフォームが表示されないことも確認する。
- ネットワークの診断、証明書更新、Gatewayポート設定の入力検証・固定値保存・ネットワーク再開と画面再読込後の実適用、および760px幅の横スクロールの有無を確認する。
- 実mTLSの参加要求をHubで許可し、PC名・プロジェクトの用途・停止／再許可・恒久失効・削除をGUIで操作する。別のPCが変更されないことを独立した読取で確認する。
- 端末から届くMCP履歴の失敗・一覧・ページ送り・詳細・Markdown保存を確認する。長い読み取り本文のTab／Shift+Tab、PageDown／Ctrl+End／Ctrl+Homeと通常poll後のfocus・scroll保持、選択変更後の古い応答、停止中の端末への履歴要求、削除した端末の取得済み履歴も確認する。
- Tab／Enterでモデルへのページ内リンクと編集ボタンを操作し、dialogのTab／Shift+Tab、Escape取消とfocus復帰、必須値・容量の入力検証、Spaceによるcheckbox変更と保存を確認する。native dialogの順序末尾でブラウザー側へ一度移る動作を許し、背景のページ要素へのfocus移動や、次のTabでも戻らない状態は拒否する。
- 実ブラウザーのclipboardで共通configとWeb URLをコピーし、configはWindowsの改行変換だけを除いてGUIダウンロードと一致することを確認する。コピー後に試験自身が書いた公開情報だけを読み取る。利用者の既存clipboardは読まないが、この試験のコピー操作で内容は置き換わる。
- 試験用HTTP providerが使用中のポートにネットワークを開始してエラー表示を確認し、GUIから別のポートへ直して開始・停止できることを確認する。
- 終了時に、この試験が起動したHubを停止し、取得済みのWeb・端末ネットワーク・Gateway・内部管理ポートが閉じたことを確認する。

`web-admin.spec.mjs`は起動とネットワークの基本操作、`management-workflows.spec.mjs`はモデル・診断、`shared-management.spec.mjs`はプロジェクト管理と更新競合、`device-management.spec.mjs`はPC管理と履歴、`keyboard-recovery.spec.mjs`はキーボード・clipboard・ポート競合からの回復を担当します。画面の操作はPlaywright、起動先の照合・独立した読取・終了処理は共有`hub_server.mjs`のprivate host-control APIを使います。既存のHub保存先や、利用者が開いているブラウザーには接続しません。

`control-audit.spec.mjs`はcontrol単位の不足を補います。全ページリンクと説明の展開／取消、Gateway checkbox両値、手動入力の検証、4 profileの登録、論理モデル／容量プールを新規へ戻す選択、PC名dialogの取消／再読込、プロジェクトのPC用途、削除済みフォームの非表示、ネットワークの不正入力、外部Web URLの表示・コピーを対象にします。シナリオの追加と実GUIの合格は別で、実行結果はtask-localレポートで確認してください。control一覧添付はその時点の表示・disabled・optionの読取であり、全操作の合格判定ではありません。

同ファイルでは、PCの照合IDと恒久失効のdetailsを閉じて通常poll後に開き直す操作も確認します。未保存の失効checkboxは保持だけを検査し、dialog取消後に未失効を照合します。

`authentication.spec.mjs`は、未許可画面、起動ticketからCookieへの交換、利用終了、使用済みticketの拒否、新ticketでの再開、遅延応答の遮断、別タブでのCookie置換と旧session要求の拒否を実サーバーで確認します。パスワード入力欄が存在しないことも検査します。ticketとCookieは試験専用です。

外部Web URLのケースは、共有fixtureの`remoteWeb: true`で公開CLIの`--web-settings`を指定します。専用のloopback HTTP listenerを併設するだけで、LANへの公開・FW変更・HTTPS例外設定は行いません。local管理画面から外部URLを表示・コピーし、remote HTTPページではパスワード欄がなく管理を許可されたPCからの接続を案内することを確認します。既定のケースとDesktop結合consumerは従来どおりlocal listenerのみです。終了時は追加listenerも同じownerが停止を確認します。traceには試験用認証情報を含み得るため、共有前に確認してください。

`provider-fixture.mjs`はloopbackでモデル一覧だけを返すHTTPホストです。`device-fixture.mjs`は試験専用のP-256鍵・CSRを生成し、Hubの参加／mTLS／生存確認／公開申告／履歴応答プロトコルを使います。Hub管理画面では生成できない外部参加要求を準備するためのもので、Desktopアプリやエージェント実行を代替して受入済みとするものではありません。管理の変更はブラウザーから行います。

GUI操作・DOM assertionの成功と、保存した画像を人が確認した結果は別に記録します。この試験の成功は、全GUI機能、moyAI Desktopのネイティブ操作、物理別端末、実LLM生成、外部HTTPS/proxy、長時間試験の受入を意味しません。

`firewall.spec.mjs`の専用ケースだけは、Hubが読み取った現在のLAN候補から社内IPv4を明示選択し、隔離Hubの端末ネットワークをそのIPへ一時的に公開します。Web管理はloopbackのままです。起動ticketから受信許可欄への遷移、自動preview、未確認時の配布抑止、管理者用PS1の保存、組織管理の明示的な配布判断と対象変更時の解除までを操作します。apply／remove／UACは実行しません。対象IPの候補がなければこの環境受入はskipとして残り、全regression合格にはなりません。fixtureは事前に許可したIPのHub／Gatewayだけを追跡します。WindowsではLANへのTCP接続がhost policyでtimeoutになる場合があるため、正常終了とdescriptor削除に加え、OSの待受一覧でそのIP・ポートおよびwildcardの待受がないことを確認します。読取失敗やtimeoutを閉鎖済みへ読み替えません。通常のloopbackケースは従来どおりTCP接続拒否で確認します。

## 証跡と後始末

`verify:gui`／`test:gui`の既定保存先は、リポジトリの隣の`project_sandbox/hub-gui/<実行日時とID>/`です。`--artifact-parent <絶対パス>`で親フォルダーを指定すると、その下に新しい実行フォルダーを作ります。`--artifacts <新しい絶対パス>`は実行フォルダーそのものを指定し、既存フォルダーの上書きを拒否します。

- `stage-summary.json`: unit／frontend-build／rust-build／browser-e2eの結果と所要時間。単体件数とGUI件数は別欄。目視は常に`not_performed_by_runner`です。
- `public-summary.json`: 自動共有用の限定要約。段階、suite、件数、SHA-256、分類コード、生成されたcase IDと所要時間だけを出力します。環境変数、端末情報、DOM、失敗全文、原添付データは含みません。
- `logs/`: 各コマンドのローカルログ。`browser/report/`はHTML、`browser/playwright-results.json`は原JSON、`browser/results/`は画像・添付データ・隔離したHub保存先です。

**CIの自動uploadは`public-summary.json`だけに限定してください。** 原JSON、HTML、trace、ログや`hub-data/`には試験用の秘密情報が含まれ得ます。段階入口は子コマンドの原出力もローカルログへ保存し、consoleには段階・件数・分類だけを表示します。必要な詳細を人が確認して共有することと、フォルダー一式の自動共有を区別します。

低水準の`test:browser`では従来どおり`project_sandbox/hub-playwright/<実行日時とID>/`に保存します。`MOYAI_HUB_BROWSER_ARTIFACTS`で変更する場合は、新しい実行用フォルダーの絶対パスを指定してください。段階入口は、この値を今回の実行フォルダー配下へ設定し直します。どちらも失敗時のtraceと画像を保持します。

各試験は新しいHubを起動し、正規終了時にprivate管理トークンを無効にします。保存先には試験専用のCA秘密鍵も含まれるため、`hub-data/`やprivate接続情報をそのまま共有しないでください。画像・レポート・traceも内容を確認してから共有します。後始末に失敗した場合は試験も失敗し、`hub-lifecycle`添付に終了状態と取得済みポートの状態を残します。
