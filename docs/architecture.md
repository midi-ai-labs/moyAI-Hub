# moyAI Hub architecture

2026-09-08。モデル管理、端末ネットワークと別プロセス Gateway の実装索引。型、保存形式、制限値、状態遷移の正本は下記の source と passing tests とする。

## 現在の実装と owner

Hub は独立した Rust / Tauri アプリであり、管理画面と情報 API が同じ `HubCore` を参照する。LLM provider の process や Desktop の workspace・推論本文を所有しない。

| 責務 | Source owner | 近傍の確認先 |
| --- | --- | --- |
| 永続 catalog、stable ID、software / catalog revision、変更差分、保存内容の検証 | [catalog.rs](../src/catalog.rs) | [core_control_plane.rs](../tests/core_control_plane.rs) |
| 端末 identity / presence と route context の分離、選択集合、review gate、advisory lease / permit、取消・期限、モデル観測 | [core.rs](../src/core.rs) | [core_control_plane.rs](../tests/core_control_plane.rs) |
| 共通の状態・選択・待機・エラー・割当投影の型 | [protocol.rs](../src/protocol.rs) | core の境界・再起動・idempotence tests |
| 認証付きカタログ、端末登録 / heartbeat、Main / Side の確認保存、切断・トークン失効、client 公開情報の投影 | [service.rs](../src/service.rs) | 同ファイルの HTTP tests、[client_review_api.rs](../tests/client_review_api.rs) |
| 永続端末・鍵に束縛した参加申請と承認・可逆な新規利用停止・グループ・方向付き許可・恒久失効、公開先・短期資格・再委任経路 | [network/mod.rs](../src/network/mod.rs)、[network/join.rs](../src/network/join.rs) | [device_network.rs](../tests/device_network.rs)、[device_network/join.rs](../tests/device_network/join.rs) |
| 受入jobの永続authority、root取消・保持上限・退役、exact親経路 | [network/authority.rs](../src/network/authority.rs)、[network/mod.rs](../src/network/mod.rs) | [device_network.rs](../tests/device_network.rs) |
| Hub CA・CSR 所有証明・証明書発行、TLS peer identity、接続上限 | [network/pki.rs](../src/network/pki.rs)、[network/tls.rs](../src/network/tls.rs) | [device_network.rs](../tests/device_network.rs) |
| Hub/Gateway leafの無停止更新、証明書projection、公開診断 | [network/certificate.rs](../src/network/certificate.rs)、[network/server_runtime.rs](../src/network/server_runtime.rs) | [server_certificate_rotation.rs](../tests/server_certificate_rotation.rs) |
| mTLS ネットワーク API、受入時の権限照会、モデル session への接続 | [network/api.rs](../src/network/api.rs) | [device_network.rs](../tests/device_network.rs)、[gateway_execution.rs](../tests/gateway_execution.rs) |
| ローカル管理者のMCP指示/実行履歴照会、既存mTLS上の自端末ポーリング、サイズ・対象検証、メモリ内snapshotと期限 | [network/history.rs](../src/network/history.rs)、[network/api.rs](../src/network/api.rs)、[network_admin.rs](../src/desktop/network_admin.rs) | [device_network/history.rs](../tests/device_network/history.rs)、実Tauriの閲覧・Markdown保存 |
| 端末別 prepare / turn 終了、claim 時の最新 revision・owner gate、metadata の再観測 | [execution_api.rs](../src/service/execution_api.rs) | [gateway_execution.rs](../tests/gateway_execution.rs) |
| 別プロセス内の一回限りの認証、Chat Completions 転送、実容量・上流終端・終了不明の保留 | [gateway.rs](../src/gateway.rs) | 同ファイルと [gateway_execution.rs](../tests/gateway_execution.rs) |
| 同一 exe の子 process、stdin 初期化、停止・実行ロック・再起動時の保留 | [gateway_process.rs](../src/gateway_process.rs) | [gateway_execution.rs](../tests/gateway_execution.rs) |
| 管理者入力 endpoint の正規化、provider metadata 取得、観測根拠の解釈 | [discovery.rs](../src/discovery.rs) | 同ファイルの parser / endpoint tests |
| Tauri command、保存前の revision 照合、atomic save、data directory 排他、listener lifecycle | [desktop.rs](../src/desktop.rs) | 当該 command の実 Tauri 操作 |
| 端末ネットワークの管理 command、公開共通設定の保存、TLS と Gateway の一括開始・停止 | [network_admin.rs](../src/desktop/network_admin.rs) | 実 Tauri 操作（共通設定・参加許可・停止・再許可を同一 Windows で確認） |
| 入力 draft、focus、候補取得の generation、Grid 描画、操作 feedback | [main.ts](../ui/src/main.ts)、[state.ts](../ui/src/state.ts)、[style.css](../ui/src/style.css) | [state.test.ts](../ui/test/state.test.ts)、実 Tauri 操作 |
| 接続端末の申請/承認/停止一覧、所属・方向付き許可の draft と revision 照合、公開設定の受け渡し | [network.ts](../ui/src/network.ts)、[network_state.ts](../ui/src/network_state.ts) | [network.test.ts](../ui/test/network.test.ts) |
| MCP履歴の端末・方向選択、ページと詳細、取得日時・未接続表示、遅延応答の対象照合 | [history.ts](../ui/src/history.ts)、[history_state.ts](../ui/src/history_state.ts) | [history.test.ts](../ui/test/history.test.ts)、実Tauri操作 |

