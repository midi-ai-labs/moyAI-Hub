# ブラウザーからHubを管理する

2026-09-12。Rustサーバーと通常のWebブラウザーで管理します。管理操作には利用者アカウントによる管理者ログインを必須としました。HubのTauri管理アプリや端末参加資格を管理者認証に流用しません。過去の匿名管理画面の確認結果と、新しい認証導線の確認結果を区別します。

モデルの登録・編集・一覧確認、端末の承認・停止・削除、グループと方向付き利用許可、ネットワークの開始・停止、選択した端末のMCP履歴をブラウザーで操作します。保存処理はRustサーバーの同じ`Admin`が所有します。Desktop用の相互TLSと端末の利用許可は独立して維持します。

「Desktopの接続」は、接続の開始・接続ファイルの配布・端末の参加許可を順に進める入口です。「利用者と仕事の設定」では、利用者・プロジェクト・仕事の実行先を準備します。「管理画面のURL」は管理者がこの画面を開くためのURLです。一般利用者にはDesktop用の接続ファイルと利用者アカウントを渡し、管理用URLをDesktopの接続先として入力しません。

## このPCで使う

PowerShellなどからHubを起動します。

```powershell
.\moyai-hub.exe
```

初回は`http://127.0.0.1:9080/admin/`を自動で待ち受け、接続用URLをコンソールに表示します。そのURLをEdgeやChromeで開き、管理者のユーザー名・表示名・パスワードを設定してください。パスワードは12文字以上、UTF-8で1024バイト以下です。次回からログインが必要です。このPCのloopback HTTPではTLS証明書を要求しません。端末ネットワークを開始していなくても管理画面を使えます。

管理者の初回設定は、このPCの直接loopback接続から一度だけ実行できます。他端末やreverse proxyからの初回登録はできません。共有仕事DBにパスワードのsalt付き検証値とセッションのhashを保存し、平文パスワードやセッションtokenは保存しません。ログアウト時はそのブラウザーのセッションを失効させ、管理表示を閉じます。Hub・Gateway・既存仕事の停止とは別の操作です。

保存済みWeb設定がある場合はその設定で自動開始します。ポートだけを変える場合は`--web-port 9082`のように指定します。`0`は空きポートの自動選択です。`--web-port`は他端末へのWeb公開をOFFにした設定として保存します。

`127.0.0.1`はブラウザーを開いたPC自身を指します。別端末からの管理には下記の公開設定を使います。

## 保存先・状態確認・終了

保存先は`--data-dir`、環境変数`MOYAI_HUB_DATA_DIR`、既定のアプリデータフォルダーの順で決まります。Windowsの既定値は従来版と同じ`%APPDATA%\ai.midi.moyai.hub`です。明示指定がなければ以前のcatalogや端末設定を引き継ぎます。同じ保存先でサーバーを重複起動することはできません。

```powershell
.\moyai-hub.exe --data-dir C:\moyai-hub-data
.\moyai-hub.exe --status --data-dir C:\moyai-hub-data
.\moyai-hub.exe --stop --data-dir C:\moyai-hub-data
```

`--status`は稼働中のHubのWeb URLと状態を表示します。`--stop`はその保存先のHubとGatewayを停止します。どちらも停止中のHubを自動起動しません。起動したコンソールで`Ctrl+C`を押しても終了します。

ブラウザーやタブを閉じてもHub・ネットワーク・Gatewayは動き続けます。ブラウザーの「ネットワークを停止」は端末通信とGatewayを止め、Web管理画面を維持します。Hubサーバー本体を終了するとWebを含む待受も停止します。

サーバーを再起動するとWebは保存済み設定で自動開始します。通常のコンソール起動では端末ネットワークと従来のモデルサーバーは停止状態なので、必要な通信をブラウザーから開始してください。Windowsサービス登録、ログオン時の自動起動、OS再起動後の自動復旧は実装していません。

### ワンクリックでホスティングする

