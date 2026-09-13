# Hub Web管理 GUI試験の範囲

2026-09-12時点の`ui/index.html`、`ui/src/main.ts`、`network.ts`、`history.ts`、`web_admin.ts`にある現役の操作を対象にする。以下は再利用する試験の対応表であり、全操作の受入完了表ではない。各実行のPASS/FAIL・バイナリーSHA-256・画像の目視結果は、その実行の`project_sandbox`レポートを正とする。

## 管理画面と対応する試験

| 画面／操作・状態 | 現在のブラウザーシナリオで確認する内容 | 残るGUI確認 |
| --- | --- | --- |
| ワンクリック起動（ブラウザー外の入口） | `tests/launch.rs` のCLI結合試験で初回・保存復帰・同時起動・ポート競合と復旧・IP変更を確認。ブラウザー試験の件数には含めない | 起動アイコンからEdgeが開きHubが稼働するところまで実操作。初回入力→停止→アイコンで再開→ブラウザー終了後の継続は追加GUI受入待ち（2026-09-12はComputer UseのURL検査で停止） |
| 起動済みHubの案内・本体の稼働表示 | `launch-status.spec.mjs`で複数の起動要求が同じPIDを再利用し、ブラウザーに起動済みの案内を表示。モデル接続停止中も本体は稼働と表示し、本体終了後は応答未確認へ切替。案内は再読込で繰り返さない | 既定ブラウザーが開くタブの選び方はOS／ブラウザーに従う。タブ数を本体の多重起動とみなさない |
| ページ内移動、概要、接続状態 | 直接URL起動、ネットワーク・モデル・接続端末・履歴・Web管理への移動、モデル数とrevision反映、再接続、Tab／Enterでモデルリンクへ移動 | その他のリンクのkeyboard操作、再起動を伴うepoch変更・通信途絶からの回復、稼働中Gatewayの実通信表示 |
| 管理者認証 | `authentication.spec.mjs`で初回設定・入力不一致・ログイン失敗・ログアウト・再ログイン・遅延操作・別タブのセッション置換を確認。LAN HTTPの入口にはpassword入力を出さない | 物理別端末のHTTPS/proxy、複数OS利用者による運用 |
| Web管理 | 認証済み管理者へのローカルURL・稼働状態、760px幅、実clipboardへのローカルURLコピーと読取一致。既存HTTP公開URLは案内として表示 | clipboard拒否時のGUI、公開設定エラー表示。remote HTTPS／proxyは別の起動設定の試験が必要 |
| 従来の手動サーバー | 詳細展開、bind入力、token生成、開始・停止、Web管理の継続、required／短いtoken／不正文字・不正bind、256文字token、Gateway checkbox両値と実起動・停止 | token全境界、使用中ポート・Gateway起動失敗の表示 |
| モデル探索 | 四つのprofile切替、候補選択、HTTP(S)以外の拒否、404／空一覧と回復、前の接続先の遅延応答を破棄 | 認証が必要なprovider、timeout・不正JSON・巨大一覧、実oMLX／LM Studioとの組合せ |
| モデル登録・一覧 | 表示名、tools明示許可、capacity最小値、登録、存在／モデルなし／メンテナンス表示、状態再確認、revision履歴 | 多数の配置での可読性、実際に実行中／待ちの表示、一覧にある未ロードモデルの実生成 |
| モデルの論理名・共有容量 | 既存の論理モデルと容量プールへ追加、capacity入力の有効無効、既存→新規への選択復帰、共有プール編集、4 profileで新規登録、capacity1024／1025、再読込 | 異なるホストの共有プール、能力の異なる配置、実並列要求による容量上限 |
| モデル編集dialog | 表示名・capacity・有効checkbox、保存、背景クリックを無視、入力とfocusをpolling中に維持、削除の二段階確認／取消、Tab／Shift+Tabのfocus範囲・Escape取消と復帰・必須値／capacity検証・Space変更とキーボード保存 | 二つの管理ページでの同一モデル編集・削除競合、IME、長い名前・狭い画面 |
| 端末ネットワーク開始・停止 | IPv4・port入力、開始、設定保存有効化、停止してもWebは稼働、停止中の操作disable、使用中portのエラー・入力保持・修正後の再開始 | IPv6、不正bind、Gatewayだけの起動失敗、他端末からの到達、Windows FW |
| 共通config | GUIダウンロード、URL・公開CA含有、秘密鍵非含有、実clipboardコピーとダウンロード内容の一致 | clipboard拒否時のGUI、ダウンロード取消、Desktop GUIのimportはcombined試験側 |
| 接続詳細・診断・証明書 | Gateway portの範囲検証と自動設定、固定port保存→ネットワーク再開→画面reload後の再開で実Gateway URL一致、実Hub／Gateway診断、FW項目の未実施表示、3回pollingで展開と入力を維持、証明書更新、旧診断の無効化、760px幅 | Hubプロセスの再起動後適用、証明書期限切れ・更新失敗、既存通信を持ったままの更新、手動長時間ちらつき観察 |
| グループ | 追加／編集、一覧、二つのページでの競合拒否・draft維持・最新情報取得後の再保存 | 複数所属、グループ多数・長い名称、同じグループのrenameと同時端末編集 |
| 方向付き接続ルール | 送受信元のグループ／端末、直接／再委任、許可／拒否、深さ1・2、全公開対象／特定profile、追加／個別削除／再読込、端末削除による当該rule除去 | 深さ3の実委任、同時rule変更と受付、複合グループの優先規則、実WinA→WinB→WinC |
| 接続端末の絞り込み | 全端末／承認待ち、空結果、PC名検索のdraft・focus、グループ／IP／識別ID検索とclear | 数百行の可読性・keyboard選択・scroll維持 |
| 参加承認 | 実TLS参加要求、許可、名前・所属を確認して許可、承認済み表示、別端末とのID分離 | 期限切れ申請、承認待ちの名前競合が後から発生する場合、承認と他ページの削除競合 |
| 端末名・所属・詳細 | 空白／大文字小文字を含む同名変更の拒否、rename、required、複数所属checkboxと解除、close／Escape取消とreopen、review後のdraft保持、照合ID展開維持、背景クリックを無視 | 160文字境界、dialogでの他ページ同時変更、複数checkboxのkeyboard操作 |
| 端末の稼働・停止・削除 | mTLS生存確認／公開申告の差、停止・再許可、恒久失効、削除前の端末ID確認・取消・確定、別端末保持、削除済み一覧とreload | 実行中・終了未確認の削除拒否、実際のタスク停止、オフライン化の時間推移、失効後のDesktop GUI |
| 従来の参加コード | 必須label、複数所属ON/OFF、発行・readonly code・期限表示、実clipboard照合、details close/reopen | 互換Desktopの一回限り登録、実時間による期限切れ、再発行 |
| MCP履歴の取得 | 端末行のshortcutが自動取得、種類選択、取得中disable、失敗から再取得、一覧・前後page・空結果、遅い旧種類の応答を破棄 | 別端末への選択変更と応答の競合、60秒timeout、100件超・大量保持・30分expiry |
| MCP履歴の詳細・保存 | 詳細ボタン、取得済み表示、truncated警告、HTMLを文字列として表示、Markdown保存、160行本文のTab／Shift+Tab・PageDown／Ctrl+End／Ctrl+Home・通常poll後のfocus/scroll保持、種類変更時のclear、停止端末から取得、削除済み端末のcached詳細 | 実Desktopの指示／実行履歴との一致、64KiB上限・全文案内、OS保存先選択の取消、再起動後のcache消失 |
| モデル接続の生存確認 | 現在のHub単独試験は空表示まで | Main／Sideの選択承認、待機・処理・再確認・切断表示はactual Desktopを使うcombined試験側 |

