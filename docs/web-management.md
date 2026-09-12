# ブラウザーからHubを管理する

2026-09-11。Rustサーバーと通常のWebブラウザーだけで管理する構成へ変更しました。HubのTauri管理アプリ、管理キー、ログイン・ログアウトを廃止し、公開CLIとEdgeの基本操作を確認済みです。確認範囲は末尾に記載します。

モデルの登録・編集・一覧確認、端末の承認・停止・削除、グループと方向付き利用許可、ネットワークの開始・停止、選択した端末のMCP履歴をブラウザーで操作します。保存処理はRustサーバーの同じ`Admin`が所有します。Desktop用の相互TLSと端末の利用許可は独立して維持します。

## このPCで使う

PowerShellなどからHubを起動します。

```powershell
.\moyai-hub.exe
```

初回は`http://127.0.0.1:9080/admin/`を自動で待ち受け、接続用URLをコンソールに表示します。そのURLをEdgeやChromeで開いてください。サーバーが専用ブラウザーを起動することはなく、ログイン操作・証明書・証明書の例外設定は不要です。端末ネットワークを開始していなくても管理画面を使えます。

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

`--launch --no-browser`は結果（PID・Web URL・開始状態・初回設定の要否・ネットワークエラー）をJSONで返します。設定済みのネットワークを開始できない場合は終了コード1、初回未設定は0です。`--web-port` / `--web-settings`は未起動時のみ併用でき、起動済みサーバーのWeb設定を暗黙に変えません。保存先の選択順序は通常起動と同じです。

## 他のPCから使う

他端末向けWeb公開は初回OFFです。公開する場合はJSONファイルを用意し、起動時に`--web-settings`で指定します。`--web-port`と同時には指定できません。どの方式でも、このPC専用のloopback HTTPを併設します。

```powershell
.\moyai-hub.exe --data-dir C:\moyai-hub-data --web-settings C:\moyai-hub-data\web-settings.json
```

| `remote.mode` | 公開方法 | 準備するもの |
| --- | --- | --- |
| `off` | このPC専用 | `local_port`のみ |
| `http` | 指定したIP・ポートでHTTP公開 | 待受先と公開HTTP Origin |
| `https` | HubがHTTPSを受け付ける | 待受先、公開HTTPS Origin、Web用証明書と秘密鍵 |
| `proxy` | 同じPCのreverse proxy経由 | 固定loopback待受とproxyの公開HTTPS Origin |

Web管理に利用者認証はありません。公開URLへ到達できる利用者は設定の変更やMCP履歴の取得を操作できます。HTTPSは通信の暗号化であり、管理者の識別を追加しません。公開範囲は待受IP、ファイアウォール、既存proxyなどの運用設定で決めます。Hubがファイアウォールを自動変更することはありません。

### LAN内へHTTPで公開する

以下は例です。待受IPと公開先は利用するHubの値へ置き換えます。

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

共通設定やMCP履歴のMarkdownはブラウザーのダウンロード先へ保存します。履歴は管理画面で明示的に取得した時点のスナップショットです。全件を常時収集するものではなく、保存元は各Desktopです。

Web設定はdata directoryの`web-management.json`（schema 1）へ、開始成功後にatomic保存します。CLIへ渡すJSONは`local_port`と`remote`の設定本体であり、保存用の`schema`・`settings`ラッパーは付けません。設定ファイルの上限は32 KiBです。証明書のパスを保持しますが、秘密鍵本文をWeb設定へ保存しません。

設定の破損・未知schema、ポート競合、証明書の不備などでWebを開始できなければ、公開CLIの起動は失敗して終了します。元の設定は上書きしません。正しい`--web-settings`か`--web-port`を明示して再起動し、待受と保存が成功したときに設定を更新します。

WebはHost・Origin照合、CSRF、要求サイズ・同時処理数・処理期限の上限を適用します。`/admin/session`は認証なしで同一Origin用のCSRF値を返す入口です。ログインセッションや認証Cookieは作りません。CSRF値は管理者の本人確認や端末資格ではありません。

静的画面はビルド時に組み込みます。ブラウザー操作は許可された`Admin::dispatch`へ送られ、端末の相互TLS APIやprivate `/host/*`制御とは入口を分けます。private制御は同じPCのCLIから、利用者限定ファイルに保存した資格で接続します。

## 今回の確認範囲

サーバーとブラウザーだけの起動・操作経路を実装しました。[Playwrightによるブラウザー試験](../tests/browser/README.md)で、実Hubの起動、ログインなしの操作、ネットワークの開始・停止、グループ保存、共通設定のダウンロード、後始末を、このWindowsのEdgeを表示した実操作で確認しました。再接続・従来の手動サーバー開始停止・760px幅の画面も確認しました。Rustの160件、UIの36件とビルドが通過しています。旧Tauriアプリや管理キーログインを使った試験の成功は、この変更後の合格を意味しません。

外部HTTP/HTTPS/proxy・物理別端末・全GUI・長時間試験・配布物の受入は、実施した証跡がある範囲だけを完了とします。
