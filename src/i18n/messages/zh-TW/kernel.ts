import type { Dict } from "..";
import type zh from "../zh-CN/kernel";

export default {
  // ── RPC 與會話 ──────────────────────────────────────────────
  "kernel.rpc.badRequest": "核心收到了無法解析的請求。",
  "kernel.rpc.unimplemented": "方法 {method} 尚未在核心實作。",
  "kernel.session.notFound": "會話 {id} 不存在。",
  "kernel.turn.busy": "正在跑一輪，等它結束再操作。",
  "kernel.turn.panicked": "內部錯誤，這一輪沒有完成。可以重試，或者換一種說法。",
  "kernel.turn.toolBatchLost": "工具批次沒有回傳結果，這一輪已中止。",

  // ── 上下文編輯 / 重新產生 ──────────────────────────────────
  "kernel.history.emptyText": "內容不能為空。想去掉這則訊息的話，用刪除。",
  "kernel.history.notInContext": "這則訊息已經不在目前上下文裡。",
  "kernel.history.compacted": "這則訊息已被壓縮進摘要，模型看的是摘要 —— 改它不會影響上下文。",
  "kernel.history.noPromptBefore": "找不到這則回覆前面的使用者訊息，沒辦法重新產生。",
  "kernel.history.resendNotPrompt": "只能從使用者訊息重新送出。改回覆的話用「編輯」。",
  "kernel.history.noText": "這則訊息沒有可編輯的文字。",
  "kernel.history.systemNoText": "這則是系統提示，沒有可編輯的文字。",
  "kernel.history.systemNoDelete": "這則是系統提示，不支援刪除。",
  "kernel.history.noCheckpoint": "這則提問沒有檔案檢查點，沒辦法依對話回退。",
  "kernel.history.noRedo": "沒有可以恢復的回退。重新送出或繼續對話之後就不能再還原了。",

  // ── 壓縮 ────────────────────────────────────────────────────
  "kernel.compact.empty": "還沒有對話內容，沒什麼可壓縮的。",
  "kernel.compact.failed": "壓縮失敗，歷史保持原樣。稍後再試。",

  // ── 服務商（模型呼叫）────────────────────────────────────────
  "kernel.provider.missingKey": "缺少 API key。去設定裡給這家服務商填上。",
  "kernel.provider.httpClient": "HTTP 客戶端初始化失敗。",
  "kernel.provider.auth": "服務商拒絕了 API key，去設定裡檢查一下。",
  "kernel.provider.rateLimited": "服務商限流了，稍等一會兒再送。",
  "kernel.provider.overloaded": "服務商過載，多次重試後仍然失敗。稍等再試。",
  "kernel.provider.backgroundOverloaded": "服務商過載，這個背景請求已跳過。",
  "kernel.provider.retriesExhausted": "多次重試後仍然失敗。",
  "kernel.provider.quota": "服務商帳戶額度不足。",
  "kernel.provider.modelNotFound": "服務商不認識這個模型，檢查設定裡的模型名稱。",
  "kernel.provider.refused": "服務商拒絕了請求（HTTP {status}）。",
  "kernel.provider.htmlResponse":
    "服務商回傳的是網頁而不是介面回應（HTTP {status}）。多半是被閘道或防火牆擋下了，也可能是 API 位址填錯了。",
  "kernel.provider.refusedInStream": "服務商在回應中途報了錯。",
  "kernel.provider.transport": "連不上服務商 —— 檢查網路、代理或 base URL。",
  "kernel.provider.unreachable": "多次嘗試後仍然連不上服務商 —— 檢查網路、代理或 base URL。",
  "kernel.provider.timeout": "請求逾時了，網路或服務商沒有按時回應。稍等重試一般就好。",
  "kernel.provider.streamBroken": "讀取服務商回應時中斷了。",
  "kernel.provider.idle": "服務商 {secs} 秒沒有送任何資料，已結束本次請求。通常是中間代理或閘道的問題。",
  "kernel.provider.contextOverflow": "上下文溢位：用了 {used}，上限 {limit}。",
  "kernel.provider.outputLimit": "輸出 token 耗盡。",
  "kernel.provider.outputLimitExhausted": "輸出 token 連續 {count} 次耗盡，任務需要的輸出超出模型能力。",
  "kernel.provider.mediaTooLarge": "附件過大（{bytes} 位元組）。",

  // ── Hooks ───────────────────────────────────────────────────
  "kernel.hook.promptBlocked": "訊息被 UserPromptSubmit hook 攔下：{reason}",
  "kernel.hook.stopBlocked": "Stop hook 要求繼續：{reason}",
  "kernel.hook.badShape": "hooks.json 的結構不對。",

  // ── MCP ─────────────────────────────────────────────────────
  "kernel.mcp.notRunning": "沒有叫「{id}」的 MCP 伺服器在執行。先在設定裡啟用它。",
  "kernel.mcp.commandNotFound":
    "啟動失敗：找不到命令「{command}」。從 Finder 或 Dock 開啟時沒有終端機裡的 PATH，把命令改成 `which {command}` 給出的絕對路徑，或確認 npx / uvx / node 已安裝。",
  "kernel.mcp.spawnFailed": "啟動失敗。檢查命令路徑和參數。",
  "kernel.mcp.disconnected": "程序結束或連線中斷。點「重新連線」再試。",
  "kernel.mcp.stopped": "已停止。",
  "kernel.mcp.closed": "連線已中斷（伺服器程序可能結束了）。",
  "kernel.mcp.timeout": "{method} 等了 {secs} 秒沒有回應。",
  "kernel.mcp.serverError": "伺服器報錯（{code}）。",
  "kernel.mcp.cancelled": "已取消。",
  "kernel.mcp.badResponse": "伺服器的回應不是預期的形狀。",

  // ── 排程任務 ────────────────────────────────────────────────
  "kernel.schedule.unavailable": "這個環境沒有接入排程器，建立不了排程任務。",
  "kernel.schedule.hostUnavailable": "聯絡不上主機的排程器。",
  "kernel.schedule.hostRejected": "主機沒有接受這次排程操作。",

  // ── 子代理（Task）────────────────────────────────────────────
  "kernel.task.activity.started": "啟動",
  "kernel.task.activity.tool": "→ {name}",
  "kernel.task.activity.said": "{text}",
  "kernel.task.activity.completed": "完成",
  "kernel.task.activity.failed": "失敗",
  "kernel.task.activity.cancelled": "已停止",
  "kernel.task.activity.interrupted": "Riot 重啟，已中斷",
  "kernel.task.started": "[{kind}·{model}] {title} 啟動",
  "kernel.task.startedBackground": "[{kind}·{model}] {title} 背景啟動",
  "kernel.task.launched": "{title} 已在背景啟動（{id}）",
  "kernel.task.completed": "{title} 完成 · {model} · {tokens} tokens · {count} 次工具呼叫",

  // ── 技能與指令（設定頁）──────────────────────────────────────
  "kernel.skill.noFrontmatter": "缺 frontmatter：檔案要以 --- 開頭，裡面至少寫一行 description。",
  "kernel.skill.unterminatedFrontmatter": "frontmatter 沒有結束的 ---。",
  "kernel.skill.noDescription": "缺 description —— 它是模型決定要不要載入的唯一依據。",
  "kernel.skill.emptyBody": "內文是空的：frontmatter 之後要寫這個技能的具體做法。",
  "kernel.slash.compact": "把對話歷史壓縮成摘要，騰出上下文視窗",

  // ── 設定（設定頁）────────────────────────────────────────────
  "kernel.config.unreadable": "檔案讀不出來。",
  "kernel.config.badJson": "不是合法的 JSON。",
  "kernel.config.encode": "設定序列化失敗。",
  "kernel.config.mcpImportShape": '形狀不對。期待 {"mcpServers": {"名稱": {"command": …}}}',
  "kernel.config.mcpImportEmpty": "裡面一個伺服器都沒有。",
  "kernel.config.mcpImportUnnamed": '形狀不對。每個伺服器要有名稱：{"mcpServers": {"名稱": {"command": …}}}',
  "kernel.config.mcpServerBad": "「{name}」解析失敗。",
  "kernel.config.mcpRemoteUnsupported": "「{name}」是 http/sse 遠端伺服器，Riot 暫時只支援 stdio（command + args）。",
  "kernel.config.mcpMissingCommand": "「{name}」缺 command。",
  "kernel.config.mcpIdClash": "「{name}」和另一個伺服器的 id 清理後撞名了（{id}），改一下名稱。",
  "kernel.config.mcpIdEmpty": "MCP 伺服器的 id 不能為空。",
  "kernel.config.mcpIdInvalid": "MCP 伺服器 id「{id}」只能用字母、數字、- 和 _（它要進工具名稱）。",
  "kernel.config.mcpIdDuplicate": "MCP 伺服器 id「{id}」重複了。",
  "kernel.config.promptIdEmpty": "提示詞的 id 不能為空。",
  "kernel.config.promptIdDuplicate": "提示詞 id「{id}」重複了。",
  "kernel.config.noProvider": "還沒有設定服務商，去設定裡新增一家。",
  "kernel.config.providerNotFound": "找不到服務商「{id}」。",
  "kernel.config.noModelSelected":
    "「{name}」還沒有選定模型。在設定裡新增一個模型並點選，或在輸入框的模型選單裡選擇。",
} satisfies Dict<typeof zh>;
