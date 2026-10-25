# ブラウザーからHubを管理する

Hub PCの起動操作、または管理を許可されたPCのDesktopから開きます。ID・パスワード入力は不要です。URLを知っているだけでは管理操作できません。チームの導入手順は[チーム準備](team-preparation.md)へ、端末通信のWindows設定は[受信許可](firewall.md)へまとめています。

## このPCで使う

初回は[Setup-Hub.cmdで導入](windows-setup.md)し、次回からはスタートメニューの **moyAI Team Management** で開きます。配置先の`Start-Hub.cmd`、または次のコマンドでも開けます。

```powershell
.\moyai-hub.exe --launch
```

初回の管理URLは `http://127.0.0.1:9080/admin/` です。保存済みのWeb設定がない場合は、実行ファイルと同じフォルダーの初期`web-management.json`を読み込みます。別の固定ポートは`Setup-Hub.cmd -WebPort 9081`で指定できます。Program Files内の初期ファイルを編集する必要はありません。初期ファイルでは1以上のポート番号を指定し、0や省略はエラーにします。

```json
{
  "schema": 1,
  "settings": {
    "local_port": 9080,
    "remote": { "mode": "off" }
  }
}
```

起動に成功した設定は、保存先の `web-management.json` に保存されます。次回からは保存済みの設定を使い、同梱ファイルの変更は反映されません。実行ファイルだけを使い、隣に初期設定ファイルがない場合も、初回はポート9080・他端末向け公開OFFで起動します。指定ポートが使用中なら起動エラーになり、別のポートへ自動では切り替わりません。

設定を変更する場合はHubを停止し、`--web-port PORT` または `--web-settings FILE` を指定して起動します。これらの明示指定は、保存済みの設定と同梱の初期設定より優先されます。例えば、ローカル管理ポートを9081に変更するコマンドは次のとおりです。成功した設定は保存され、次回も使います。現在のURLは `--status` で確認できます。

```powershell
.\moyai-hub.exe --launch --web-port 9081
```

起動ticketは60秒・一度限りで、URLのfragmentから管理cookieへ交換した後、アドレスから除きます。期限切れ・使用済み・通信結果不明の場合は同じ起動入口で開き直してください。「この画面の利用を終了」はブラウザーの資格だけを失効し、Hubや仕事を止めません。

管理画面ではモデル、PC参加、プロジェクトの操作・実行PC、端末通信と保守を扱います。旧グループ・方向付き許可は通常画面の新規設定では使いません。「MCP履歴」は旧方式で各Desktopに保存された記録の閲覧です。

### ほかの操作と保存が重なった場合

プロジェクト編集中の入力は保持します。「入力を残して最新の設定と比較」で差を確認し、「比較した内容で編集を続ける」から保存します。比較だけでは保存しません。削除された対象は選び直し、削除や実行停止確認などの操作は対象と内容を開き直します。通信結果不明時は同じ要求の受付を確認し、新しい操作として重複送信しません。

## 保存先・状態確認・終了

保存先は `--data-dir`、`MOYAI_HUB_DATA_DIR`、Windows既定の `%LOCALAPPDATA%\midi-ai-labs\moyai-data\hub` の順です。同じ保存先は一つのHubだけが所有します。旧既定からの移行・停止条件・初期化は[Windowsへの導入と削除](windows-setup.md)を参照してください。

```powershell
.\moyai-hub.exe --launch --data-dir C:\moyai-hub-data
.\moyai-hub.exe --status --data-dir C:\moyai-hub-data
.\moyai-hub.exe --stop --data-dir C:\moyai-hub-data
```

ブラウザーを閉じてもHubとGatewayは稼働します。「接続の受付を停止」は端末通信を停止し、Webを維持します。`--stop` は本体を終了し、`--status` は状態とWeb URLを確認します。どちらも停止中のHubを起動しません。

### ワンクリックでホスティングする

`--launch` は保存済み端末通信とGatewayを再開してブラウザーを開きます。起動済みなら同じHubを使います。初回は画面でLAN側IPとポートを設定します。標準ポートはHub9471、Gateway9472で、旧Gateway設定0は自動選択のまま保持します。現在の受信許可も照合し、変更が必要なら画面へ案内します。OS設定は利用者が確認して実行したときだけ変えます。

ネットワーク再開の失敗では修正用Web画面を残し、Web自体の開始失敗では終了して `hub-startup.log` へ記録します。直接 `moyai-hub.exe` を実行するコンソール起動ではWebだけを開始し、端末通信は画面から開始します。この方法はCtrl+Cで終了できます。

