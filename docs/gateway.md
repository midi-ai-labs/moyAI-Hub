# Gateway の実行境界

Hub のサーバープロセスは catalog、端末、選択、revision、turn、lease、request ID と中継接続数のメタデータを扱います。管理画面は通常のWebブラウザーで開き、Rustサーバーの管理APIへ接続します。prompt / response は Desktop からさらに別 process の Gateway に送られ、Gateway が provider に転送します。サーバーが同じ `moyai-hub` 実行ファイルを `--gateway` で起動し、Gateway の制御用秘密情報はサーバーとの stdin パイプだけで初期化します。モデルの load / unload、推論のキューとスケジューリング、生成の開始・終了は oMLX / LM Studio などの provider が所有します。Hub は provider 内の実行状態を管理・推定しません。

Gateway の管理用待受は一時的な IPv4 loopback port です。従来の同一 PC 接続はこの HTTP 経路を使います。Hub の「端末ネットワーク」を開始した場合は、同じ子プロセスに別ポートの **相互 TLS が必須の生成用 listener** を開きます。Hub のメタデータ listener と生成本文を処理する listener は別で、生成用 listener に管理 API は公開しません。生成用ポートは「PCの接続」で指定し、新規保存先では9472が標準です。既存の0は自動選択として維持します。固定ポートが使用中なら開始を拒否し、自動ポートへ切り替えません。変更は次回開始から適用します。組織の Firewall では両ポートを確認してください。WindowsのHub PCでは、[限定した受信許可](firewall.md)をローカル画面で確認し、UACを経て明示設定できます。

## 管理画面とサーバーの寿命

通常起動する`moyai-hub`が保存先の排他lock、catalog / DeviceNetwork、端末認証、Web / metadata / TLS listenerとGatewayを所有します。公開CLIは保存済み設定でWeb管理を自動開始し、integration testも同じ起動経路を使います。ブラウザーを閉じたり強制終了したりしてもサーバーとGatewayは停止しません。起動・保存先・公開設定は[Web管理の手順](web-management.md)を参照してください。

ブラウザー管理commandは固定Host/Origin・CSRF・管理者本人の用途別セッションを照合したWeb APIから同じ`Admin`へ渡します。Hub PCの起動操作、または管理を許可された登録PCのDesktopから、一度だけ使えるアクセス情報で管理画面を開きます。ID・パスワード入力は不要です。別PCからの管理にはHTTPSを使い、PCの現在の許可と利用者との対応を再確認します。Hubの待受やモデル接続先の変更はHub PCの管理画面に限定します。CLIの状態確認・終了は別のprivate loopback制御へBearer認証で接続します。この資格情報は利用者限定ファイルで保護し、Web・ログ・Desktop配布用の共通設定へ載せません。端末のmTLSと実行許可はこのWeb構成変更と独立しています。catalogと端末設定の読み書きはサーバーの`Admin`が一元所有します。

ブラウザーの「接続の受付を停止」は端末向けlistenerとGatewayを停止し、サーバープロセスとWeb管理を維持します。サーバー自体を終了する操作は`--stop [--data-dir <保存先>]`、またはforegroundサーバーのCtrl+Cです。Windowsのログオン時には、Hub PCのローカル管理画面から任意登録した `--launch --no-browser` が保存済みのHub・Gateway設定を再開します。[チームの初期設定](team-preparation.md)を参照してください。Windowsサービス、ログオン前・サインアウト後の稼働や外部AIの自動起動は対象外です。

## 認証と実要求

登録後の model client 専用 Bearer で `POST /v1/requests/prepare` に次を送ります。未知フィールドを拒否します。従来の protocol 1 接続では loopback HTTP を使います。端末ネットワーク接続では、永続端末の証明書で HTTPS `/v1/network/model-session` に接続して専用 client を得ます。その後のモデル API も、その端末の現在の証明書と client token の両方を検証します。受入側の遠隔 job はそれぞれ独立した model client を持ち、同じ Main active turn を共有しません。

```json
{"id":"client-id","context":"main","turn_id":"stable-user-turn","request_id":"request-attempt","expected_hub_id":"hub-id","reviewed_revision":"3"}
```