実行ファイルと同じフォルダーへ `scripts/Start-Hub.cmd` と `scripts/Stop-Hub.cmd` を配置します。`Start-Hub.cmd`、または `moyai-hub.exe --launch` を指定したショートカットから、バックグラウンドのHub起動・保存済み端末ネットワークとGatewayの開始・通常の既定ブラウザー起動まで進みます。初回のネットワーク未設定時だけ、Web画面で具体的なIP・ポートを入力して開始します。ブラウザーを閉じてもホスティングは続き、同じ入口を再度開いてもプロセス・待受を重複させません。停止は `Stop-Hub.cmd` または `--stop` です。

成功した待受先と案内IPは `network-hosting.json`（`schema_version: 1`）へatomic保存します。Gatewayポートは既存の端末設定を使います。保存データの破損、IP変更、ポート競合でネットワークを再開できない場合は、エラーを返してWebを維持します。画面から設定を修正し、開始成功時だけ保存を更新します。Web自体の開始失敗はHubを終了し、`hub-startup.log`へ記録します。ログは次回起動時に1 MiBを超えていれば前回分一つへ切り替えます。

`--launch --no-browser`は結果（PID・起動済み本体の再利用を示す`already_running`・Web URL・開始状態・初回設定の要否・ネットワークエラー）をJSONで返します。通常の起動では管理画面にも起動／再利用の案内を表示し、画面上部でHub本体の応答状態を確認できます。この表示はモデル接続や端末ネットワークの開始状態と独立しています。起動案内のURL fragmentは初回応答後に除去し、再読込では案内を繰り返しません。設定済みのネットワークを開始できない場合は終了コード1、初回未設定は0です。`--web-port` / `--web-settings`は未起動時のみ併用でき、起動済みサーバーのWeb設定を暗黙に変えません。保存先の選択順序は通常起動と同じです。

## 他のPCから使う

他端末向けWeb公開は初回OFFです。公開する場合はJSONファイルを用意し、起動時に`--web-settings`で指定します。`--web-port`と同時には指定できません。どの方式でも、このPC専用のloopback HTTPを併設します。

```powershell
.\moyai-hub.exe --data-dir C:\moyai-hub-data --web-settings C:\moyai-hub-data\web-settings.json
```

| `remote.mode` | 公開方法 | 準備するもの |
| --- | --- | --- |
| `off` | このPC専用 | `local_port`のみ |
| `http` | 接続先案内のみ。管理者ログイン・管理操作は不可 | 待受先と公開HTTP Origin |
| `https` | HubがHTTPSを受け付ける | 待受先、公開HTTPS Origin、Web用証明書と秘密鍵 |
| `proxy` | 同じPCのreverse proxy経由 | 固定loopback待受とproxyの公開HTTPS Origin |

Web管理には管理者のユーザー名・パスワードによるログインが必要です。別PCからはHTTPSまたはHTTPSのreverse proxyを使います。LANの平文HTTPではパスワード入力画面と管理操作を公開せず、安全なURLへの案内を表示します。公開範囲は待受IP、ファイアウォール、既存proxyなどの運用設定で決めます。Hubがファイアウォールを自動変更することはありません。

### 既存のLAN HTTP公開設定

設定自体は読み取れますが、このURLでは管理者ログイン・管理操作を行えません。管理には下記のHTTPS設定か、このPCのloopback URLを使います。

```json
{
  "local_port": 9080,
  "remote": {
    "mode": "http",
    "bind": "0.0.0.0:9081",
    "public_origin": "http://192.168.10.2:9081"
  }
}
```

この例の別端末向けURLは`http://192.168.10.2:9081/admin/`です。HTTPでは証明書は使いません。ブラウザーでクリップボード操作が利用できない場合は設定ファイルのダウンロードを使います。

公開Originはscheme・host・portだけを指定し、`/admin/`などのpath、ユーザー情報、query、fragmentを含めません。ブラウザーは設定した公開OriginのURLでアクセスします。別名や別のHostへ自動で信頼範囲を広げません。

### Hub自身でHTTPSを受け付ける

