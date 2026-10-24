# moyAI Hubの実装索引

HubはRustサーバー、ブラウザー管理画面、モデル中継用Gatewayで構成します。moyAI Desktopと組み合わせて使います。型・保存形式・制限値・状態遷移の正本はsourceと近傍testsです。利用手順は[README](../README.md)、検証の入口は[ブラウザー試験](../tests/browser/README.md)へ集約します。

## 責任範囲

- HubはPC登録、プロジェクト参加、モデル選択、共有会話・仕事・入力・成果・資源と停止確認を保存する。
- GatewayはモデルHTTP通信を中継し、自身の接続数を制限する。モデルロード・GPU容量・provider内部の生成終了は管理しない。
- Runnerは端末上の作業フォルダーと実プロセスを管理し、Hubへ実行・停止を報告する。
- Desktopは依頼・承認・成果確認の入口。ブラウザーはHub管理の入口で、閉じてもHubと仕事を停止しない。

## Source owner

| 責務 | Source / 詳細 |
| --- | --- |
| CLI、起動済みHubの再利用、保存先、停止 | [cli.rs](../src/cli.rs)、[host_process/](../src/host_process/)、[Web管理](web-management.md) |
| 設定保存と管理command | [admin.rs](../src/admin.rs)、[admin/](../src/admin/) |
| Web認証、起動ticket、CSRF、用途別cookie、HTTPS | [web_admin/](../src/web_admin/)、[identity/](../src/identity/) |
| モデルcatalogとrevision | [catalog.rs](../src/catalog.rs)、[core.rs](../src/core.rs)、[protocol.rs](../src/protocol.rs) |
| モデル一覧と接続profile | [provider.rs](../src/provider.rs)、[discovery.rs](../src/discovery.rs) |
| AIホストのAPIキー保存・解決 | [upstream_auth.rs](../src/upstream_auth.rs)、[OSファイル権限](../src/host_process/private_file.rs)、[中継時の扱い](gateway.md#aiホストのapiキー) |
| モデル選択・確認保存・実行準備 | [service/](../src/service/)、[待ち順序](request-queue.md)、[子実行](delegated-execution.md) |
| 別processのモデル中継と終了 | [gateway.rs](../src/gateway.rs)、[gateway_process.rs](../src/gateway_process.rs)、[Gateway](gateway.md) |
| PC登録、CA、端末証明書、mTLS | [network/](../src/network/)、[identity/device.rs](../src/identity/device.rs)、[PC登録](device-registry.md) |
| プロジェクトの操作/実行PC、参加世代・離脱、フォルダー準備 | [project_setup.rs](../src/shared_work/project_setup.rs)、[management.rs](../src/shared_work/management.rs) |
| 共有会話CRUD・停止・最新発言編集 | [conversation_catalog.rs](../src/shared_work/conversation_catalog.rs)、[stop_scope.rs](../src/shared_work/stop_scope.rs) |
| 仕事の受付、割当、承認、資源、担当交代 | [shared_work/](../src/shared_work/)、[共有API](shared-work.md) |
| 入力・成果・会話archive、保持期限 | [data.rs](../src/shared_work/data.rs)、[blob_store.rs](../src/shared_work/blob_store.rs) |
| 継続アプリの保持と停止確認 | [services.rs](../src/shared_work/services.rs) |
| バックアップ・復元 | [backup.rs](../src/backup.rs)、[保守手順](shared-work.md#保守と復元) |
| 旧MCP履歴の明示取得 | [network/history.rs](../src/network/history.rs)、[履歴](mcp-history.md) |
| 管理画面の表示・編集・競合比較 | [ui/src/](../ui/src/)、[browser tests](../tests/browser/) |
| Windows受信許可の照合・設定script | [admin/firewall.rs](../src/admin/firewall.rs)、[受信許可](firewall.md) |

## 認証・保存・互換性

通常のPC利用は端末IDと鍵を使い、ID/PW入力を求めません。Web管理はHub PCの起動操作または管理を許可された登録PCから専用資格を取得します。公開URL、CSRF値、Windowsユーザー名だけで権限を与えません。端末用mTLS、ブラウザー用Web、CLIのprivate制御は別の入口です。

単一データフォルダーは一つのHub processが所有します。設定保存はrevisionと入力を照合し、永続化成功後にruntimeへ反映します。共有DBはforward migrationで更新し、公開済みmigrationを書き換えません。

AIホストのAPIキーはcatalog外の利用者限定ファイルに保存します。catalogには任意の参照IDだけを持ち、公開端末catalog・管理snapshot・grantにキーを含めません。バックアップから除外し、参照先が欠けた場合は接続を拒否します。保存形式・更新と実行中要求への適用範囲は[Gateway](gateway.md#aiホストのapiキー)を参照してください。

旧グループ・方向付き許可・受入済みMCP仕事・利用者役割は互換データとして維持します。新規の仕事と会話はHubプロジェクトを使い、旧履歴から自動移行しません。旧MCP履歴の閲覧は端末へ明示照会する一時snapshotで、現在の共有会話保存とは異なります。

Desktop側の設計は[複数PC利用](../../moyAI/docs/design/multi-device-session.md)、[モデル接続](../../moyAI/docs/hub-integration.md)、[共有Runner](../../moyAI/docs/runner-shared.md)を参照してください。隣接repositoryへのリンクは開発workspace用で、Hub実行時の依存ではありません。

## 実装と検証の区別

実装の存在は、物理複数PC・会社のFirewall・外部HTTPS/proxy・長時間運転・最終配布物の合格を意味しません。実行範囲は各taskのRESULTSで確認します。2026-09-23のLive試験は同一Windows上の仮想WinA（操作専用）と唯一の実行PC WinBです。

未対応の主な範囲はHubルートCAの全端末への自動入替、Windowsサービス運用です。現在のWindows利用者のログオン時自動起動は実装済みで、OSサービスとは別です。
