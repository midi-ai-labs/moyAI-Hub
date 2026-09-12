# moyAI Hub architecture

2026-09-11。Rustサーバー、ブラウザー管理画面、モデル管理、端末ネットワークとGatewayの実装索引。HubのTauri管理アプリを廃止し、公開CLIとEdgeの基本操作を確認済み。型、保存形式、制限値、状態遷移の正本は下記のsourceとpassing testsとする。

## 現在の実装と owner

Hubは独立したRustサーバーであり、`Admin`と`HubCore`、端末ネットワーク、保存データ、端末認証、listenerとGatewayを一元所有する。通常起動でWeb管理を自動開始し、利用者はEdgeやChromeで表示URLを開く。ブラウザーは設定ファイルを直接編集しない。タブやブラウザーの終了・強制終了では通信は止まらず、同じURLから再接続できる。通信停止はWeb操作、サーバー終了はCLIの`--stop`またはforegroundのCtrl+Cで行う。LLM providerのprocessやDesktopのworkspace・推論本文を所有しない。

要求のFIFO待ち順序は [core/queue.rs](../src/core/queue.rs) が所有し、serviceのprepareはこの入口へ一本化する。[待ち順序と期限](request-queue.md) に、再問い合わせ・Gatewayの容量拒否・取消・別poolの扱いを記載する。

ローカル子エージェントの独立した実行contextは [core/delegated.rs](../src/core/delegated.rs) が同じRouteContext内で管理する。親の確認済み選択のsnapshot、子ごとの終了・取消・heartbeat、終了済みIDの保持と上限は [子エージェントの実行境界](delegated-execution.md) に記載し、MCPのjob authorityとは分ける。