`context` は `main` / `side_chat` / `approve`、revision は正の十進文字列です。待機時は 200 `{state:"waiting",reason,retry_after_ms:1000}`。reason は `busy` / `global_capacity` / `target_unavailable` / `maintenance` です。旧版の `jit_pending` は型の互換性を保ちますが、現行 Hub は返しません。

`purpose` の省略は通常の user turn です。`preparation` は依頼整形、`delegated` は [子エージェントの独立実行](delegated-execution.md) に使い、どちらも同じ permit / 中継枠の認可を通し、user-turn affinity 回数を消費しません。Delegated だけは確認済み選択の snapshot と独立した context を使い、finish/cancel と heartbeat に対象を区別する `delegated:true` を渡します。

割当は登録した provider の新しいモデル一覧で存在を確認し、管理者の maintenance と選択・能力・中継接続数の条件を適用します。LM Studio の未ロード報告やロード情報の欠如を理由に待機しません。一覧にあるモデルへ要求を送り、ロードや実行可否は provider が判断します。接続不能、確認期限切れ、成功した一覧からのモデル欠落は区別します。管理画面の `model_present` は同じ観測から導出し、ロード済みを意味しません。

登録/model-session が `supports_turn_heartbeat:true` を返す場合、Desktop は既存の `/v1/clients/heartbeat` に `active_turns:[{context,turn_id}]` を含めます。Hub は認証された client の同じ active turn の期限内 lease だけを `lease_ttl_ms` 分延長します。tool 実行や遠隔待機中も同じ割当を維持できますが、permit の期限、request ID、affinity の残り回数は変えません。省略は旧クライアントと同じ presence 更新のみです。失効・終了・取消済み turn、別 context/turn、旧 revision の lease は更新しません。これは provider 実行の継続や終了を証明する heartbeat ではありません。

準備成功は 200 `{state:"ready",lease_id,permit_id,logical_model_id,gateway_base_url,request_token,expires_at_ms,provider_profile,provider_api_mode}`。期限は十進文字列です。従来の target は `http://127.0.0.1:<port>/r/<permit_id>/v1`、端末ネットワークでは `https://<Hub-IP>:<Gateway-port>/r/<permit_id>/v1` です。Desktop は下表の API path を付加し、専用 request token を Bearer に指定します。body.model は logical model ID を使用し、Gateway が登録済み provider model ID に変換します。実 endpoint・provider credential は Desktop に配布しません。AIホストのAPIキーはHub側だけで解決します。

| 登録する profile | モデル一覧 | 生成 API | Desktop への profile / API mode |
| --- | --- | --- | --- |
| `openai_compatible_chat` | `/v1/models` | `/v1/chat/completions` | `openai_compatible_chat` / `chat_completions` |
| `lmstudio_chat` | `/api/v1/models` | `/v1/chat/completions` | 同上 |
| `openai_compatible_responses` | `/v1/models` | `/v1/responses` | `openai_compatible_responses` / `responses` |
| `lmstudio_responses` | `/api/v1/models` | `/v1/responses` | 同上 |

管理画面の接続方式で metadata と生成 API の組を明示します。モデル一覧の成功だけでは Responses 対応を証明しません。provider が公開する API を選び、非対応時は要求を失敗として返します。Chat Completions への自動切替は行いません。既存 profile の意味は維持し、新しい Responses profile の利用には対応版の Hub と Desktop が必要です。

Responses は Desktop の既存 HTTP adapter と同じ全履歴の `input` 配列、`instructions`、`store:false`、`stream:true` を使います。`previous_response_id`、`conversation`、`background` を受け付けず、保存済み provider 会話や非同期 job を permit の外へ持ち越しません。Gateway は許可の profile と API path、logical model を照合した後、model 名だけを置換して本文・tool call・SSEをそのまま中継します。Chat形式への変換やprovider stateの保持はありません。Desktop は prepare の profile/API の正しい組から request ごとの immutable adapter を選びます。

