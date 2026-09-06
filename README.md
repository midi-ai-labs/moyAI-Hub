# moyAI Hub

Tauri で動作する LLM ホストの管理アプリです。moyAI Desktop **3.0.0 / LYNX** と連係する Hub の最初の開発版（0.1.0）です。Desktop とは別プロセス・別リポジトリで動作します。

モデル登録・状態確認・revision 管理・認証付きカタログ、Main / Side Chat ごとの確認保存に加え、永続端末登録、TLS による LAN 接続、公開先の探索と方向付きの依頼許可を実装しています。モデル要求は **別プロセス Gateway 経由の Chat Completions** に対応します。Hub 本体は prompt / response を受け取りません。同じ実行ファイルの `--gateway` process が短期実行権を一度だけ受け付け、この Gateway を通る要求の容量を制御します。普通の LM Studio / oMLX への外部からの直接実行は計数・制限できません。

Desktop の「moyAI Hub」画面から接続し、カタログの取得、Main / Side Chat ごとのモデル選択と確認保存、更新後の再確認、切断を行います。選択した logical model と Hub の識別情報を保存し、接続トークンは保存しません。Direct / Hub の選択と生成 lifecycle は Desktop が所有し、Hub 障害時に Direct へ暗黙に切り替えません。

## モデルを登録する

1. 接続方式で `OpenAI compatible / oMLX` または `LM Studio` を選びます。
2. ホストの `IP:port` または HTTP(S) URL を入力します。入力完了後にモデル候補を自動取得します。入力値が変わった場合、古い候補は無効になります。
3. モデルと表示名を選び、登録します。ツール利用を管理者が確認済みの場合に限り「ツール利用を許可（管理者確認）」を選べます。これは provider が報告しない能力の明示的な宣言で、モデル一覧への掲載から能力を推測するものではありません。同一 logical model の別ホストとして追加する場合は「同じモデルの別ホスト・容量を設定」を開きます。登録する capability が異なる配置は同じ logical model に束ねません。
4. 同じ GPU / provider 実行枠を共有する登録先は、「実行枠を共有するグループ」（容量プール）を同じにします。この容量と全体上限は Gateway 経由の実行に適用されます。
5. 「状態を確認」で登録先のメタデータを再取得します。「編集」で表示名、プール容量、メンテナンス状態を変更できます。最後の配置を削除すると、その logical model も削除されます。

モデル登録はローカル設定へ保存されます。表示名、IP、provider の model 文字列を identity として使わず、logical model・配置・プールにはそれぞれ stable ID を付けます。モデルロード・アンロードや provider プロセスの起動・停止は行いません。

### ステータスの意味

| 表示 | 色 | 根拠 |
| --- | --- | --- |
| ロード済み | 青 | LM Studio `/api/v1/models` が `loaded_instances` を報告 |
| 未ロード | 緑 | LM Studio にモデルがあり、`loaded_instances` が空。JIT 自動ロードが設定されているかは確認しない |
| モデルなし | 赤 | モデル一覧の取得成功後、登録済み model ID が一覧に存在しない |
| ロード状態不明 | 灰 | 未確認・期限切れ、ロード情報なし、または HTTP / 応答形式の確認失敗 |
| 接続・確認不可 | 灰 | ホストへの接続に失敗 |
| メンテナンス | 黄 | 管理者が割当候補から除外 |

汎用 `/v1/models` への掲載から ready を推測しません。ただし OpenAI compatible / oMLX は新しい一覧で存在を確認できたモデルに実要求を試行できます。表示はロード状態不明のままで、ロード済み表示も生成の成功を保証しません。状態には観測時刻を付け、runtime の観測は再起動時に復元しません。metadata request は redirect 禁止、接続 3 秒・全体 8 秒、応答 2 MiB・2,048 model の上限を持ちます。現在の discovery は認証不要のホストを対象とします。

概要には登録先ごとの「ロード済み」と「状態不明」の件数を並べます。ロード済みが 0 でも、使えるモデルがないという意味ではありません。左ナビはページ内の移動リンクで、常に概要を選択表示するものではありません。モデル状態や実行経路の技術的な説明は必要時に展開できます。