## 証拠の読み方

`control-audit.spec.mjs`で追加したcontrol監査の準備範囲は、全ページ内リンク／説明details、手動Gateway ON/OFFと入力検証、4 profile登録、logical/pool既存→新規、端末dialog取消・review・複数所属解除、互換参加コード、深さ3の設定、宛先に応じた公開対象候補、network不正bind、条件付き外部Web URLです。上表の「残るGUI確認」は実GUI証跡が得られるまで自動で完了にしません。新規8 casesとcontrol生成99箇所（所属fieldsetを分けて100 families）の照合・個別実行結果は `project_sandbox/gui-controls-audit-20260912/hub-controls/` に記録します。動的な全model/端末ID、全組合せを検査したことを意味しません。

この追加8件はheaded Edgeで実操作した。自動判定では通った公開対象DDDWの空欄を画像目視で検出し、現在のoption再構築ownerを修正した。`selectedIndex`と選択中option文言も検査する回帰へ強め、新binaryで当該case・端末編集・二画面group/rule競合の3件を再実行してPASS。証跡と14枚の個別目視結果、残る状態は上記task-local `RESULTS.md` とcontrol行ごとの対応表を正とする。外部URLはloopback上の別HTTP listenerの操作であり、実LAN公開やHTTPS/proxyの合格を意味しない。

その後の追補は固定Gateway適用・履歴本文keyboardに加え、固定5方向（送信元の未選択戻し、拒否→許可保存、特定公開対象→すべて選択、端末照合ID／恒久失効detailsの閉じる→poll→再展開）も実GUIでPASSした。Hub taskの個別目視は計27画像となった。100 control familiesの基本入口と固定値・開閉の代表操作は確認済みであり、動的全組合せ・各controlの全keyboard・実時間期限切れ等を網羅した意味ではない。詳細は同taskの `fixed-directions-20260912` と `RESULTS.md` を参照する。

Desktopとの結合は [Desktop E2E](../../../moyAI/tests/desktop_e2e/README.md) の `hub.browser-enrollment`（native config import→Hub承認→Main/Side独立選択）と `mcp.receiver-live`（実Desktop受付→実行→Hubから同じ実履歴・結果を取得）で補う。後者の送信端末とproviderはprotocol fixtureであり、指示側DesktopのGUI、物理WinA/WinB、実LLMの品質は別の受入である。上表のHub単独シナリオの範囲と混ぜない。

- `management-workflows.spec.mjs`、`device-management.spec.mjs`、`keyboard-recovery.spec.mjs`の操作は実ブラウザーから実Rust管理APIへ到達する。独立した`hub.command`は読取確認専用。シナリオを追加しただけではGUI合格とはしない。
- provider fixtureはモデルのメタデータだけを返す。ツール呼出や生成の品質・応答時間の証拠にはならない。
- participant fixtureは実CSR署名・CA検証・証明書取得・mTLSを通すが、Desktopの設定画面・MCP実行・実際の履歴生成は操作していない。履歴本文は明示的なfixture。
- screenshot添付だけでは目視済みとはしない。各実行のRESULTSに目視した画像名と判定範囲を書く。DOMの横幅assertionも、全viewportで見た目が良いことの証明ではない。
- 通常の成功時にもtraceを残すには`--trace on`を指定する。fixtureは終了時のHub・取得済み全listenerの停止を検査し、後始末が失敗したら試験もFAILにする。
