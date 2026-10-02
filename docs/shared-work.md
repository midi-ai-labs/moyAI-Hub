# 共有仕事APIと運用境界

Hubプロジェクトの会話・仕事・入力・成果とPC参加を永続管理するAPIです。利用者の操作は[チーム準備](team-preparation.md)、Desktopの動作は[複数PC設計](../../moyAI/docs/design/multi-device-session.md)を参照してください。旧端末間MCPの保存契約とは別で、旧仕事を自動移行しません。

## 保存と認証

保存先の `shared-work.sqlite3` と `shared-work.assets/` が共有データを所有します。端末台帳・CA・モデルcatalogは別に保持します。schema変更はforward migrationです。実装は [shared_work/](../src/shared_work/) と [migrations/](../migrations/) を正とします。

登録PCと監査用の利用者IDを結び、新PCには非管理者の専用利用者を自動作成します。Windowsユーザー名やPC名は認証条件ではありません。通常の導入にID/PWを使いません。旧 `login` / `setup-password` / `refresh` は公開HTTPから使用不可です。

操作APIはmTLSで登録PCを確認し、`POST device-session` で得たtokenをBearerとして使います。短期sessionは別の端末証明書へ持ち替えられません。Runnerは自端末の証明書で割当と報告を扱います。ブラウザー管理資格は別で、詳細は[Web管理](web-management.md)を参照してください。

プロジェクトの役割は `reader` / `contributor` / `manager` です。明示的な操作PC指定と有効な利用者の対応を要求し、既存の個別・グループ役割がなければContributorを導出します。既存Readerを自動昇格させません。要求の待機後・副作用の確定前にも現在の資格を確認します。

## 相互TLS内のAPI

端末ネットワークの `/v1/shared/` 以下です。端末証明書のない要求とブラウザーOrigin付き要求を拒否します。厳密な引数・応答は [api.rs](../src/shared_work/api.rs)、[api/](../src/shared_work/api/) と近傍testsを参照してください。