## 端末ネットワークを開始する

初期状態は停止です。別 PC の Desktop を参加させる場合は、Hub の「端末ネットワーク」を使います。従来のモデルサーバーが稼働している場合は先に停止してください。

1. 「このHubのIP」に、参加端末から到達できる Hub PC の具体的な IPv4 を入力します。ポートの初期値は `9471` です。「ネットワークを開始」で TLS のモデル接続と Gateway をまとめて起動します。
2. 「共通設定をコピー」または「設定ファイルを保存」で、Hub URL と公開 CA を配布します。Desktop の「moyAI Hub → 端末連携」で読み込みます。共通設定に端末 ID、秘密鍵、長期共有トークンは含めません。信頼する管理者から渡された設定を使ってください。
3. 必要なグループを作り、「参加コードを発行」で端末名と所属を指定します。コードは共通設定と別に許可した端末へ渡し、Desktop で参加します。有効期間は 10 分、使用は 1 回です。同じ署名要求の再送は登録応答の回復として扱います。
4. 登録端末の「管理」で名前・所属・失効を管理します。端末は複数グループに所属できます。認証に使う端末 ID は Hub が発行し、秘密鍵は各 Desktop が保持します。IP や端末名の自己申告で別の端末にはなれません。
5. 受入側 Desktop で公開対象と権限を確認し、受付を開始します。Hub の「方向付きの接続許可」で送信元・受入先・公開対象を選びます。端末が参加しただけでは依頼できません。「この宛先の公開対象すべて」は、後から受付を開始する対象も許可範囲に含みます。
6. 送信側 Desktop で利用する受入先を選びます。この選択は Hub の許可を狭める操作です。逆方向には別の許可が必要です。端末間の仕事本文と結果は Desktop 同士の直接 TLS 接続を使い、Hub を中継しません。

グループの許可に端末同士の個別例外を加えられます。再委任には、直前の端末から宛先への「直接の依頼」と、最初の依頼元から宛先への「再委任の宛先」の両方の許可が必要です。例えば 00 → 19 → 20 を許可しても、00 → 20 の直接接続を許可する必要はありません。元の依頼・経路・公開対象を短期資格に結び付け、循環と段数を制限します。

Hub は端末ごとに公開証明書を発行・更新します。公開 CA の信頼はアプリ内に限定し、Windows 全体の信頼ストアへ追加しません。初回参加を除くネットワーク API は端末証明書による相互 TLS 認証が必要です。公開先の探索、対象ごとの資格発行、受入時の照会で現在の許可を検証します。IP が変わった端末は証明書更新と公開情報の再登録が必要です。

公開 OFF、端末失効、許可変更は新しい受付を止め、以前の実行資格を復活させません。**既に受け付けた仕事の停止完了とは別です。** 実行中の仕事は Desktop の停止操作と状態表示で確認します。Hub の停止・ウィンドウ終了はネットワーク受付も止めます。常駐や tray は未実装です。CA、端末、グループ、許可は残りますが、接続状態・短期資格・再委任の経路記録は再起動後に復元しません。再起動前の仕事を自動再実行せず、復旧できない経路の停止は受入側 Desktop で行います。

モデルの実行状態は「応答中」「上流終了確認待ち」「予約」に分けます。端末への応答接続が閉じても provider の終端を確認するまで実行枠を保持します。**Stop は即座の空き枠を意味しません。** 保留と回復の手順は [Gateway の実行境界](docs/gateway.md) を参照してください。

## 従来の同一 PC 接続（互換導線）

「従来の手動接続（同じPC内）」を開きます。初期状態は **停止**、待受先は `127.0.0.1:9470` です。起動するたびにトークンを生成または入力して「サーバーを開始」を押します。生成したトークンは起動前に必要な端末へ安全に共有してください。起動後は画面から消去し、ディスクへ保存しません。

この手順の待受は **loopback のみ** です。別 PC の参加には上記の端末ネットワークを使います。ここで発行する一時的な model client ID / token と、端末ネットワークの永続 ID / 証明書は別です。