ネットワークの permit は発行先端末の証明書に結び付きます。別端末が token を取得しても使用できず、HTTP 経路への持ち替えも拒否します。Gateway は実行の claim 時に Hub へ照会し、証明書が現在も有効か確認します。Desktop は共通設定の公開 CA をアプリ内で信頼し、Hub / Gateway の証明書と接続先 IP を検証します。端末の TLS 秘密鍵や共通の長期 token を配布する方式ではありません。

Hub / Gateway の server 証明書だけに予約名 `hub.moyai.invalid` を含めます。managed client は実接続先の IP とこの予約名を同じ証明書で検証し、同じ CA・同じ IP の端末証明書を Hub と取り違えません。端末 CSR で予約名を要求すると拒否します。予約名は実 IP の検証を置き換えるものではありません。

一つの request ID は未使用の予約である間だけ prepare を再取得できます。一度 claim した token は再利用できません。実要求を再試行する場合は新しい request ID を使い、同じ user turn は同じ turn ID を保持します。claim 時にも最新 revision、端末の生存、context、active turn、期限を Hub に照合します。claim 後の revision 変更は進行中の本文転送を切りません。

モデル割当と作業実行は別の資格です。現行のHubプロジェクトは[共有仕事API](shared-work.md)が会話・入力・成果と実行権限を保存します。以下は互換性のため残る旧Desktop間MCPの境界です。旧作業委任では Hub の公開先一覧と方向付き許可から対象限定の短期資格を発行し、受入側 Desktop が毎回照会します。その仕事が必要とするモデル生成は上記 model session と Gateway を使います。この旧経路ではHubは仕事本文を受け取らず、受入側の永続job台帳が重複実行を防ぎます。

`POST /v1/turns/finish` と `/v1/turns/cancel` は `{id,context,turn_id}` を受け取り、`{id,context,turn_id,closed:true}` を返します。切断も含めて未使用の予約を失効させ、対象の開始済み要求は Gateway 自身の上流 HTTP 接続を閉じます。HTTP 処理の後始末後に中継枠を解放します。provider 内の生成終了を証明する操作ではありません。

HTTP エラーは本文や URL を含まない `{error,current_revision?}` です。401 `unauthorized`、409 `review_required` / `different_hub` / `active_turn` / `turn_closed` / `permit_expired` / `lease_expired` / `selection_required` / `stale_client`、400 `invalid_request` / `model_removed` / `capability_mismatch`、503 `gateway_unavailable`、429 `registry_full` を返します。Gateway 自体は `permit_consumed`、`provider_unavailable`、`upstream_uncertain` 等を返します。`upstream_uncertain` はその要求の応答を完了できなかったエラーであり、provider の稼働判定や容量プールの保留ではありません。metadata API と Gateway はブラウザー Origin 付き要求を拒否します。

## モデルのシステムプロンプト

管理画面のモデル登録・編集にある「システムプロンプト」は論理モデル単位で保存し、同じモデルの各配置で共有します。空欄は指示なし、上限は16,384 Unicode文字です。外側の空白を除き、本文・改行を維持します。編集は既存のcatalog revision確認とatomic保存を通り、変更後はDesktop/Runnerのモデル設定の再確認が必要です。プロジェクトやHub全体の指示階層は持ちません。

この指示は公開catalogにも含まれます。Desktop/Runnerは確認済みrevisionと実際に割り当てられたmodelを使い、要求ごとに一度だけ指示を組み込みます。Gatewayは指示を追加・置換しません。候補は許可モデル・必須機能・wait policyで絞ります。preferredだけを待つ場合は代替モデルを含めません。指示のある選択可能な候補を利用するclientはPrepareに`supports_model_system_prompt:true`を付けます。省略はfalseとして扱い、未対応clientにはpermit発行前にHTTP 409 `model_system_prompt_unsupported`を返します。空欄のモデルは従来のPrepareを利用できます。

JSON catalog schemaは5です。schema 1–4は、対応する旧形式の内容を保持してロード時に一度revisionを進めて保存します。新しいSub/Approve標準モデルは未指定として読み、旧schemaで新しい標準値を名乗るデータは拒否します。schema 1–3で新しいモデル指示を名乗るデータも従来どおり拒否します。Hub管理者は指示の保存と確認を行い、実際のモデルが常にその指示に従うことは別途実行結果で確認します。