| Method / path (`/v1/shared/`以下) | 内容 |
| --- | --- |
| POST `device-session` / `logout` | 登録PCに結び付く利用者の短期共有sessionの作成・失効 |
| POST `admin/web-access` | 管理権限のある登録PCから、HTTPS管理画面の一度限りの起動ticketを取得 |
| GET `device-projects` | 端末証明書で自PCの操作用・実行用の指定、参加世代と実行準備状況を取得。人や仕事の本文は含まない |
| GET `session` | 現在の本人と共有用セッションの有効期限 |
| GET `projects` | 本人が所属し、このPCから操作することを許可されたプロジェクト。自PCの `participation_generation` を含む |
| POST `projects/{id}/leave` | 自PCをプロジェクトから離脱。`expected_participation_generation` で古い操作を拒否し、実行中の仕事は停止確認まで保持 |
| GET `projects/{id}/conversations` | 送信済みの共有会話一覧。名称、最新仕事、編集可否、削除時の停止待ち、revisionを返す |
| POST `conversations/{id}/rename` / `conversations/{id}/delete` | プロジェクトIDと会話revisionを照合して名称変更・論理削除。削除は仕事と保持サービスを停止要求し、清算後に通常一覧から隠す |
| GET `environments?project_id=...` | 閲覧可能な公開環境と共通枠の占有数 |
| POST `jobs` | 共有根仕事の永続受付。操作PCの参加世代、任意の編集対象仕事とそのrevisionを照合 |
| GET `jobs?project_id=...&before=...&limit=50` | プロジェクト内の仕事。最大100件 |
| GET `jobs/{id}` | 入力・checkpoint・結果を含む仕事の詳細 |
| GET `origins/{origin_session_ref}/work?job_before=...&service_before=...` | 同じ操作PCの通常会話から依頼した、本人の仕事と保持サービスを現在の参加プロジェクト全体から取得。仕事50件・サービス128件の独立ページ |
| POST `origins/{origin}/turns/{turn}/admit` | 通常会話の保存済みturn revisionを登録し、後続の委任に使うepochを取得。既存turnのみ照会する `create:false` も可 |
| POST `origins/{origin}/turns/{turn}/stop` / POST `origins/{origin}/stop` | このturn、または指定revisionまでの通常会話の遠隔実行を停止要求。後続turnは新epochで再利用可 |
| GET `conversations/{id}/history?project_id=...&snapshot=...&before=...&limit=20` | 一つのHub仕事会話の根仕事・追加指示を保存順で取得。会話本文は各仕事のtranscriptを参照 |
| POST `conversations/{id}/stop` | 指定Hub仕事会話の待機・実行仕事と保持サービスをまとめて停止要求し、会話epochを進めて古い追加指示を拒否する |
| GET `status?project_id=...&before=...&limit=50&environment_before=...` | 利用者・待機理由・共有資源の占有を所属に応じて投影。仕事は最大100件、環境は20件の独立したページ |
| POST `jobs/{id}/cancel` | 保存された依頼元PCと本人による根仕事全体の取消 |
| GET / POST `jobs/{id}/handover` | 退役済み。認証済み旧clientへ `410 Gone` と廃止理由を返し、担当を変更しない |
| POST `admin/jobs/{id}/cancel` / `admin/services/{id}/stop` | Project Managerによる明示的な管理取消・保持サービス停止。通常会話の操作権を移さない |
| POST `jobs/{id}/continue` | 会話で最新の根仕事が終端し、同じ会話に未清算の仕事・試行がない場合だけ、その仕事から次の根仕事を受付。過去の根仕事からの並行継続は拒否。停止後は最新detailの `conversation_epoch` を要求 |
| GET `inbox?before=...&limit=50` / POST `inbox/{id}/read` | 本人の承認依頼・仕事終端通知と既読 |
| GET `jobs/{id}/approval` | 最新の具体的な操作承認と現在の判断権限。`include_descendants=true` で子仕事の承認と正確な対象contextも返す |
| POST `jobs/{id}/approvals/{approval_id}/decision` | 依頼元PCによる `approve` / `deny` / `stop`。子承認はcontextのjob IDを指定 |
| POST `runner/claim` | 対象端末に割り当てる永続試行。人のtokenではなく端末証明書で検証 |
| GET `runner/assignments` | 対象端末の未確認試行・停止要求。新たな起動許可ではない |
| GET `runner/attempts/{id}` | 自端末の正確な試行、現在状態、不確定理由の照合 |
| POST `runner/report` | 対象試行の開始確認・公開済み途中経過・操作承認要求・安全地点での子待ち・不確定観測・停止確認付き結果 |
| POST `runner/approvals/{id}/consume` | 正確な試行ID・世代に結び付く人の回答。現在の権限と有効期限を再確認 |
| POST `runner/attempts/{id}/authorize` | 個別副作用の直前に、実行世代・現在の権限・必要な承認を再確認 |
| POST `runner/local-admissions` | 登録資源を使う既存ローカル入口の本人確認と即時枠確保 |
| POST `runner/templates` / GET `runner/environments` | Runnerが許可した配置テンプレートと自端末の環境・資源の照合 |
| GET / POST `runner/provisioning` | 配置要求の配送と、同一世代の成功・失敗受領 |
| POST `projects/{id}/assets` | 入力ファイルの永続保存。最大8 MiB |
| GET `jobs/{id}/assets` / GET `assets/{id}` | 現在のプロジェクト所属で検証する成果・入力一覧とダウンロード |
| GET `jobs/{id}/transcript?after=0&limit=100` | 保存済み会話のページと、実行中の現在工程の途中経過 |
| POST `runner/attempts/{id}/assets` | 正確な試行に対する成果・会話アーカイブ・子仕事へ渡す入力の保存。`kind=input` は稼働中の親試行と現在の実行権限を要し、`base_sha256=null`、最大8 MiB。返るasset IDを子仕事の `input_refs` に使う |
| GET `runner/attempts/{id}/archive` / GET `runner/attempts/{attempt}/assets/{id}` | 当該仕事に結び付く再開会話・入力だけの取得 |
| GET `runner/attempts/{attempt}/children/{child}/artifacts?generation=...` / GET `.../artifacts/{asset}?generation=...` | 現在の親試行・世代と直下の終端子仕事を照合し、その子の成果だけを一覧・取得。前者は `{items:[Asset...]}`、後者は `AssetDownload` |
| GET `admin/snapshot` / POST `admin/command` | プロジェクト・人・PC用途・配置・保守の共通管理owner（旧グループ等の互換APIは保持） |
| GET `admin/maintenance?before=...&limit=128` | 未終端試行と不確定理由の管理用ページ。仕事本文は含まない |
| GET `admin/users` | 管理者による保存済み利用者の一覧 |
| POST `admin/projects` / `admin/memberships` / `admin/environments` | プロジェクト、所属役割、固定端末環境の登録 |

