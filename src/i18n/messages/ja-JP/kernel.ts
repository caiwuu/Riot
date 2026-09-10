import type { Dict } from "..";
import type zh from "../zh-CN/kernel";

export default {
  // ── RPC とセッション ────────────────────────────────────────
  "kernel.rpc.badRequest": "カーネルが解析できないリクエストを受け取りました。",
  "kernel.rpc.unimplemented": "メソッド {method} はカーネルにまだ実装されていません。",
  "kernel.session.notFound": "セッション {id} は存在しません。",
  "kernel.turn.busy": "ターンが実行中です。終わるまで待ってから操作してください。",
  "kernel.turn.panicked": "内部エラーにより、このターンは完了しませんでした。再試行するか、別の言い方で試してください。",
  "kernel.turn.toolBatchLost": "ツールバッチが結果を返さなかったため、このターンは中止されました。",

  // ── コンテキスト編集 / 再生成 ───────────────────────────────
  "kernel.history.emptyText": "内容を空にはできません。このメッセージを取り除きたい場合は削除してください。",
  "kernel.history.notInContext": "このメッセージはもう現在のコンテキストに含まれていません。",
  "kernel.history.compacted": "このメッセージは要約に圧縮されており、モデルが見ているのは要約です —— 編集してもコンテキストには影響しません。",
  "kernel.history.noPromptBefore": "この返答の前にユーザーメッセージが見つからないため、再生成できません。",
  "kernel.history.resendNotPrompt": "再送信できるのはユーザーメッセージだけです。返答を変えるには「編集」を使ってください。",
  "kernel.history.noText": "このメッセージには編集できるテキストがありません。",
  "kernel.history.systemNoText": "これはシステム通知のため、編集できるテキストがありません。",
  "kernel.history.systemNoDelete": "これはシステム通知のため、削除できません。",
  "kernel.history.noCheckpoint": "この質問にはファイルのチェックポイントがないため、会話から戻せません。",
  "kernel.history.noRedo": "やり直せる回退はありません。再送信や会話の続きをすると復元できなくなります。",

  // ── 圧縮 ────────────────────────────────────────────────────
  "kernel.compact.empty": "まだ会話がないため、圧縮するものがありません。",
  "kernel.compact.failed": "圧縮に失敗しました。履歴はそのまま残っています。しばらくしてからやり直してください。",

  // ── プロバイダー（モデル呼び出し）───────────────────────────
  "kernel.provider.missingKey": "API key がありません。設定でこのプロバイダーに入力してください。",
  "kernel.provider.httpClient": "HTTP クライアントの初期化に失敗しました。",
  "kernel.provider.auth": "プロバイダーが API key を拒否しました。設定で確認してください。",
  "kernel.provider.rateLimited": "プロバイダーによりレート制限されています。しばらく待ってから送信してください。",
  "kernel.provider.overloaded": "プロバイダーが過負荷状態で、複数回再試行しても失敗しました。しばらくしてからやり直してください。",
  "kernel.provider.backgroundOverloaded": "プロバイダーが過負荷状態のため、このバックグラウンドリクエストはスキップされました。",
  "kernel.provider.retriesExhausted": "複数回再試行しても失敗しました。",
  "kernel.provider.quota": "プロバイダーアカウントの残高が不足しています。",
  "kernel.provider.modelNotFound": "プロバイダーがこのモデルを認識できません。設定のモデル名を確認してください。",
  "kernel.provider.refused": "プロバイダーがリクエストを拒否しました（HTTP {status}）。",
  "kernel.provider.htmlResponse":
    "プロバイダーが API 応答ではなく Web ページを返しました（HTTP {status}）。ゲートウェイやファイアウォールに遮られたか、API の URL が間違っている可能性があります。",
  "kernel.provider.refusedInStream": "プロバイダーが応答の途中でエラーを返しました。",
  "kernel.provider.transport": "プロバイダーに接続できません —— ネットワーク、プロキシ、base URL を確認してください。",
  "kernel.provider.unreachable": "複数回試してもプロバイダーに接続できません —— ネットワーク、プロキシ、base URL を確認してください。",
  "kernel.provider.timeout": "リクエストがタイムアウトしました。ネットワークまたはプロバイダーが時間内に応答しませんでした。少し待って再試行すれば通常は解決します。",
  "kernel.provider.streamBroken": "プロバイダーの応答の読み取りが途中で中断されました。",
  "kernel.provider.idle": "プロバイダーから {secs} 秒間データが届かなかったため、このリクエストを終了しました。通常は中間のプロキシやゲートウェイの問題です。",
  "kernel.provider.contextOverflow": "コンテキストが上限を超えました：使用 {used}、上限 {limit}。",
  "kernel.provider.outputLimit": "出力 token を使い切りました。",
  "kernel.provider.outputLimitExhausted": "出力 token を {count} 回連続で使い切りました。タスクに必要な出力量がモデルの能力を超えています。",
  "kernel.provider.mediaTooLarge": "添付ファイルが大きすぎます（{bytes} バイト）。",

  // ── Hooks ───────────────────────────────────────────────────
  "kernel.hook.promptBlocked": "メッセージが UserPromptSubmit hook によってブロックされました：{reason}",
  "kernel.hook.stopBlocked": "Stop hook が続行を要求しました：{reason}",
  "kernel.hook.badShape": "hooks.json の構造が正しくありません。",

  // ── MCP ─────────────────────────────────────────────────────
  "kernel.mcp.notRunning": "「{id}」という MCP サーバーは実行されていません。まず設定で有効にしてください。",
  "kernel.mcp.commandNotFound":
    "起動に失敗しました：コマンド「{command}」が見つかりません。Finder や Dock から起動した場合はターミナルの PATH が引き継がれないため、コマンドを `which {command}` が返す絶対パスに変えるか、npx / uvx / node がインストールされているか確認してください。",
  "kernel.mcp.spawnFailed": "起動に失敗しました。コマンドのパスと引数を確認してください。",
  "kernel.mcp.disconnected": "プロセスが終了したか、接続が切れました。「再接続」をクリックしてやり直してください。",
  "kernel.mcp.stopped": "停止しました。",
  "kernel.mcp.closed": "接続が切断されました（サーバープロセスが終了した可能性があります）。",
  "kernel.mcp.timeout": "{method} は {secs} 秒待っても応答がありませんでした。",
  "kernel.mcp.serverError": "サーバーがエラーを返しました（{code}）。",
  "kernel.mcp.cancelled": "キャンセルしました。",
  "kernel.mcp.badResponse": "サーバーの応答が予期した形式ではありません。",

  // ── スケジュールタスク ──────────────────────────────────────
  "kernel.schedule.unavailable": "この環境にはスケジューラーが接続されていないため、スケジュールタスクを作成できません。",
  "kernel.schedule.hostUnavailable": "ホストのスケジューラーに接続できません。",
  "kernel.schedule.hostRejected": "ホストがこのスケジュール操作を受け付けませんでした。",

  // ── サブエージェント（Task）────────────────────────────────
  "kernel.task.activity.started": "起動",
  "kernel.task.activity.tool": "→ {name}",
  "kernel.task.activity.said": "{text}",
  "kernel.task.activity.completed": "完了",
  "kernel.task.activity.failed": "失敗",
  "kernel.task.activity.cancelled": "停止",
  "kernel.task.activity.interrupted": "Riot の再起動で中断",
  "kernel.task.started": "[{kind}·{model}] {title} を起動",
  "kernel.task.startedBackground": "[{kind}·{model}] {title} をバックグラウンドで起動",
  "kernel.task.launched": "{title} をバックグラウンドで起動しました（{id}）",
  "kernel.task.completed": "{title} 完了 · {model} · {tokens} tokens · ツール呼び出し {count} 回",

  // ── スキルとコマンド（設定）────────────────────────────────
  "kernel.skill.noFrontmatter": "frontmatter がありません：ファイルは --- で始まり、少なくとも description の行を含む必要があります。",
  "kernel.skill.unterminatedFrontmatter": "frontmatter を閉じる --- がありません。",
  "kernel.skill.noDescription": "description がありません —— モデルが読み込むかどうかを判断する唯一の手がかりです。",
  "kernel.skill.emptyBody": "本文が空です：frontmatter の後にこのスキルの具体的な手順を書いてください。",
  "kernel.slash.compact": "会話履歴を要約に圧縮してコンテキストウィンドウを空ける",

  // ── 設定 ────────────────────────────────────────────────────
  "kernel.config.unreadable": "ファイルを読み込めません。",
  "kernel.config.badJson": "有効な JSON ではありません。",
  "kernel.config.encode": "設定のシリアライズに失敗しました。",
  "kernel.config.mcpImportShape": '形式が正しくありません。期待する形式：{"mcpServers": {"名前": {"command": …}}}',
  "kernel.config.mcpImportEmpty": "サーバーが 1 つも含まれていません。",
  "kernel.config.mcpImportUnnamed": '形式が正しくありません。各サーバーには名前が必要です：{"mcpServers": {"名前": {"command": …}}}',
  "kernel.config.mcpServerBad": "「{name}」を解析できませんでした。",
  "kernel.config.mcpRemoteUnsupported": "「{name}」は http/sse のリモートサーバーです。Riot は現在 stdio（command + args）のみ対応しています。",
  "kernel.config.mcpMissingCommand": "「{name}」に command がありません。",
  "kernel.config.mcpIdClash": "「{name}」の id は正規化後に別のサーバーと重複します（{id}）。名前を変更してください。",
  "kernel.config.mcpIdEmpty": "MCP サーバーの id を空にはできません。",
  "kernel.config.mcpIdInvalid": "MCP サーバー id「{id}」に使えるのは英数字、- 、_ だけです（ツール名の一部になります）。",
  "kernel.config.mcpIdDuplicate": "MCP サーバー id「{id}」が重複しています。",
  "kernel.config.promptIdEmpty": "プロンプトの id を空にはできません。",
  "kernel.config.promptIdDuplicate": "プロンプト id「{id}」が重複しています。",
  "kernel.config.noProvider": "プロバイダーがまだ設定されていません。設定で追加してください。",
  "kernel.config.providerNotFound": "プロバイダー「{id}」が見つかりません。",
  "kernel.config.noModelSelected":
    "「{name}」でモデルが選択されていません。設定でモデルを追加して選択するか、入力欄のモデルメニューから選んでください。",
} satisfies Dict<typeof zh>;