永続 catalog と現在の観測は別の owner にする。再起動で model client、lease、permit、provider のロード状態を復元しない。端末ネットワークの CA・端末 ID・グループ・許可と受入jobの認可経路・root取消は保持する。presence と短期資格は復元せず、現在の公開先と端末証明書で再取得する。認可の保持・退役は [Gateway の実行境界](gateway.md#仕事の認可記録と診断) を参照する。画面の入力と選択は TypeScript が保持し、保存・副作用の可否は Rust command が判定する。

公開 API はカタログ、自端末の観測・確認保存・切断、Main / Side の生成準備と turn 終了を扱う。Main / Side の review は異なる context に保存し、端末用トークンから mutation owner を検証する。管理操作と provider endpoint はローカル管理側に閉じている。core の lease / permit は割当計算であり、実際の容量は別プロセス Gateway が上流の終端まで保持する。通常の provider への直接実行はこの制御外である。[Gateway の実行境界](gateway.md) と [README](../README.md) を参照する。

Desktop の従来接続は Hub の識別情報と logical model の選択を保存し、model client / 実行トークンは runtime に限る。新しい端末連携は端末自身の秘密鍵・証明書・永続 ID を保持する。Hub は公開先と方向付き許可を扱い、Desktop 間の仕事本文を中継しない。Direct / Hub の選択、待機・Stop・user turn と遠隔 job の境界は Desktop の runtime が所有する。

MCP履歴の閲覧では、ローカル管理者が指定した端末の保存済み記録を、既存の相互TLSポーリングで明示的に取得する。これは実行経路とは別の読み取り専用操作で、本文をカタログ・端末台帳へ保存しない。要求は1端末1件・60秒、完了snapshotは最大64件・30分、遠隔Markdownは64 KiBまでに制限する。新規利用停止中も診断を許可し、失効・資格期限切れ・別端末からの結果提出を拒否する。取得日時と接続状態を分けて表示し、Hubからの実行・停止APIは設けない。[利用手順と通信境界](mcp-history.md) を参照する。

## Desktop と共有する設計

2 アプリのユースケース、責任分界、review、Main / Side Chat、MCP 配信、受入条件は、Desktop repository の [LYNX / Hub integration](../../moyAI/docs/hub-integration.md) を参照する。ここへ契約本文を複製しない。このリンクは開発 workspace の隣接 checkout 用であり、Hub の build / runtime 依存ではない。

端末参加、証明書、直接依頼と再委任の許可、複数端末の受入条件は [Hub device network](../../moyAI/docs/design/hub-device-network.md) を参照する。

同文書の `REC-HUB-CONTROL-PLANE-01` と `REC-HUB-CATALOG-REVISION-01` は上表の Hub owner に対応する。`REC-DESKTOP-HUB-ROUTING-01` と `REC-DESKTOP-MCP-PUBLISH-01` の実行・公開 authority は Desktop 側にあり、Hub の情報 API と共用しない。

## 後続の工程

端末連携の未確認 GUI 操作と物理 LAN の受入試験、CA root 自動 rotation、provider 側の終了照合、Chat Completions 以外の wire、background / tray、配布は残る工程である。参加申請・許可・停止・再許可の実画面確認済み範囲は共有設計の実装状況を参照する。現在の Gateway 経路を外部直接実行も制御できる構成とみなさない。変更時は共有設計の該当ユースケースと affected source / tests を更新し、この索引には owner の変更だけを反映する。