保持アプリには、`GET services`、`POST services/{id}/stop`、`POST conversations/{id}/services/stop`、Runnerの `GET runner/services` / `GET runner/conversation-services` / `POST runner/services/{id}/stop` / `POST runner/services/{id}/conversation-stop` を使います。対象仕事・世代・プロジェクト・会話と現在の権限を照合します。[services.rs](../src/shared_work/services.rs)が保存と停止確認を所有します。

## 受付・実行・停止

- `request_id` と保存内容で再送を照合します。応答が不明な場合も、別IDで起動し直しません。
- 根仕事の受付時に利用可能な実行環境の上限を固定し、その後の現在の権限と交差して使います。設定変更で元の上限を広げません。
- 明示しない初期実行先は、許可済みの操作PCを優先して配置します。その後の工程・PC選択はエージェントのツール呼出しによります。明示した宛先を別PCへ置き換えません。
- Runnerは開始意図をjournalへ保存し、Hubの開始受領と現在の試行を確認してから実行します。子仕事へ移るときはcheckpoint保存・子作成・親の待機を確定します。
- `descendant_budget` は根仕事配下に同時に保持できる未完了の子仕事数です。0は委任不可です。子待ち中の中間仕事と取消清算中の仕事も数え、成功・失敗・取消済みの子は履歴を残したまま枠を返します。通常Desktopの値8は累積実行回数の制限ではありません。
- Hub全体の未終端根仕事1,000件、子枠合計32,000件、保存仕事100,000件の上限は維持します。根仕事の受付では、現在の履歴と未使用の子枠を合わせて上限を確認します。子作成時にも保存件数を確認します。この受付判定は、反復実行で増える将来の全履歴を保存できる保証ではありません。
- 取消・停止の受付は実プロセス停止の証明ではありません。通信断、再起動、期限切れだけで同じ枠を別の仕事へ割り当てません。不確定な試行は停止確認まで保持します。
- 失効したPCにも自端末の既存試行の停止照合・報告を限定して認めますが、新規割当・開始・委任は拒否します。

保存済みの `descendant_budget` 値とschemaは変更せず、既存仕事にも未完了数の意味を適用します。失敗済み仕事は自動再開しません。累積件数で判定する旧Hubへ戻すと、完了履歴が保存値を超えた仕事の追加委任は再び拒否されます。

源泉は [store.rs](../src/shared_work/store.rs)、[runner_contact.rs](../src/shared_work/runner_contact.rs)、[stop_scope.rs](../src/shared_work/stop_scope.rs)、Runnerの [journal](../../moyAI/src/runner/shared/journal.rs) です。

## 実行中の途中経過

新Hubの割当は `supports_progress_reports: true` を返します。省略する旧Hubへ新Runnerは途中経過を送らず、旧Runnerも既存の開始・完了報告を継続できます。`Progress` は現在の実行試行と権限を確認した表示専用の報告です。仕事状態、実行枠、期限、接触確認を変更せず、送信失敗を仕事失敗へ変えません。

HubはV23の `shared_attempt_progress` に試行ごとの最新投影1件だけを保持します。公開済みcanonical historyの順序をrevisionとし、同revision・同内容の再送は観測時刻を進めず、古いrevisionや同revisionの差替えを拒否します。最大12件、1件16KiB、報告全体64KiBとし、省略は `truncated` で示します。内部指示・診断・思考は対象外です。

`transcript` の既存 `items` / `next_after` は保存済みarchiveのままです。追加の任意 `activity` は、選択した仕事が現在待つ子を辿った実行工程だけを返します。停止・終端・資格失効・古い試行では表示せず、同試行のarchiveが保存された後はarchiveを優先します。この投影をcheckpointや再実行の根拠にはしません。
## 依頼元の操作・承認・継続

通常の承認・追加指示・最新発言の編集・停止は、保存された依頼元PCと本人に固定します。子仕事・再委任・親の再開でも依頼元を維持します。同じ利用者の別PC、実行PC、Project Managerからの代理操作は認めません。承認は具体的な対象・操作・試行世代・依頼元の参加世代に結び付き、回答時・消費時・副作用直前に現在の権限を再確認します。依頼元が不明な旧仕事を、PC名や現在の担当者から補完しません。実行PCの緊急停止と、管理権限による明示的な取消・停止清算は別の経路として保持します。

`deny` / `stop` は回答保存と同じtransactionで現在の根仕事と子仕事を取り消します。子の終了後に親AIを自動再開せず、停止確認まで資源を保持します。同じ否定回答の再送・配送は元の操作と認可世代へ照合し、後から明示的に始めた次の仕事へ影響させません。