`--launch --no-browser` は画面を開かず結果をJSONで返します。稼働状態、Web URL、ネットワークエラー、受信許可状態を含みます。既存サーバーへ `--web-port` / `--web-settings` を渡して設定を変えることはできません。正確な出力型は [launch.rs](../src/host_process/launch.rs) を参照してください。

「サインイン時の自動起動」はHub PCのローカル画面から、このWindows利用者のログオン時起動を登録します。サービス・ログオン前の稼働ではありません。標準Setupのアンインストールは一致する登録を解除します。Hub単体の削除・設置先変更・独自変更した開始登録は元の設定方法で解除してください。

## 他のPCから使う

他端末向けWeb公開は初回OFFです。公開する場合はJSONファイルを用意し、起動時に`--web-settings`で指定します。このCLI用JSONは、下の例のように `local_port` と `remote` を直接記述します。同梱・保存用の `web-management.json` にある `schema` と `settings` のラッパーは付けません。`--web-port`と同時には指定できません。どの方式でも、このPC専用のloopback HTTPを併設します。

```powershell
.\moyai-hub.exe --data-dir C:\moyai-hub-data --web-settings C:\moyai-hub-data\web-settings.json
```

| `remote.mode` | 公開方法 | 準備するもの |
| --- | --- | --- |
| `off` | このPC専用 | `local_port`のみ |
| `http` | 接続先案内のみ。管理画面の利用・管理操作は不可 | 待受先と公開HTTP Origin |
| `https` | HubがHTTPSを受け付ける | 待受先、公開HTTPS Origin、Web用証明書と秘密鍵 |
| `proxy` | 同じPCのreverse proxy経由 | 固定loopback待受とproxyの公開HTTPS Origin |

別PCから管理するには、Hubの詳細設定でそのPCを管理権限のある利用者へ結び付け、HTTPSの管理URLを設定します。管理権限はその利用者を引き継ぐ全PCが対象です。そのPCのDesktopで「Hubの管理画面を開く」を押すと、相互TLSで現在のPC登録・利用者の対応・管理権限を確認して一度限りの起動ticketを渡します。PCの停止・失効・対応先変更後に以前の管理資格を使い続けられません。LANの平文HTTPは管理操作不可です。

遠隔の管理画面ではプロジェクト・PCの許可と保存設定を管理できます。モデル接続先の探索・変更、Hubの待受設定やWindows変更など、Hub PC上の実行・通信を伴う管理操作はHub PCから直接開いた画面で行ってください。詳しい許可対象は現在の管理command境界が決めます。Windowsの受信許可は[Hub／Gatewayの限定受信許可](firewall.md)を確認し、UACを経て設定します。

### 既存のLAN HTTP公開設定（互換）

設定自体は読み取れますが、このURLでは管理操作を行えません。別PCのDesktopから開くには下記のHTTPS設定を使います。Hub PCではスタートメニューまたはCLIから開いてください。

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

- ブラウザーは設定ファイルを直接変更せず、認証付きの管理commandを送ります。保存時はrevisionを照合し、周期更新で編集中の入力を置き換えません。
- Web資格はHttpOnly・SameSite=Strict cookieを使い、HTTPSではSecureを付けます。管理要求はsession識別子、CSRF、Host・Originを確認し、別タブで資格が変わった旧画面の操作を拒否します。
- CLIのprivate `/host/*`、端末のmTLS、ブラウザー管理は別入口です。Web管理者の資格に端末接続ファイルやHub CAを流用しません。
- Web設定は保存先の `web-management.json` へ開始成功後に保存します。同梱・保存用ファイルは `schema: 1` と `settings` を持つ形式です。破損・未知schema・待受や証明書の不備では元設定を上書きしません。
- `--web-port` はこのPC専用の設定として保存し、他端末向け公開をOFFにします。ポート0は空きポートを選びます。公開設定は停止後、正しい引数で起動し直してください。
- 配布する `hub-config.moyai-join` は公開URLとCAです。旧MCP履歴のダウンロードは明示取得した一時snapshotで、現行共有会話のHub保存とは別です。

実装は [web_admin/](../src/web_admin/) と [host_process/](../src/host_process/) を参照してください。[ブラウザー試験](../tests/browser/README.md)で実行範囲を記録し、外部HTTPS/proxy・物理別端末・長時間運転・配布物の受入は個別の証跡で判断します。
