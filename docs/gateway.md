# Gateway の実行境界

Hub 本体は catalog、端末、選択、revision、turn、lease、request ID と容量のメタデータを扱います。prompt / response は Desktop から別 process の Gateway に送られ、Gateway が provider に転送します。同じ `moyai-hub` 実行ファイルを `--gateway` で起動し、秘密情報は親子の stdin パイプだけで初期化します。

Gateway の管理用待受は一時的な IPv4 loopback port です。従来の同一 PC 接続はこの HTTP 経路を使います。Hub の「端末ネットワーク」を開始した場合は、同じ子プロセスに別ポートの **相互 TLS が必須の生成用 listener** を開きます。Hub のメタデータ listener と生成本文を処理する listener は別で、生成用 listener に管理 API は公開しません。生成用ポートは詳細設定で固定でき、0 は自動選択です。固定ポートが使用中なら開始を拒否し、自動ポートへ切り替えません。変更は次回開始から適用します。組織の Firewall では両ポートを確認してください。アプリは Firewall 規則を作成しません。

## 認証と実要求

登録後の model client 専用 Bearer で `POST /v1/requests/prepare` に次を送ります。未知フィールドを拒否します。従来の protocol 1 接続では loopback HTTP を使います。端末ネットワーク接続では、永続端末の証明書で HTTPS `/v1/network/model-session` に接続して専用 client を得ます。その後のモデル API も、その端末の現在の証明書と client token の両方を検証します。受入側の遠隔 job はそれぞれ独立した model client を持ち、同じ Main active turn を共有しません。

```json
{"id":"client-id","context":"main","turn_id":"stable-user-turn","request_id":"request-attempt","expected_hub_id":"hub-id","reviewed_revision":"3"}
```

`context` は `main` / `side_chat`、revision は正の十進文字列です。待機時は 200 `{state:"waiting",reason,retry_after_ms:1000}`。reason は `busy` / `global_capacity` / `target_unavailable` / `maintenance` / `jit_pending` です。

準備成功は 200 `{state:"ready",lease_id,permit_id,logical_model_id,gateway_base_url,request_token,expires_at_ms,provider_profile:"openai_compatible_chat",provider_api_mode:"chat_completions"}`。期限は十進文字列です。従来の target は `http://127.0.0.1:<port>/r/<permit_id>/v1`、端末ネットワークでは `https://<Hub-IP>:<Gateway-port>/r/<permit_id>/v1` です。Desktop は `/chat/completions` を付加し、専用 request token を Bearer に指定します。body.model は logical model ID を使用し、Gateway が登録済み provider model ID に変換します。実 endpoint・provider credential は Desktop に配布しません。現在は認証不要の provider が対象です。

ネットワークの permit は発行先端末の証明書に結び付きます。別端末が token を取得しても使用できず、HTTP 経路への持ち替えも拒否します。Gateway は実行の claim 時に Hub へ照会し、証明書が現在も有効か確認します。Desktop は共通設定の公開 CA をアプリ内で信頼し、Hub / Gateway の証明書と接続先 IP を検証します。端末の TLS 秘密鍵や共通の長期 token を配布する方式ではありません。

Hub / Gateway の server 証明書だけに予約名 `hub.moyai.invalid` を含めます。managed client は実接続先の IP とこの予約名を同じ証明書で検証し、同じ CA・同じ IP の端末証明書を Hub と取り違えません。端末 CSR で予約名を要求すると拒否します。予約名は実 IP の検証を置き換えるものではありません。

一つの request ID は未使用の予約である間だけ prepare を再取得できます。一度 claim した token は再利用できません。実要求を再試行する場合は新しい request ID を使い、同じ user turn は同じ turn ID を保持します。claim 時にも最新 revision、端末の生存、context、active turn、期限を Hub に照合します。claim 後の revision 変更は進行中の本文転送を切りません。