| 責務 | Source owner | 近傍の確認先 |
| --- | --- | --- |
| 公開CLIの起動・保存先・Web設定・状態表示・終了、private ACL付き資格情報、認証済みloopback制御と期限 | [cli.rs](../src/cli.rs)、[host_process.rs](../src/host_process.rs)、[private_file.rs](../src/host_process/private_file.rs)、[main.rs](../src/main.rs) | CLI解析tests、[host_process.rs](../tests/host_process.rs)、[browser fixture](../tests/browser/fixture.mjs) |
| サーバーの単一Admin owner、保存前revision照合、atomic save、data directory排他、管理command dispatchとworker実行 | [admin.rs](../src/admin.rs) | 同ファイルのdispatch / 永続化 tests、[host_process.rs](../tests/host_process.rs) |
| 永続 catalog、stable ID、software / catalog revision、変更差分、保存内容の検証 | [catalog.rs](../src/catalog.rs) | [core_control_plane.rs](../tests/core_control_plane.rs) |
| 端末 identity / presence と route context の分離、選択集合、review gate、advisory lease / permit、取消・期限、モデル観測 | [core.rs](../src/core.rs) | [core_control_plane.rs](../tests/core_control_plane.rs) |
| 共通の状態・選択・待機・エラー・割当投影の型 | [protocol.rs](../src/protocol.rs) | core の境界・再起動・idempotence tests |
| 認証付きカタログ、端末登録 / heartbeat、Main / Side の確認保存、切断・トークン失効、client 公開情報の投影 | [service.rs](../src/service.rs) | 同ファイルの HTTP tests、[client_review_api.rs](../tests/client_review_api.rs) |
| 永続端末・鍵に束縛した参加申請と承認・可逆な新規利用停止・グループ・方向付き許可・恒久失効、名前一意性・登録削除、公開先・短期資格・再委任経路 | [network/mod.rs](../src/network/mod.rs)、[network/join.rs](../src/network/join.rs)、[network/registry.rs](../src/network/registry.rs) | [device_network.rs](../tests/device_network.rs)、[device_network/registry.rs](../tests/device_network/registry.rs)、[名前・削除の契約](device-registry.md) |
| 受入jobの永続authority、root取消・保持上限・退役、exact親経路 | [network/authority.rs](../src/network/authority.rs)、[network/mod.rs](../src/network/mod.rs) | [device_network.rs](../tests/device_network.rs) |
| Hub CA・CSR 所有証明・証明書発行、TLS peer identity、接続上限 | [network/pki.rs](../src/network/pki.rs)、[network/tls.rs](../src/network/tls.rs) | [device_network.rs](../tests/device_network.rs) |
| Hub/Gateway leafの無停止更新、証明書projection、公開診断 | [network/certificate.rs](../src/network/certificate.rs)、[network/server_runtime.rs](../src/network/server_runtime.rs) | [server_certificate_rotation.rs](../tests/server_certificate_rotation.rs) |
| mTLS ネットワーク API、受入時の権限照会、モデル session への接続 | [network/api.rs](../src/network/api.rs) | [device_network.rs](../tests/device_network.rs)、[gateway_execution.rs](../tests/gateway_execution.rs) |
| Web管理からのMCP指示/実行履歴照会、既存mTLS上の自端末ポーリング、サイズ・対象検証、サーバーメモリ内snapshotと期限 | [network/history.rs](../src/network/history.rs)、[network/api.rs](../src/network/api.rs)、[network_admin.rs](../src/admin/network_admin.rs) | [device_network/history.rs](../tests/device_network/history.rs)、実ブラウザーの閲覧・Markdown保存 |
| 端末別 prepare / turn 終了、claim 時の最新 revision・owner gate、metadata の再観測 | [execution_api.rs](../src/service/execution_api.rs) | [gateway_execution.rs](../tests/gateway_execution.rs) |
| 別プロセス内の一回限りの認証、Chat Completions / Responsesの無変換中継、自身の中継接続数、応答終端の検証と後始末 | [gateway.rs](../src/gateway.rs) | 同ファイルと [gateway_execution.rs](../tests/gateway_execution.rs)、[responses.rs](../tests/gateway_execution/responses.rs) |
| hostを親とする同一exeのGateway子process、stdin初期化、明示停止・親host消滅時の後始末、重複起動を防ぐ実行lock | [gateway_process.rs](../src/gateway_process.rs) | [gateway_execution.rs](../tests/gateway_execution.rs)、[host_process.rs](../tests/host_process.rs) |
| 管理者入力 endpoint の正規化、4 profileのmetadata/API対応、provider metadata取得、観測根拠の解釈 | [provider.rs](../src/provider.rs)、[discovery.rs](../src/discovery.rs) | 同ファイルの parser / endpoint tests、[Gateway profile表](gateway.md#認証と実要求) |
| ブラウザーの非同期管理client、CSRF取得と再接続、タブを閉じてもhostを停止しないlifecycle | [management.ts](../ui/src/management.ts)、[web_admin.rs](../src/web_admin.rs)、[host_process.rs](../src/host_process.rs) | [browser smoke](../tests/browser/web-admin.spec.mjs)、Web HTTP tests |
| 端末ネットワークの管理command、公開共通設定の生成、TLSとGatewayの一括開始・停止 | [network_admin.rs](../src/admin/network_admin.rs) | [device_network.rs](../tests/device_network.rs)、当該commandの実ブラウザー操作 |
| 公開共通設定・取得済み履歴のブラウザーダウンロード | [management.ts](../ui/src/management.ts) | [browser smoke](../tests/browser/web-admin.spec.mjs)、実ブラウザーのMarkdown保存 |
| 独立Web listener・設定保存、loopback HTTPと明示したHTTP/HTTPS/同一PCproxy、公開CLIの自動開始と設定復元 | [web_admin/server.rs](../src/web_admin/server.rs)、[host_process.rs](../src/host_process.rs)、[admin.rs](../src/admin.rs) | [server/tests.rs](../src/web_admin/server/tests.rs)、設定・寿命・待受失敗と復元、[導入・終了手順](web-management.md) |
| 認証なしWeb、固定Host/Originと起動世代付きCSRF、管理command allowlist、静的画面の埋込み | [web_admin.rs](../src/web_admin.rs)、[build.rs](../build.rs)、[management.ts](../ui/src/management.ts)、[web_admin.ts](../ui/src/web_admin.ts) | [web_admin/tests.rs](../src/web_admin/tests.rs)、[browser smoke](../tests/browser/web-admin.spec.mjs)、[導入・終了手順](web-management.md) |
| 入力 draft、focus、候補取得の generation、Grid 描画、操作 feedback | [main.ts](../ui/src/main.ts)、[state.ts](../ui/src/state.ts)、[style.css](../ui/src/style.css) | [state.test.ts](../ui/test/state.test.ts)、実ブラウザー操作 |
| 接続端末の申請/承認/停止一覧、所属・方向付き許可の draft と revision 照合、公開設定の受け渡し | [network.ts](../ui/src/network.ts)、[network_state.ts](../ui/src/network_state.ts) | [network.test.ts](../ui/test/network.test.ts) |
| MCP履歴の端末・方向選択、ページと詳細、取得日時・未接続表示、遅延応答の対象照合 | [history.ts](../ui/src/history.ts)、[history_state.ts](../ui/src/history_state.ts) | [history.test.ts](../ui/test/history.test.ts)、実ブラウザー操作 |

永続 catalog と現在の観測は分けて扱う。サーバー再起動で model client、lease、permit、モデル一覧の観測を復元しない。ブラウザーの再読込だけではサーバー内の状態を維持する。端末ネットワークの CA・端末 ID・グループ・許可と受入jobの認可経路・root取消は保持する。サーバー再起動時の presence と短期資格は復元せず、現在の公開先と端末証明書で再取得する。認可の保持・退役は [Gateway の実行境界](gateway.md#仕事の認可記録と診断) を参照する。画面の入力と選択は TypeScript が保持し、保存・副作用の可否はサーバーの Rust owner が判定する。

端末用APIはカタログ、自端末の観測・確認保存・切断、Main / Sideの生成準備とturn終了を扱う。Main / Sideのreviewは異なるcontextに保存し、端末用トークンからmutation ownerを検証する。管理操作とprovider endpointは端末用APIへ公開しない。Web管理には利用者認証がなく、公開URLへ到達できる利用者が設定変更や履歴取得を操作できる。固定Host/Originと起動世代に束縛したCSRF・操作allowlistは本人認証ではない。Webから端末ネットワークと従来のモデルサーバーを開始・停止できる。Web設定の変更はCLIを指定した再起動で行う。Web待受は端末TLSと分離し、このPCは127.0.0.1のHTTP、任意公開はHTTP・設定済みPEMのHTTPS・同一PCproxyを使う。network停止ではWebを止めず、host終了では両方を停止する。private `/host/*`制御APIはブラウザーから呼べず、Forwarded系ヘッダーから公開先やidentityを推測しない。coreのlease / permitは割当計算であり、Gatewayが制限するのは自身の中継HTTP接続数である。接続切断や不完全応答は当該要求の失敗と後始末で扱い、他要求やHub再起動の保留条件にしない。モデル一覧で存在を確認した対象へ要求を送り、ロード・アンロード、GPU容量、生成のキュー・実行・終了はoMLX/LM Studioが管理する。[Gatewayの実行境界](gateway.md)と[README](../README.md)を参照する。

Desktop の従来接続は Hub の識別情報と logical model の選択を保存し、model client / 実行トークンは runtime に限る。新しい端末連携は端末自身の秘密鍵・証明書・永続 ID を保持する。Hub は公開先と方向付き許可を扱い、Desktop 間の仕事本文を中継しない。Direct / Hub の選択、待機・Stop・user turn と遠隔 job の境界は Desktop の runtime が所有する。

MCP履歴の閲覧では、Web管理で指定した端末の保存済み記録を、既存の相互TLSポーリングで明示的に取得する。これは実行経路とは別の読み取り専用操作で、本文をカタログ・端末台帳へ保存しない。要求は1端末1件・60秒、完了snapshotは最大64件・30分、遠隔Markdownは64 KiBまでに制限する。新規利用停止中も診断を許可し、失効・資格期限切れ・別端末からの結果提出を拒否する。取得日時と接続状態を分けて表示し、Hubからの実行・停止APIは設けない。[利用手順と通信境界](mcp-history.md) を参照する。

## Desktop と共有する設計

2 アプリのユースケース、責任分界、review、Main / Side Chat、MCP 配信、受入条件は、Desktop repository の [LYNX / Hub integration](../../moyAI/docs/hub-integration.md) を参照する。ここへ契約本文を複製しない。このリンクは開発 workspace の隣接 checkout 用であり、Hub の build / runtime 依存ではない。

端末参加、証明書、直接依頼と再委任の許可、複数端末の受入条件は [Hub device network](../../moyAI/docs/design/hub-device-network.md) を参照する。

同文書の `REC-HUB-CONTROL-PLANE-01` と `REC-HUB-CATALOG-REVISION-01` は上表の Hub owner に対応する。`REC-DESKTOP-HUB-ROUTING-01` と `REC-DESKTOP-MCP-PUBLISH-01` の実行・公開 authority は Desktop 側にあり、Hub の情報 API と共用しない。

## 後続の工程

今回のFIFO待ち順序、Responses、独立子エージェント、端末名一意・削除、周期更新の操作保持は実装済み・最終検証中である。HubはRustサーバーとブラウザー管理へ移行し、Tauriアプリと管理ログインを廃止した。公開CLIとEdgeの基本操作は確認済み。公開CLIは初回127.0.0.1:9080、以後は保存済み設定でWebを自動開始する。外部HTTP/HTTPS/proxy、物理多端末・3台再委任・長時間soak・全GUI・配布物の受入は実施した証跡だけを完了とする。旧Tauri/管理キーログイン構成の成功は変更後の合格へ流用しない。CA root自動rotation、認証付きprovider discovery、Windowsサービス登録・ログオン時自動起動・OS再起動後の自動復旧は別範囲として残す。ブラウザーを閉じてもサーバーが続くこととOSサービス化は異なる。GPU容量・モデルload・provider全生成の終了照合と旧手動MCPの第二trustは追加工程に含めない。
