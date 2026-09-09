/**
 * 内核（crates/）发来的错误与状态文本。键由 Rust 里的
 * `ui_error!("kernel.…")` / `ui_text!("kernel.…")` 决定。
 * `{detail}` 不在模板里 —— 技术细节（服务方原话、HTTP 正文）由前端统一追加。
 */
export default {
  // ── RPC 与会话 ──────────────────────────────────────────────
  "kernel.rpc.badRequest": "内核收到了无法解析的请求。",
  "kernel.rpc.unimplemented": "方法 {method} 尚未在内核实现。",
  "kernel.session.notFound": "会话 {id} 不存在。",
  "kernel.turn.busy": "正在跑一轮，等它结束再操作。",
  "kernel.turn.panicked": "内部错误，这一轮没有完成。可以重试，或者换一种说法。",
  "kernel.turn.toolBatchLost": "工具批次没有返回结果，这一轮已中止。",

  // ── 上下文编辑 / 重新生成 ──────────────────────────────────
  "kernel.history.emptyText": "内容不能为空。想去掉这条消息的话，用删除。",
  "kernel.history.notInContext": "这条消息已经不在当前上下文里。",
  "kernel.history.compacted": "这条消息已被压缩进摘要，模型看的是摘要 —— 改它不会影响上下文。",
  "kernel.history.noPromptBefore": "找不到这条回复前面的用户消息，没法重新生成。",
  "kernel.history.resendNotPrompt": "只能从用户消息重新发送。改回复的话用「编辑」。",
  "kernel.history.noText": "这条消息没有可编辑的文本。",
  "kernel.history.systemNoText": "这条是系统提示，没有可编辑的文本。",
  "kernel.history.systemNoDelete": "这条是系统提示，不支持删除。",

  // ── 压缩 ────────────────────────────────────────────────────
  "kernel.compact.empty": "还没有对话内容，没什么可压缩的。",
  "kernel.compact.failed": "压缩失败，历史保持原样。稍后再试。",

  // ── 服务方（模型调用）────────────────────────────────────────
  "kernel.provider.missingKey": "缺少 API key。去设置里给这个服务方填上。",
  "kernel.provider.httpClient": "HTTP 客户端初始化失败。",
  "kernel.provider.auth": "服务方拒绝了 API key，去设置里检查一下。",
  "kernel.provider.rateLimited": "服务方限流了，稍等一会儿再发。",
  "kernel.provider.overloaded": "服务方过载，多次重试后仍然失败。稍等再试。",
  "kernel.provider.backgroundOverloaded": "服务方过载，这个后台请求已跳过。",
  "kernel.provider.retriesExhausted": "多次重试后仍然失败。",
  "kernel.provider.quota": "服务方账户额度不足。",
  "kernel.provider.modelNotFound": "服务方不认识这个模型，检查设置里的模型名。",
  "kernel.provider.refused": "服务方拒绝了请求（HTTP {status}）。",
  "kernel.provider.htmlResponse":
    "服务方返回的是网页而不是接口响应（HTTP {status}）。多半是被网关或防火墙拦下了，也可能是 API 地址填错了。",
  "kernel.provider.refusedInStream": "服务方在响应中途报了错。",
  "kernel.provider.transport": "连不上服务方 —— 检查网络、代理或 base URL。",
  "kernel.provider.unreachable": "多次尝试后仍然连不上服务方 —— 检查网络、代理或 base URL。",
  "kernel.provider.timeout": "请求超时了，网络或服务方没有按时响应。稍等重试一般就好。",
  "kernel.provider.streamBroken": "读取服务方响应时中断了。",
  "kernel.provider.idle": "服务方 {secs} 秒没有发任何数据，已结束本次请求。通常是中间代理或网关的问题。",
  "kernel.provider.contextOverflow": "上下文溢出：用了 {used}，上限 {limit}。",
  "kernel.provider.outputLimit": "输出 token 耗尽。",
  "kernel.provider.outputLimitExhausted": "输出 token 连续 {count} 次耗尽，任务需要的输出超出模型能力。",
  "kernel.provider.mediaTooLarge": "附件过大（{bytes} 字节）。",

  // ── Hooks ───────────────────────────────────────────────────
  "kernel.hook.promptBlocked": "消息被 UserPromptSubmit hook 拦下：{reason}",
  "kernel.hook.stopBlocked": "Stop hook 要求继续：{reason}",
  "kernel.hook.badShape": "hooks.json 的结构不对。",

  // ── MCP ─────────────────────────────────────────────────────
  "kernel.mcp.notRunning": "没有叫「{id}」的 MCP 服务器在运行。先在设置里启用它。",
  "kernel.mcp.commandNotFound":
    "启动失败：找不到命令「{command}」。从访达或 Dock 打开时没有终端里的 PATH，把命令改成 `which {command}` 给出的绝对路径，或确认 npx / uvx / node 已安装。",
  "kernel.mcp.spawnFailed": "启动失败。检查命令路径和参数。",
  "kernel.mcp.disconnected": "进程退出或连接断开。点「重连」再试。",
  "kernel.mcp.stopped": "已停止。",
  "kernel.mcp.closed": "连接已断开（服务器进程可能退出了）。",
  "kernel.mcp.timeout": "{method} 等了 {secs} 秒没有响应。",
  "kernel.mcp.serverError": "服务器报错（{code}）。",
  "kernel.mcp.cancelled": "已取消。",
  "kernel.mcp.badResponse": "服务器的响应不是预期的形状。",

  // ── 定时任务 ────────────────────────────────────────────────
  "kernel.schedule.unavailable": "这个环境没有接入定时任务调度器，创建不了定时任务。",
  "kernel.schedule.hostUnavailable": "联系不上宿主的调度器。",
  "kernel.schedule.hostRejected": "宿主没有接受这次调度操作。",

  // ── 子代理（Task）────────────────────────────────────────────
  "kernel.task.activity.started": "启动",
  "kernel.task.activity.tool": "→ {name}",
  "kernel.task.activity.said": "{text}",
  "kernel.task.activity.completed": "完成",
  "kernel.task.activity.failed": "失败",
  "kernel.task.activity.cancelled": "已停止",
  "kernel.task.activity.interrupted": "Riot 重启，已中断",
  "kernel.task.started": "[{kind}·{model}] {title} 启动",
  "kernel.task.startedBackground": "[{kind}·{model}] {title} 后台启动",
  "kernel.task.launched": "{title} 已在后台启动（{id}）",
  "kernel.task.completed": "{title} 完成 · {model} · {tokens} tokens · {count} 次工具调用",

  // ── 技能与命令（设置页）──────────────────────────────────────
  "kernel.skill.noFrontmatter": "缺 frontmatter：文件要以 --- 开头，里面至少写一行 description。",
  "kernel.skill.unterminatedFrontmatter": "frontmatter 没有结束的 ---。",
  "kernel.skill.noDescription": "缺 description —— 它是模型决定要不要加载的唯一依据。",
  "kernel.skill.emptyBody": "正文是空的：frontmatter 之后要写这个技能的具体做法。",
  "kernel.slash.compact": "把对话历史压缩成摘要，腾出上下文窗口",

  // ── 配置（设置页）────────────────────────────────────────────
  "kernel.config.unreadable": "文件读不出来。",
  "kernel.config.badJson": "不是合法的 JSON。",
  "kernel.config.encode": "配置序列化失败。",
  "kernel.config.mcpImportShape": '形状不对。期待 {"mcpServers": {"名字": {"command": …}}}',
  "kernel.config.mcpImportEmpty": "里面一个服务器都没有。",
  "kernel.config.mcpImportUnnamed": '形状不对。每个服务器要有名字：{"mcpServers": {"名字": {"command": …}}}',
  "kernel.config.mcpServerBad": "「{name}」解析失败。",
  "kernel.config.mcpRemoteUnsupported": "「{name}」是 http/sse 远程服务器，Riot 暂时只支持 stdio（command + args）。",
  "kernel.config.mcpMissingCommand": "「{name}」缺 command。",
  "kernel.config.mcpIdClash": "「{name}」和另一个服务器的 id 消毒后撞名了（{id}），改一下名字。",
  "kernel.config.mcpIdEmpty": "MCP 服务器的 id 不能为空。",
  "kernel.config.mcpIdInvalid": "MCP 服务器 id「{id}」只能用字母、数字、- 和 _（它要进工具名）。",
  "kernel.config.mcpIdDuplicate": "MCP 服务器 id「{id}」重复了。",
  "kernel.config.promptIdEmpty": "提示词的 id 不能为空。",
  "kernel.config.promptIdDuplicate": "提示词 id「{id}」重复了。",
  "kernel.config.noProvider": "还没有配置服务方，去设置里添加一个。",
  "kernel.config.providerNotFound": "找不到服务方「{id}」。",
  "kernel.config.noModelSelected":
    "「{name}」还没有选中模型。在设置里添加一个模型并点选，或在输入框的模型菜单里选择。",
};