## AIホストのAPIキー

管理画面で登録先ごとにキーを入力し、モデル一覧取得・「状態を確認」・実行準備時のモデル存在確認・Gatewayの生成要求で同じ認証設定を使います。送信形式は `Authorization: Bearer …` です。Desktopのrequest tokenをAIホストへ転送せず、従来のキー未設定ホストは認証なしのまま利用できます。

キーの実値はHub保存先の `upstream-credentials/<ULID>.json` に置き、Windowsでは利用者限定ACL、Unixでは `0600` のファイル権限で保護・検証します。暗号化保存ではありません。Deploymentには任意の不透明な `upstream_credential_id` だけを保持します。schema 3で加わったこの参照を含め、従来のcatalogは既存モデルとHub識別情報を保持してschema 5へ前方移行します。公開端末catalog・管理snapshot・grantはキーを含まず、登録前のdiscovery evidenceが保持するキーもメモリー内だけで、応答へserializeしません。

登録先の編集ではキー欄の空欄が保持、入力が置換、削除チェックが削除です。Gatewayは予約受付時にキーを保持し、生成を開始した通信はキー変更後も継続します。まだclaimしていない要求は、他のモデル設定変更と同様にrevisionの再確認が必要です。再確認後の新しい要求から変更後のキーを使います。登録済み参照の欠落、権限不正、接続先・profileの不一致では失敗し、認証なしへ切り替えません。

Hubのバックアップ対象に `upstream-credentials` は含めません。catalogの参照は保持するため、復元・別Hubへの移行後は管理画面からキーを入力し直します。保存・解決は [upstream_auth.rs](../src/upstream_auth.rs)、管理操作は [admin.rs](../src/admin.rs)、要求単位の保持・送信は [gateway.rs](../src/gateway.rs) が所有します。

## 中継接続数と応答の完了

core の期限付き permit は予約計算です。Gateway は未使用予約と自身が中継する HTTP 要求を数え、登録した共有容量プールと全体上限をこの Gateway 経路で強制します。これは Hub 経由の同時接続上限です。通常の LM Studio / oMLX はこの permit を検証せず、他アプリや別 Gateway からの要求や、接続終了後の provider 内の処理は数えません。GPU の容量、推論全体の同時実行数、キューは provider 側で管理します。

未使用予約は TTL で解放します。claim 後はローカル HTTP 処理が存在する間だけ枠を保持し、応答完了、Stop、接続切断、通信失敗や期限切れによる後始末で解放します。permit の期限切れだけでは進行中の HTTP 処理を終了済みにしません。Chat Completions の正確な `data: [DONE]` 行、Responses の `response.completed` / `response.failed` / `response.incomplete` と一致する response status を持つ完全な SSE event、および応答 EOFを、自身が受信した stream の終端検証に使います。Responses の失敗・不完全イベントは変更せず Desktop の既存 decoder に渡し、生成成功として扱いません。非 streaming Chat は完全な JSON を検証します。不完全な応答や上限超過はその要求のエラーとして通知し、容量プールや Hub の起動を保留しません。要求を自動再送せず、消費済みの request token を再利用しません。

接続 timeout は 3 秒、headers / 各 response read の無通信 timeout は 300 秒です。**応答が継続している間の全体時間制限はありません。** body は 8 MiB、response は 64 MiB、SSE 行と Responses event data は各 1 MiB が上限です。redirect を追わず、入力 Authorization を provider に転送しません。下流への backpressure は有界キューで受けます。

管理画面は「Hub のモデル中継」に「中継中」「予約」の件数を表示します。中継中の件数は既存の Running record から導出し、接続が閉じてローカルの後始末中のものも終了まで含めます。互換 snapshot の `responding` / `draining` は response channel の有無による内訳で、provider の生成状態ではありません。`quarantined_pools` は互換用の空配列です。`uncertain` はローカル Gateway から状態を取得できなかった場合だけに使い、oMLX / LM Studio の生成を指しません。

