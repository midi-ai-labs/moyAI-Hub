# Hub モデル経路を使う子エージェント

Desktop のローカル子エージェントは、Main / Subで確認済みのモデル選択を受け取り、自分の実行 ID に結び付いた独立した Hub route context を使います。Approveの判定も、確認済みApprove選択を使う短い独立scopeを持ちます。これは Desktop 間の MCP 委任の資格や job 台帳を置き換えるものではありません。子は同じ登録 model client の資格を使い、provider の raw endpoint や認証情報を取得しません。

登録と network model-session が `supports_delegated_execution:true` を返す場合だけ使用します。省略する旧 Hub では Desktop が非対応を明示し、未知フィールドを省いて通常 Main に送ったり Direct に迂回したりしません。

## 要求と選択の固定

子の prepare は既存 `id`、`context`、`turn_id`、`request_id`、`expected_hub_id`、`reviewed_revision` に、`purpose:"delegated"` と `delegated_selection` を追加します。`context` は確認元の `main` / `side_chat` / `approve`、`turn_id` は子の canonical uppercase ULID です。Approveの短い判定要求も同じ独立scopeを使います。`delegated_selection` は review API と同じ JSON で、revision は既存の top-level 値を使います。Delegated 以外の purpose にこの selection を渡した要求、Delegated で省略した要求は拒否します。

最初の prepare では認証・catalog revision に加え、渡された選択が現在の親 review と完全に一致することを確認します。成功すると同じ `RouteContext` owner に immutable selection を持つ内部 context を作ります。以後の prepare はその snapshot に一致する必要があり、別モデル選択への差し替え、Main/Sub/Approveの持ち替え、別の execution ID への付け替えは拒否します。親の選択を後で保存し直しても、既存の子はその新しい選択へ自動追従しません。catalog revision が変われば、既存の子の新規要求も review gate で停止します。

内部 context は通常の review API で作成できず、Main/Sub/Approveの確認一覧にも表示しません。生成の prepare は同じ FIFO、capacity pool、Gateway permit、claim、取消の実装を使います。子は自分の lease を持ち、親の affinity 回数を消費しません。子の要求も provider の実行枠を予約するものではなく、Gateway の中継上限に従います。

## 終了・取消・heartbeat

finish/cancel は既存 `{id,context,turn_id}` に `delegated:true` を付けます。省略時は通常のMain/Sub/Approve context操作です。子を閉じる操作は exact 内部 context の queue / permit を取り除き、その Gateway 通信だけを閉じます。親の finish/cancel は子や兄弟の scope を暗黙に閉じません。親からの一括取消の対象と時期は Desktop の実行 owner が管理し、各子の取消を送ります。

最初の prepare より先に取消が届いた場合も、当該 client / 親 / execution ID を終了済みとして保持します。遅れて到着した prepare は子を復活させません。同じ close は同じ親なら冪等です。終了済み ID や別の親に属する ID で子を再作成することはできません。

heartbeat の `active_turns` item にも `delegated:true` を付け、同じ親 context と子 turn ID を渡します。サーバーは既存の有効な子 lease だけを延長します。heartbeat で子 context を作成したり、終了済みの子や permit の期限を復活させたりしません。親の heartbeat と複数の子の heartbeat は、同じ model client の既存通信にまとめます。切断時はすべての context / queue / permit を既存 client owner と一緒に破棄します。

終了通知だけが失敗した場合も、子の枠を残し続けません。同じ RouteContext にある子presenceの最終時刻は、その子の有効な prepare または `active_turns` で明示した heartbeat だけで更新します。通常の client heartbeat は子の生存証拠になりません。`heartbeat_stale_ms` を越えた子は次の heartbeat / prepare で期限退役し、core の待ち順序とadvisory permitを除去して終了済み ID に移します。期限後の明示heartbeatや遅いprepareでは復活しません。既に開始済みの Gateway HTTP通信や実際の中継枠はこの期限で終了扱いにせず、Gateway自身がEOF・通信切断・自身の予約期限を処理します。provider生成の終了確認や再送は行いません。

同時に保持できる通常の子 context は1 clientにつきMain/Sub合計14件のままです。Approveの短い判定scopeは別に最大15件で、通常の3用途を含む全体は32 contextまでです。件数は同じcontext mapから数え、待ち順序と中継容量は全用途で共有します。終了済み ID は client の寿命中4096件まで保持します。active と終了済みの合計が4096件に達した場合、新しい実行を `registry_full` で拒否します。古い終了記録を追い出して同じ ID を再受付する方式にはしません。clientの切断・再登録やHub再起動では、旧client資格と共にこのruntime stateも終了します。

Source は [core/delegated.rs](../src/core/delegated.rs)、[execution_api.rs](../src/service/execution_api.rs)、[service.rs](../src/service.rs)。focused regression は [core/delegated/tests.rs](../src/core/delegated/tests.rs) と [gateway_execution/delegated.rs](../tests/gateway_execution/delegated.rs) に置きます。物理端末間の MCP や実モデルによる子エージェント受入とは確認範囲を分けます。