```json
{
  "local_port": 9080,
  "remote": {
    "mode": "https",
    "bind": "0.0.0.0:9081",
    "public_origin": "https://hub.example.com:9081",
    "certificate_path": "C:/certs/hub-fullchain.pem",
    "private_key_path": "C:/certs/hub-key.pem"
  }
}
```

証明書と秘密鍵はHubを起動するアカウントが読み取れる絶対パスで指定します。各PEMファイルの上限は1 MiBです。開始時に秘密鍵との組合せ、leaf証明書の有効期間、公開Originの名前・IPとSANの一致を確認します。証明書の信頼はアクセス元ブラウザーで判定します。

Web証明書の発行・信頼配布・更新は運用側が担当します。Desktopへ配布するHub CAや端末用証明書を、Webの利用者認証や証明書例外として流用しません。証明書・設定を変更する場合はHubを停止してから新しい設定で再起動してください。

### 同じPCのreverse proxyを使う

```json
{
  "local_port": 9080,
  "remote": {
    "mode": "proxy",
    "bind": "127.0.0.1:9081",
    "public_origin": "https://hub.example.com"
  }
}
```

proxyはHubと同じPCで動かします。proxy用待受は固定の`127.0.0.1:ポート`で、このPC専用Webの待受とは別です。proxyは公開Originと一致するHost、元のOrigin、CSRFヘッダー、`/admin/`のpathを保持して転送します。証明書やproxy自体の設定・起動は既存の運用側で行います。

Hubは`Forwarded`や`X-Forwarded-*`から公開先や権限を推測しません。private制御用`/host/*` APIをproxyの転送先にしません。

## 操作・保存と通信の境界

保存時は入力開始時のrevisionを検証し、別の画面が先に保存した場合は最新状態での再確認を案内します。周期更新は入力・選択・折りたたみを保持します。変更の結果を確認できない場合は自動再送しません。

共通設定は `hub-config.toml`、MCP履歴はMarkdownとしてブラウザーのダウンロード先へ保存します。共通設定は各Desktopで同じファイルを取り込み、端末ごとのIDや秘密鍵は含めません。履歴は管理画面で明示的に取得した時点のスナップショットです。全件を常時収集するものではなく、保存元は各Desktopです。

Web設定はdata directoryの`web-management.json`（schema 1）へ、開始成功後にatomic保存します。CLIへ渡すJSONは`local_port`と`remote`の設定本体であり、保存用の`schema`・`settings`ラッパーは付けません。設定ファイルの上限は32 KiBです。証明書のパスを保持しますが、秘密鍵本文をWeb設定へ保存しません。

設定の破損・未知schema、ポート競合、証明書の不備などでWebを開始できなければ、公開CLIの起動は失敗して終了します。元の設定は上書きしません。正しい`--web-settings`か`--web-port`を明示して再起動し、待受と保存が成功したときに設定を更新します。

WebはHost・Origin照合、CSRF、要求サイズ・同時処理数・処理期限の上限を適用します。`/admin/session`はCSRF値と現在の認証状態を返します。ログイン成功時はHttpOnly・SameSite=Strictの用途別cookieを発行し、HTTPSではSecure属性を付けます。管理要求にはcookieと一致するセッション識別子も必要です。別タブで利用者が切り替わっても、古い画面から新しい利用者として操作することはできません。CSRF値そのものは管理者の本人確認や端末資格ではありません。

静的画面はビルド時に組み込みます。ブラウザー操作は許可された`Admin::dispatch`へ送られ、端末の相互TLS APIやprivate `/host/*`制御とは入口を分けます。private制御は同じPCのCLIから、利用者限定ファイルに保存した資格で接続します。

## 今回の確認範囲

新しいログイン導線の実操作は [Playwrightによるブラウザー試験](../tests/browser/README.md) の認証ケースを使います。以前の匿名操作や旧管理キー方式の成功は、今回の利用者認証の合格へ流用しません。実行したケースと残る範囲はtask-localのRESULTSで区別します。

外部HTTP/HTTPS/proxy・物理別端末・全GUI・長時間試験・配布物の受入は、実施した証跡がある範囲だけを完了とします。