## 停止・異常終了と回復

Gateway process は `gateway-execution.lock` を保持して同じ保存先からの子プロセスの重複起動を防ぎます。このロックはローカルの process を表し、provider の実行状態ではありません。listener の bind や TLS 検証に失敗した場合は、その起動エラーを返します。旧版の `gateway-uncertain.json` は過去の記録として変更せずに残しますが、現行版は読み書きせず、起動・割当の条件に使いません。provider の全生成が止まったことを管理者に確認させる操作もありません。

「接続の受付を停止」と Hub サーバーの明示終了は、TLS 受付・モデル管理・Gateway を同じ停止経路へ通します。ブラウザーやタブの終了はこの経路を呼びません。CA、永続端末IDと登録を保持します。旧MCPのグループ・許可・受入済み仕事の認可経路とroot取消も互換データとして保持します。旧MCPの通信停止ではpresenceと60秒の短期資格を破棄し、端末が公開先を再通知してから同じ仕事の照会・停止資格を得ます。受入済みの仕事は24時間を超えても時間だけで破棄しません。受入通知が不明な intent は `recovery_required` として受入側の永続台帳を照合し、自動再実行しません。モデル client / 未使用 permit は別の一時状態です。端末失効や公開 OFF は新規受付の拒否であり、既に claim したモデル要求の終了を意味しません。

Hub サーバーの明示停止や終了、サーバープロセスの消滅によって stdin が閉じると、Gateway は新規受付を閉じ、未使用予約を失効させ、自身の HTTP 処理へ取消を通知します。ブラウザーの消滅ではサーバーと Gateway のパイプは閉じません。後始末の待機は最大2秒です。サーバーは子の完了通知を最大3秒待ち、通常終了を確認できなければローカルの Gateway 子プロセスを終了させます。provider のプロセスは終了させません。ローカル終了の確認に失敗した場合はその状態を表示し、生存中の同じ子プロセスに重ねて起動しません。

HTTP 接続を閉じた後に生成を取り消すか、完了まで続けるかは provider が決めます。Hub の再開始を provider 全体のアイドル確認に結び付けません。途中で切れた応答を成功に変えたり、停止・再起動後に自動再送したりすることもありません。MCP の受入済み仕事の永続台帳と取消・照合は別の責務で、以下の認可記録の契約を維持します。

<a id="証明書の期限と計画停止"></a>

## 証明書の期限と更新

Hub / Gateway のサーバー証明書は30日間有効です。期限の7日前から毎分、同じ公開 CA・実 IP・予約名を持つ新しい証明書を生成し、Gateway が検証して受理した後に Hub へ適用します。新しい TLS handshake が新証明書を使用し、既存接続と生成応答を切断・再送しません。管理画面から同じ更新処理を明示実行することもできます。

更新に失敗した場合は現在の証明書を維持し、固定の更新失敗状態と現在の期限を表示します。期限を超えれば新しい接続の TLS 検証は失敗します。失敗が続く場合は CA と Gateway の状態を確認してください。CA root の自動更新・信頼の自動交換は対象外です。

端末証明書更新と server 証明書更新は別です。モデル client ID/token は端末 ID に結び付き、新しい制御要求では現在の端末証明書を検証します。新しい permit はその時点の証明書に結び付き、開始済み要求を更新のために再送しません。

## 仕事の認可記録と診断

Hub は本文を保存せず、origin / actor / audience / profile / scope / root / request key / parent と受入側の job ID を永続化します。同じ intent の資格再発行は同じ authority ID を使います。受入側は永続 job 台帳を作成し、Hub の `jobs/accepted` を確認してから通常の実行を開始します。完了は受入側だけが `jobs/settled` で通知し、他端末による通知、job ID の差し替え、保存中の同じ受入 job を別 authority へ結び付ける通知を拒否します。再委任には受入済みの親 job と現在の有向許可が必要です。