モデル割当と Desktop 間の作業委任は別の資格です。作業委任では Hub の公開先一覧と方向付き許可から対象限定の短期資格を発行し、受入側 Desktop が毎回照会します。その仕事が必要とするモデル生成は上記 model session と Gateway を使います。Hub は仕事本文を受け取らず、同じ仕事を二度実行しない責任は受入側の永続 job 台帳が持ちます。

`POST /v1/turns/finish` と `/v1/turns/cancel` は `{id,context,turn_id}` を受け取り、`{id,context,turn_id,closed:true}` を返します。切断も含めて未使用の予約を失効させます。開始済み要求は端末側の Stop / 接続切断だけでは空き枠に戻しません。

HTTP エラーは本文や URL を含まない `{error,current_revision?}` です。401 `unauthorized`、409 `review_required` / `different_hub` / `active_turn` / `turn_closed` / `permit_expired` / `lease_expired` / `selection_required` / `stale_client`、400 `invalid_request` / `model_removed` / `capability_mismatch`、503 `gateway_unavailable` / `gateway_quarantined`、429 `registry_full` を返します。Gateway 自体は `permit_consumed`、`provider_unavailable`、`upstream_uncertain` 等を返します。metadata API と Gateway はブラウザー Origin 付き要求を拒否します。

## 容量と終了確認

core の期限付き permit は予約計算です。Gateway は未使用予約と実行中要求を数え、登録した共有容量プールと全体上限をこの Gateway 経路で強制します。通常の LM Studio / oMLX はこの permit を検証しません。他アプリや別 Gateway から provider へ直接送った要求を認識・制限できず、GPU 全体の排他を保証しません。

未使用予約だけが TTL で解放されます。claim 後は TTL、Stop、切断、端末 heartbeat 失効では枠を早期解放せず、上流を読み切ります。SSE は正確な `data: [DONE]` 行と応答 EOF、非 streaming は完全な JSON 応答を終端確認に使用します。接続開始前の明確な失敗以外で、通信断・期限切れ・未終端・上限超過が起きた場合は容量プールを保留し、新規実行を拒否します。

接続 timeout は 3 秒、headers / 各 response read の無通信 timeout は 300 秒です。**応答が継続している間の全体時間制限はありません。** body は 8 MiB、response は 64 MiB、SSE 行は 1 MiB が上限です。redirect を追わず、入力 Authorization を provider に転送しません。下流への backpressure は有界キューで受けます。

管理画面の「応答中」「上流終了確認待ち」は、既存の Running record と端末への response channel から毎回導出します。channel の weak reference が生存し受信側が開いているものを前者、受信側が閉じたものや weak reference が失効したものを後者として数え、合計が実行枠を保持している要求数です。接続先から bytes が今届いているという意味ではなく、応答接続の状態です。独立した停止 flag や実行 counter は持たず、この分類だけでは枠を解放しません。観測に失敗した場合は不明として表示します。

## 停止・異常終了と回復

開始前に `gateway-uncertain.json` を作り、Gateway process は `gateway-execution.lock` を保持します。 子はすべての listener の bind と TLS 検証を終えてから受付を開始します。生成用 bind の失敗を受付前の型付き応答で報告し、親が正常退出まで確認できた場合だけ、失敗した起動の marker を解除します。未知の起動エラー・異常退出・終了未確認は保留を維持します。Hub の明示停止や終了で stdin が閉じると新規受付を閉じ、未使用予約を失効させます。Hub の停止 UI は子の応答を 3 秒待ち、さらに正常終了を確認できた場合だけ marker を削除します。未完了なら marker を保持したまま子が drain を続けます。

「ネットワークを停止」と Hub のウィンドウ終了は、TLS 受付・モデル管理・Gateway を同じ停止経路へ通します。CA、永続端末 ID、グループ、許可、受入済みの仕事の認可経路と root 取消を保持します。presence と60秒の短期資格は破棄し、端末が公開先を再通知してから同じ仕事の照会・停止資格を得ます。受入済みの仕事は24時間を超えても時間だけで破棄しません。受入通知が不明な intent は `recovery_required` として受入側の永続台帳を照合し、自動再実行しません。モデル client / 未使用 permit は別の一時状態です。端末失効や公開 OFF は新規受付の拒否であり、既に claim したモデル要求の終了を意味しません。