「停止」またはウィンドウを閉じると listener が終了します。常駐・tray/background lifecycle は未実装です。共通接続トークンは保持されず、再開始時に設定が必要です。端末別トークンと端末登録は失効し、端末は再登録します。

実生成を使う場合は開始前に「モデル実行も有効にする（Gateway）」を選びます。初期値は無効で、自動起動しません。実行状況を「応答中」「上流終了確認待ち」「予約」に分けて表示します。「応答中」は端末との応答接続がある要求、「上流終了確認待ち」は応答接続が閉じても上流の終端をまだ確認していない要求です。端末側の「操作待ち」とは異なる観測で、**Stop は即座の空き枠を意味しません**。終了不明の保留と回復手順は [Gateway の実行境界](docs/gateway.md) を参照してください。

### モデル API（protocol 1 の互換接続）

すべての API は認証が必要です。登録には管理画面で指定した `Authorization: Bearer <token>` を使います。登録後のカタログ取得、heartbeat、確認保存、切断には発行された端末専用 `client_token` を使い、Desktop は共有トークンを破棄できます。互換性のためカタログ取得では共有トークンも受け付けます。

| Method / path | 入出力 |
| --- | --- |
| `GET /v1/health` | `{hub_id, software_version, revision: string, mode: "management_preview"}` |
| `GET /v1/catalog` | `{hub_id, software_version, revision: string, models: [{id,label,capabilities}], changes: [{revision: string,at_ms,summary}]}` |
| `POST /v1/clients/register` | 入力 `{label}`。出力 `{id,client_token,hub_id,revision: string,heartbeat_interval_ms,identity_scope:"server_session"}` |
| `POST /v1/clients/heartbeat` | 入力 `{id,activity:"idle"\|"waiting"\|"running"}`。端末専用 `client_token` で認証。出力 `{id,revision: string}` |
| `POST /v1/clients/review` | 入力 `{id,context,expected_hub_id,reviewed_revision,selection}`。出力 `{id,context,reviewed_revision: string}`。共有トークンでは保存できない |
| `POST /v1/clients/disconnect` | 入力 `{id}`。端末専用トークンを失効させ、その端末の context を削除。出力 `{id,disconnected:true}`。既に失効したトークンは 401 |
| `POST /v1/requests/prepare` | 端末・context・turn・request・確認済み revision を検証し、待機理由または一回限りの Gateway target を返す。本文は受け付けない |
| `POST /v1/turns/finish` / `POST /v1/turns/cancel` | 入力 `{id,context,turn_id}`。turn を閉じ、未使用の予約を失効させる。既に始まった実行は読み切る |

端末 ID はサーバーが発行し、そのサーバー session 中は同じ ID を使います。IP は HTTP 接続から観測し、認証や identity に使用しません。共通接続トークンでは他端末の heartbeat や確認保存を送れません。端末一覧の「操作待ち」「空き待ち」「処理中」は端末の申告であり、provider の実測値ではありません。Gateway の実行状況は別に表示します。端末一覧は Main / Side Chat の確認状況を独立して表示し、一方の保存は他方を確認済みにしません。

`review` の `context` は `main` または `side_chat`、`reviewed_revision` は正の十進文字列です。`selection` は `allowed_model_ids`、`preferred_model_id`、`required_capabilities`、`wait_policy`、`affinity_turns` を持ちます。待機方針は `wait_for_preferred` または `allow_selected_fallback`。Hub identity の違いと古い revision は 409、空集合・削除済みモデル・capability 不足は 400 で拒否し、以前の確認を保持します。単なるカタログ取得や heartbeat は確認操作になりません。

core は端末 identity と route context を分け、context ごとに選択・lease・active turn・待機を所有します。現在は端末あたり最大 16 context に制限し、HTTP API は Main / Side Chat の 2 context を公開します。lease の N user turn と、個別 request の実行予約は別に扱います。[実行 API](docs/gateway.md) に wire と保証範囲を記載しています。

公開カタログは provider endpoint、credential、prompt / response 本文を含みません。短期 handoff にも実 provider endpoint / model ID を含めません。ネットワーク経由のモデル探索・登録・削除・設定編集はなく、管理操作は Tauri ローカル command に限定します。