担当交代のUI・command・投影・承認例外は廃止しました。V25はpending履歴を取消済みへ退役し、交代待ちで仕事を止めません。確定済みの交代履歴がある会話からの通常継続は拒否します。移行前には対象scopeの非終端job、未停止attempt、active/uncertain保持サービスを照合し、残る場合は対象rootを表示して更新を拒否します。既存版で自然終了または明示的な取消と実停止確認を済ませてから更新します。会話・成果・監査・IDとreleased migrationは保持します。

追加指示は最新の終端仕事を参照する新しい根仕事です。過去の根からの並行継続は拒否します。会話停止後は新しいepochが必要で、遅れて到着した停止前の依頼を受け付けません。同じ停止要求の再送で後続turnを停止しません。

操作権限は [origin_control.rs](../src/shared_work/origin_control.rs)、通常継続と停止清算は [handover.rs](../src/shared_work/handover.rs)、[conversation_catalog.rs](../src/shared_work/conversation_catalog.rs)、[stop_scope.rs](../src/shared_work/stop_scope.rs) と近傍testsを参照してください。

## 会話・参加・フォルダー

会話には名称・revision・最新仕事と停止待ちを保持します。名前変更・削除は現在revisionを照合します。実行や保持サービスの停止を伴う削除には依頼元の操作権が必要です。清算済みの履歴削除と名前変更は既存Contributor権限を維持します。削除時は仕事と保持アプリの停止を要求してから通常一覧から隠します。履歴はsnapshot付きページで取得し、ページごとに閲覧権限を再確認します。

最新発言の編集は `POST jobs` の `conversation_id` / `revises_job_id` / `expected_revised_revision` で受け付けます。対象仕事の終端と同会話の停止確認を必要とし、元発言は監査用に保持します。実ファイルは巻き戻しません。添付付きの発言も文字を編集でき、元発言の不変な `input_refs` を順序も含めてそのまま引き継ぎます（省略と空配列は同等）。受付時に参照の一致、データの存続、現在のプロジェクト・実行権限を再確認し、新しい仕事へ既存の入力参照を結合します。添付の欠落・追加・差替え、保持期限による削除、権限喪失は再送を拒否し、元ファイルの再読込や無音での添付省略は行いません。

PCの離脱・Kick・プロジェクト削除は新規利用を止め、実行停止を確認します。完全離脱後の再参加は新しい参加世代で、古い送信レシートを再利用しません。実行PCには新しい環境とフォルダー登録が必要です。ローカルファイルは削除せず、再参加専用の会話復元・固定探索を行いません。

通常の管理画面はプロジェクト名と操作・実行PCを保存し、フォルダーは各Desktopで登録します。Hubに端末の実パスを入力しません。準備ACK後だけ割り当て、フォルダーがなくなれば再登録を求めます。同じPCの複数プロジェクトは既定で容量1のPC資源を共有します。旧テンプレート配置・グループ・環境・資源APIは互換用途として保持します。

実行場所の登録上限（PCごと128件、Hub全体1000件）は現在の設定に適用します。準備待ち・受付停止・保管中のプロジェクトも、実行PCの紐付けや利用許可を保持する間は枠を使います。Desktopで準備した場所は、離脱・実行用途解除後に無効で許可・紐付けもなくなると履歴として残し、上限と通常の設定一覧から除きます。過去仕事の参照や成果フォルダーは削除せず、再参加には新しい実行場所とフォルダー同意が必要です。旧APIで手動登録した設定は停止・無所属でも予約として残し、自動で退役させません。

`runner/environments?current_only=true` は現在の設定の完全な一覧で、部分ページを返しません。新Runnerだけがこの形式を要求します。無指定またはfalseでは従来どおり退役行も返し、旧Runnerが旧許可を清算できる契約を維持します。いずれも128件を超えれば省略せず容量エラーにします。`workspace_bound` は現在のプロジェクト割当を示し、保管中の紐付けと退役済みの場所を区別します。新Runnerは仕事と保持アプリの停止確認後に、一覧から消えた自動フォルダー許可だけを外し、旧Hubが返す明示的な退役行にも対応します。

旧Desktopが自動作成したフォルダーも、利用できなくなった場合はDesktopで選び直します。Hubは当該環境を準備未完了にし、現在の参加権限と準備世代が一致する再登録だけを受け付けます。古い成功通知で再び実行可能にはしません。