子の drain 待機は最大 305 秒です。そこで終端未確認の要求が残る場合や、Hub / Gateway の異常終了時は marker が残り、次回起動で自動的に容量を安全とみなしません。正常なウィンドウ終了は停止 command と同じ処理を待ってからアプリを終了し、確認できた子の正常終了だけが marker を解除します。プロセス終了で provider 側の実行も終了したとは限りません。

再開始するには provider 側の実行終了を管理者が確認し、停止中の Hub で「接続先のモデル実行が終了したことを確認しました」を明示選択して保留を解除します。実行ロックを保持する旧 Gateway が生存している間は解除できません。現段階は provider 自身への確実な cancel / 実行照合 API を持たず、この確認を自動化しません。

<a id="証明書の期限と計画停止"></a>

## 証明書の期限と更新

Hub / Gateway のサーバー証明書は30日間有効です。期限の7日前から毎分、同じ公開 CA・実 IP・予約名を持つ新しい証明書を生成し、Gateway が検証して受理した後に Hub へ適用します。新しい TLS handshake が新証明書を使用し、既存接続と生成応答を切断・再送しません。管理画面から同じ更新処理を明示実行することもできます。

更新に失敗した場合は現在の証明書を維持し、固定の更新失敗状態と現在の期限を表示します。期限を超えれば新しい接続の TLS 検証は失敗します。失敗が続く場合は CA と Gateway の状態を確認してください。CA root の自動更新・信頼の自動交換は対象外です。

端末証明書更新と server 証明書更新は別です。モデル client ID/token は端末 ID に結び付き、新しい制御要求では現在の端末証明書を検証します。新しい permit はその時点の証明書に結び付き、開始済み要求を更新のために再送しません。

## 仕事の認可記録と診断

Hub は本文を保存せず、origin / actor / audience / profile / scope / root / request key / parent と受入側の job ID を永続化します。同じ intent の資格再発行は同じ authority ID を使います。受入側は永続 job 台帳を作成し、Hub の `jobs/accepted` を確認してから通常の実行を開始します。完了は受入側だけが `jobs/settled` で通知し、他端末による通知、job ID の差し替え、保存中の同じ受入 job を別 authority へ結び付ける通知を拒否します。再委任には受入済みの親 job と現在の有向許可が必要です。

未終端は最大128 root / 4096 authority、1 root は計32種類の intent です。未終端の経過時間を根拠に枠を空けません。全子孫が終端の root は最大128件の最近記録を残し、24時間後または件数超過時に新規受付に伴って整理します。退役した root の再実行は `authority_retired` で拒否します。整理後の遠隔再照会を永久保証せず、Desktop の保存済み結果と canonical 履歴を保持します。端末失効後も、登録済みの現在の秘密鍵と期限内証明書を持つ受入側は、受入済みの exact job の終端通知だけを送れます。実行やモデル利用を再許可する例外ではありません。期限切れ・鍵喪失などで終端を確認できない仕事は未確認として残り、沈黙だけで強制終端にしません。

mTLS `GET /v1/network/diagnostics` はメタデータ / Gateway の実 endpoint、公開証明書の期限と更新状態、Gateway のローカル受付可否だけを返します。ローカル稼働を他 PC からの到達成功として扱いません。MCP 接続確認は専用の60秒 inspect 資格で lifecycle / tools/list のみを確認し、仕事・再委任 intent を生成しません。再委任中の確認では受入済みの親 job と元端末の DELEGATED 許可も照合し、通常の直接接続許可へ持ち替えません。inspect で仕事の実行・照会・停止はできません。

実装と負例は `src/gateway.rs`、`src/gateway_process.rs`、`src/service/execution_api.rs`、`src/network/`、`tests/gateway_execution.rs`、`tests/device_network.rs` が正本です。実 provider と 2 アプリの操作証跡は Desktop 側の task-local 検証記録に置き、fixture 成功を実機成功として扱いません。