## 保存と revision

Tauri のアプリデータフォルダーに `catalog.json`、`device-network.json` と `hub.lock` を保存します。`MOYAI_HUB_DATA_DIR` で検証用保存先を明示できます。両設定の schema version は 1 です。設定ファイルの破損・非対応 schema は起動エラーとし、空設定へ置き換えません。単一のデータフォルダーは OS ファイルロックで一つの Hub インスタンスだけが所有します。

設定の検証後に一時ファイルへ書き、ディスク同期・置換が成功してから runtime の設定を切り替えます。カタログや容量設定・software version の変更は revision を単調増加させ、直近 64 件の差分を保持します。health / catalog は JavaScript での精度損失を避けるため、revision を十進文字列として配布します。端末の選択集合・review 済み revision と Hub の最新 revision は別に扱います。

端末ネットワークはカタログとは独立した revision で編集を照合します。`device-network.json` は CA 秘密鍵を含む Hub ローカルの保存データです。共通設定として配布しないでください。エクスポート機能は公開 CA と URL だけを出力します。端末の秘密鍵は Hub へ送らず、公開証明書・登録・管理者指定の所属を保存します。

## 開発

Rust / Windows Tauri のビルド環境、Node.js（組み込み TypeScript を実行できる版）、WebView2 が必要です。

```powershell
npm ci
npm test
npm run build
cargo test --no-default-features
cargo check --all-features
cargo build --bin moyai-hub
cargo run --bin moyai-hub
```

UI を開発サーバーで動かす場合は `npm run tauri:dev` を使用します。`node_modules` はリポジトリへ含めません。本作業環境では既存 moyAI のキャッシュ済み依存を使用して検証しています。

実 Tauri / WebView2 の管理フローは `node tests/management_smoke.mjs --binary <exe の絶対パス> --artifacts <新しい証跡フォルダーの絶対パス>` で確認します。既存設定を使わず、指定フォルダー内の隔離データと動的 loopback fixture を使用します。登録・状態確認・編集・サーバー開始停止・端末認証・Main / Side 個別確認・古い revision の拒否・端末別切断・再起動を操作し、画像と `results.json` を保存します。管理操作は画面入力、端末側の操作は公開 HTTP API を使います。物理 keyboard / IME、native pointer、WebView2 子プロセスの完全終了は別途確認します。

## 検証状況と残る範囲

- 端末登録・方向付き許可・失効・証明書更新・実 TLS 接続・Gateway 連係は deterministic fixture で検証しています。同一 Windows 上の実 Hub / Desktop 画面と分離した送受信 runtime で、参加・方向付き許可・利用先選択・実作業の結果返却・受付 ON/OFF を確認しました。Hub 最終ビルドでも実画面から開始・稼働・停止を確認し、後始末の監査は PASS しています。期限競合は単体・protocol 試験での検証です。物理 LAN をまたぐ複数 Windows と3台再委任の受入は未実施です。確認範囲は [端末接続の実装状況](../moyAI/docs/design/hub-device-network.md#7-実装状況) を参照してください。
- CA root の自動 rotation、Hub 再起動をまたぐ再委任経路の復元、認証付き provider discovery、tray lifecycle は未実装です。端末 leaf は 30 日、CA root は 10 年の期限を持ち、期限切れ CA の無人交換は行いません。
- Hub / Gateway のサーバー証明書も有効期間は 30 日で、ネットワーク開始時にだけ生成します。連続稼働中の自動更新は未実装です。期限前に仕事の終了を確認して計画的に停止・再開始してください。[証明書の期限と計画停止](docs/gateway.md#証明書の期限と計画停止) に手順と制約を記載しています。
- Responses / その他の provider wire、provider 自身が検証する権限、外部実行を含む負荷把握、終了不明状態の provider 側照合。現 Gateway は Chat Completions のみです。
- MCP 公開対象・作業権限・実行履歴は Desktop が所有します。Hub は公開先と接続許可を扱い、MCP の仕事本文を管理 API へ送る構成にはしません。

実装済みと未実装を分け、preview を production-ready な負荷分散サーバーとして扱わないことを前提としています。