PC候補は現在の権限で絞り、管理者が登録した機能名をAIへ渡します。機能名はソフトやライセンスの稼働検査ではありません。[project_setup.rs](../src/shared_work/project_setup.rs)が参加と準備を所有します。

## 入力・成果・継続アプリ

入力と成果はHubの専用領域へ固定コピーし、長さ・所有仕事・内容ハッシュをDBへ記録します。通常ファイル8 MiB、会話archive64 MiB、論理合計2 GiBの上限があります。読取りでも内容と対象を確認し、別の仕事の未関連ファイルをRunnerへ渡しません。

`shared_publish_artifact` は環境内の相対パスを固定コピーとして公開します。会話付属の出力は一会話128件・合計32 MiB、各8 MiB以内です。フォルダー全体の同期ではありません。詳細は [data.rs](../src/shared_work/data.rs)、[blob_store.rs](../src/shared_work/blob_store.rs) を参照してください。

継続アプリは元の試行・世代・会話・資源へ結び付くサービスです。`expires_at_ms: null` は暗黙の総時間制限がないことを示します。保持中の資源を他会話へ渡さず、同じ会話が同じ実行先へ戻る場合に参照します。待機理由も予約と同じ容量判定を使い、所有する保持枠を再利用できる場合に他会話の枠待ちと表示しません。機能要求だけを根拠にRunnerの更新が必要と断定しません。取消・失敗・失効、明示した互換期限の超過は停止要求になり、Runnerの停止報告後に枠を解放します。RunnerはHub制御喪失でも実アプリを停止しますが、制御を確認できた周期の無関係なローカルエラーを制御喪失として扱いません。時間の上限は設けず、件数上限は試行16件、Runner128件、会話128件です。

新規の仕事は `multi_device_session_v2` を申告するRunnerだけへ配送します。新Runnerは旧v1割当も受理します。通常会話の照会は起点PC・依頼者・現在の所属を照合します。権限失効で詳細を隠す場合も、未停止の仕事があれば `hidden_active_work` を返し、空一覧を停止済みと扱わせません。旧記録の起点を推測して新しい会話へ混ぜません。

## 保守と復元

内容の保持期限による削除は自動実行しません。管理画面のプレビュー後、現在revisionと条件を再確認します。未終端仕事の入力や続行に必要な内容を保護し、他の保持参照がある物理ファイルも残します。

Hubを停止し、未使用の保存先へバックアップ・復元します。バックアップにはCAや利用者情報も含むため、管理者が管理する場所を指定してください。

```powershell
moyai-hub --data-dir C:\moyAI\Hub --backup-dir C:\moyAI\Backups\Hub-backup
moyai-hub --restore-from C:\moyAI\Backups\Hub-backup --data-dir C:\moyAI\Hub-restored --acknowledge-runners-stopped
```

復元前に関連Runnerと管理プロセスを止めます。復元後は利用者sessionと旧承認を失効し、端末・環境を停止します。保存後に実行された可能性がある旧仕事を自動再実行せず、停止確認が必要な試行の枠を保持します。再実行は新しい明示依頼で行います。失敗した復元先の起動禁止markerは削除せず、原因を直して新しい保存先へ復元してください。

[backup.rs](../src/backup.rs)とtestsが復元契約を所有します。Windowsの保存・同一PCでの統合試験は、物理複数Windows・別OSアカウント・LAN/FW・長時間運転の受入と区別します。

通常投入の `start_before_ms` は null で初期待機列に時間失効はありません。明示された旧API期限は最初のStarted受理で消費し、子・親再開へ継承しません。承認の期限は総作業期限とは別で、期限切れは待機です。`POST jobs/{job_id}/approvals/{approval_id}/reconfirm` は依頼元PCのみが呼べ、未消費の期限切れ操作を新IDで再確認する要求です。Runnerは既存の consume の `reconfirmation_required` を受け、同じbroker待機を新しい承認へ置き換えます。旧回答は実行権限になりません。

V26では全非終端仕事がv2 Runnerを要求します。明示された初回開始期限の値は維持します。新しい継続・子仕事もv2を要求し、終端済みの旧仕事から旧Runner契約を継承しません。旧Runnerの割当poll/claimには配送せず、稼働中の旧Runnerは既存の試行不在の停止経路へ進みます。正確な旧試行の停止・不明報告は受理し、停止確認まで占有を残します。新Runnerには同じ試行を返し、自動再実行しません。旧有限期限の終端履歴と保持receiptは期限を維持します。更新時は実行PCのmoyAIも更新してください。