未終端は最大128 root / 4096 authority、1 root は計32種類の intent です。未終端の経過時間を根拠に枠を空けません。全子孫が終端の root は最大128件の最近記録を残し、24時間後または件数超過時に新規受付に伴って整理します。退役した root の再実行は `authority_retired` で拒否します。整理後の遠隔再照会を永久保証せず、Desktop の保存済み結果と canonical 履歴を保持します。端末失効後も、登録済みの現在の秘密鍵と期限内証明書を持つ受入側は、受入済みの exact job の終端通知だけを送れます。実行やモデル利用を再許可する例外ではありません。期限切れ・鍵喪失などで終端を確認できない仕事は未確認として残り、沈黙だけで強制終端にしません。

mTLS `GET /v1/network/diagnostics` はメタデータ / Gateway の実 endpoint、公開証明書の期限と更新状態、Gateway のローカル受付可否だけを返します。ローカル稼働を他 PC からの到達成功として扱いません。MCP 接続確認は専用の60秒 inspect 資格で lifecycle / tools/list のみを確認し、仕事・再委任 intent を生成しません。再委任中の確認では受入済みの親 job と元端末の DELEGATED 許可も照合し、通常の直接接続許可へ持ち替えません。inspect で仕事の実行・照会・停止はできません。

起動・管理とサーバーの境界は`src/cli.rs`、`src/host_process.rs`、`src/web_admin.rs`、`src/admin.rs`、`src/admin/`が所有します。実行・認可の実装と負例は`src/gateway.rs`、`src/gateway_process.rs`、`src/service/execution_api.rs`、`src/network/`、`tests/gateway_execution.rs`、`tests/device_network.rs`が正本です。実providerやDesktopとの操作証跡はtask-local検証記録へ置き、fixture成功を実機成功として扱いません。サーバーとブラウザーだけの管理経路は実装・検証中であり、ブラウザー終了後の通信継続、明示停止、更新後の再開を変更後の実プロセス・実ブラウザーで確認します。

## 同一PC向けモデルAPI（互換）

「従来の手動接続（同じPC内）」はloopback `127.0.0.1:9470` だけの互換入口です。開始ごとに共有tokenを指定し、停止時にclient登録とtokenを失効します。新規チーム導入は端末ネットワークを使います。

モデル管理APIの実装は [service.rs](../src/service.rs) です。登録には共有Bearer token、登録後の操作には発行した `client_token` を使います。カタログは互換用途で共有tokenも受け付けます。

| Method / path | 用途 |
| --- | --- |
| `GET /v1/health` / `GET /v1/catalog` | Hub識別情報、版、revision、公開モデル一覧。provider endpointや生成本文は含めない |
| `POST /v1/clients/register` | `{label}` からserver-session内のIDとclient tokenを発行 |
| `POST /v1/clients/heartbeat` | 自clientのactivityと任意のactive turnを報告 |
| `POST /v1/clients/review` | `context`、Hub ID、review済みrevision、選択モデルを照合して保存 |
| `POST /v1/clients/disconnect` | 自clientのtokenとcontextを失効 |
| `POST /v1/requests/prepare` | 端末・context・turn・request・revisionを確認し、待機理由またはGateway宛先を返す |
| `POST /v1/turns/finish` / `POST /v1/turns/cancel` | turnを閉じ、予約と当該中継接続を終了する |

`context` は `main` / `side_chat` / `approve`、revisionは十進文字列です。確認保存はMain/Sub/Approve独立で、一覧取得やheartbeatは確認の代わりになりません。heartbeatで有効な同一turnのleaseを延長しても、permit期限やrequest IDは再発行しません。IPや自己申告のactivityを端末認証・provider実測として使いません。厳密なrequest/responseと互換条件は同ファイルと近傍testsを参照してください。

公開catalogと端末のmodel-session応答は `default_selections:{main,side_chat,approve}` を返します。各値は従来の選択DTOまたはnullです。Mainはツール利用対応を必要とし、SubとApproveの `required_capabilities` は空です。未指定のSub/ApproveをMainへ補完せず、有効な登録先がない明示標準もnullにします。旧 `default_selection` は端末model-session応答のMain互換値として保持します。Hubの標準値は新規PCの候補であり、既に確認したPCの選択を変更しません。
