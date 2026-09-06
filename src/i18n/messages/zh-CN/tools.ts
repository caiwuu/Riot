/**
 * 工具给用户看的那句话：权限弹窗的标题（`tools.ask.*`、`tools.safety.*`、
 * `tools.bash.complex.*`）、弹窗预览和工具卡上的一句话描述（`Tool::describe`，
 * 按工具分组）、工具结果卡上的短句（`UiPayload::Message`）。
 *
 * 键由 Rust 里的 `ui_text!("tools.…")` 决定（crates/riot-tools、riot-permissions、
 * riot-kernel/gate.rs、riot-mcp）。占位符对应宏里的 `name = …` 参数。
 */
export default {
  // ── 直通 ─────────────────────────────────────────────
  // 模型自己写的原话（Bash 的 description），不翻译。
  "tools.raw": "{text}",

  // ── 权限弹窗标题 ─────────────────────────────────────
  "tools.ask.useTool": "是否允许使用 `{name}`？",
  "tools.ask.thisCall": "是否允许 `{name}` 执行这次调用？",
  "tools.ask.runCommand": "是否允许运行 `{command}`？",
  "tools.ask.confirm": "需要确认这次调用",
  "tools.ask.hook": "PreToolUse hook 要求确认：{reason}",
  "tools.ask.decision": "模型想让你做一个决定",
  "tools.ask.planReady": "计划已就绪，批准后退出规划模式开始执行",
  "tools.ask.webSearch": "是否允许联网搜索？搜索词会发给搜索后端。",
  "tools.ask.fetch": "是否允许抓取 {host}？",
  "tools.ask.openUrl": "是否允许在浏览器里打开 {host}？",
  "tools.ask.openLocalFile": "是否允许在浏览器里打开本地文件 {path}？",
  "tools.ask.sandboxEscape":
    "`{command}` 会在 OS 沙箱**之外**执行。\n\n沙箱的文件系统边界对它不生效 —— 它能写工作区以外的任何地方。",
  "tools.ask.mcp": "是否允许调用外部 MCP 服务器「{server}」的 {name}？它跑在本机的独立进程里，能做什么由那个服务器决定。",
  "tools.ask.browser.interact": "是否允许模型操作当前页面（点击、输入、按键）？",
  "tools.ask.browser.evaluate": "是否允许在当前页面执行脚本？",
  "tools.ask.browser.upload": "是否允许把本地文件上传到当前页面？",
  "tools.ask.browser.cookies": "是否允许读取当前页面的 Cookie？",
  "tools.ask.browser.handoff": "请在浏览器面板里完成：{prompt}\n做完后点「允许」继续。",
  "tools.ask.browser.handoffAny": "请在浏览器面板里完成一步需要你本人操作的动作。\n做完后点「允许」继续。",
  "tools.ask.pentest.host": "是否对 {host} 执行这次渗透动作？",
  "tools.ask.pentest.outOfScope":
    "目标 {host} 不在本次会话的渗透授权范围内。只在你有权测试的目标上继续 —— 是否授权对 {host} 进行渗透测试？",
  // AskUserQuestion 的描述与结果卡。
  "tools.ask.question": "提问：{question}",
  "tools.ask.questionAny": "请用户做决定",
  "tools.ask.answered": "{question} → {answer}",

  // ── 安全检查（弹窗标题上那句「为什么拦你」）──────────
  "tools.safety.gitInternals": "这会修改 Git 内部文件 {path}。写 .git/hooks/ 等于让下次提交自动执行代码。",
  "tools.safety.sshConfig": "这会读写 SSH 配置或密钥 {path}。",
  "tools.safety.shellRc":
    "这会修改自动执行的配置 {path}。改这个等于取得持久化执行权 —— 下次开终端、下次登录或下次敲那个命令时就会运行。",
  "tools.safety.agentConfig": "这会修改本应用自己的配置 {path}，可能影响后续的权限判断。",
  "tools.safety.toolchainConfig":
    "这会修改构建工具链的配置或可执行文件 {path}。改这个等于取得持久化执行权 —— 下次构建就会运行，而那次构建不在沙箱里。",
  "tools.safety.credentials": "{path} 看起来是凭证文件。",
  "tools.safety.commandInjection": "命令里检测到注入模式：{path}",
  "tools.safety.unparseableCommand": "无法解析这个命令：{path}",
  "tools.safety.outOfScope": "目标 {path} 不在授权的渗透范围内。",
  "tools.safety.sandboxEscape": "{path} 会在 OS 沙箱之外执行，文件系统边界对它不生效。",
  "tools.safety.gitExecConfig":
    "这会设置 Git 配置 `{key}`。这一类键的值会被 Git 当成命令执行 —— 设了它等于让之后的 git 操作自动跑指定的程序。",

  // ── Bash 命令分析看不懂 / 看出危险时的弹窗标题 ────────
  // {detail} 是触发的片段（命令子串、变量名、数字），不翻译。
  "tools.bash.complex.commandSubstitution": "含命令替换，执行内容运行时才确定。\n\n{detail}",
  "tools.bash.complex.processSubstitution": "含进程替换（`<(...)`）。\n\n{detail}",
  "tools.bash.complex.expansion": "含变量展开，结果取决于当前环境。\n\n{detail}",
  "tools.bash.complex.background": "会在后台运行（`&`）。\n\n{detail}",
  "tools.bash.complex.redirect": "会重定向写入文件。\n\n{detail}",
  "tools.bash.complex.sensitiveRedirect": "会写入 shell 启动脚本、密钥或凭证。\n\n{detail}",
  "tools.bash.complex.controlFlow": "含子 shell、循环或条件结构。\n\n{detail}",
  "tools.bash.complex.dynamicExecution": "会执行运行时才确定的内容（`eval` / `source`）。\n\n`{detail}`",
  "tools.bash.complex.dangerousAssignment": "设置了改变动态链接或命令查找的环境变量。\n\n`{detail}`",
  "tools.bash.complex.parseError": "无法解析这条命令。\n\n{detail}",
  "tools.bash.complex.tooManyCommands": "子命令太多，无法逐条审查。\n\n{detail}",
  "tools.bash.complex.tooLong": "命令过长。\n\n{detail}",
  "tools.bash.complex.nestedWrappers": "包装嵌套过深。\n\n`{detail}`",
  "tools.bash.complex.unknownNode": "含无法识别的 shell 结构。\n\n{detail}",

  // ── 工具描述：文件与搜索 ─────────────────────────────
  "tools.bash.run": "运行 {command}",
  "tools.bash.runAny": "执行命令",
  "tools.read.file": "读取 {path}",
  "tools.read.any": "读取文件",
  "tools.read.image": "图片（{media}，{kb} KB）",
  "tools.edit.file": "修改 {path}",
  "tools.edit.any": "修改文件",
  "tools.write.file": "写入 {path}",
  "tools.write.any": "写入文件",
  "tools.glob.in": "在 {path} 里查找 {pattern}",
  "tools.glob.find": "查找 {pattern}",
  "tools.grep.in": "在 {glob} 里搜索 {pattern}",
  "tools.grep.search": "搜索 {pattern}",
  "tools.preview.file": "预览 {path}",
  "tools.preview.any": "预览文件",
  "tools.preview.showBrowser": "打开浏览器面板给用户看",
  "tools.diagnostics.path": "检查诊断（{path}）",
  "tools.diagnostics.all": "检查诊断",

  // ── 工具描述：网络 ───────────────────────────────────
  "tools.webSearch.query": "搜索 {query}",
  "tools.webSearch.any": "联网搜索",
  "tools.webFetch.host": "抓取 {host}",
  "tools.webFetch.any": "抓取网页",

  // ── 工具描述：任务与计划 ─────────────────────────────
  "tools.todo.update": "更新任务清单（{count} 项）",
  "tools.todo.progress": "{done}/{total} 完成",
  "tools.todo.progressActive": "{done}/{total} 完成 · {active}",
  "tools.plan.submit": "提交计划等待批准",
  "tools.plan.approved": "计划已批准，开始执行",
  "tools.plan.empty": "（计划为空 —— 这不该发生，拒绝并让模型重新提交）",
  "tools.skill.load": "加载技能 {name}",
  "tools.skill.loaded": "已加载技能「{name}」（{count} 字符）",
  "tools.toolSearch.query": "查找工具：{query}",
  "tools.toolSearch.loaded": "已加载 {count} 个工具：{names}",
  "tools.schedule.create": "创建定时任务「{name}」",
  "tools.schedule.createAny": "创建定时任务",
  "tools.schedule.list": "查看定时任务",
  "tools.schedule.pause": "暂停定时任务",
  "tools.schedule.resume": "恢复定时任务",
  "tools.schedule.delete": "删除定时任务",
  "tools.schedule.manage": "管理定时任务",

  // ── 工具描述：子 agent ───────────────────────────────
  // {kind} 是 explore / general-purpose 这类标识，不翻译。
  "tools.task.sync": "子 agent（{kind}·同步）：{description}",
  "tools.task.background": "子 agent（{kind}·后台）：{description}",
  "tools.task.fork": "子 agent（分叉·后台）：{description}",
  "tools.task.resume": "子 agent（续接 {id}·同步）：{description}",
  "tools.task.resumeBackground": "子 agent（续接 {id}·后台）：{description}",

  // ── 工具描述：终端面板 ───────────────────────────────
  "tools.terminal.read": "读终端 {id} 的输出",
  "tools.terminal.readAny": "读服务输出",
  "tools.terminal.list": "列出可见的终端",
  "tools.terminal.kill": "停掉终端 {id}",
  "tools.terminal.killAny": "停掉服务",

  // ── 工具描述：MCP ────────────────────────────────────
  "tools.mcp.call": "调用 {server} 的 {name}",
  "tools.mcp.callWith": "调用 {server} 的 {name}：{arg}",

  // ── 工具描述：浏览器 ─────────────────────────────────
  // {target} 是定位记号：`[3]`（编号）、`` `css` ``（选择器）或 “文本”。
  "tools.browser.open": "在浏览器里打开 {url}",
  "tools.browser.snapshot": "读当前页面的结构",
  "tools.browser.screenshot": "给当前页面截图",
  "tools.browser.screenshotFrozen": "给当前页面截图（冻结动画）",
  "tools.browser.view": "看当前视口（带编号框）",
  "tools.browser.console": "读当前页面的 console",
  "tools.browser.perf": "测量页面性能指标",
  "tools.browser.sourceOf": "查 {target} 的源码",
  "tools.browser.source": "查元素对应的源码",
  "tools.browser.tabSnapshot": "读标签页 [{tab}] 的结构",
  "tools.browser.tabSnapshotAny": "读另一个标签页",
  "tools.browser.har": "导出网络请求为 HAR",
  "tools.browser.click": "点击 {target}",
  "tools.browser.clickAny": "点击页面元素",
  "tools.browser.doubleClick": "双击 {target}",
  "tools.browser.doubleClickAny": "双击页面元素",
  "tools.browser.rightClick": "右键 {target}",
  "tools.browser.rightClickAny": "右键页面元素",
  "tools.browser.typeInto": "在 {target} 输入 “{text}”",
  "tools.browser.type": "在页面里输入 “{text}”",
  "tools.browser.fill": "填写 {count} 个字段",
  "tools.browser.fillSubmit": "填写 {count} 个字段并提交",
  "tools.browser.key": "按 {key}",
  "tools.browser.scrollUp": "向上滚动页面 {px}px",
  "tools.browser.scrollDown": "向下滚动页面 {px}px",
  "tools.browser.waitSelector": "等元素 `{selector}` 出现",
  "tools.browser.waitSelectorGone": "等元素 `{selector}` 消失",
  "tools.browser.waitText": "等文本 “{text}”",
  "tools.browser.waitUrl": "等地址包含 “{url}”",
  "tools.browser.waitNetworkIdle": "等网络空闲",
  "tools.browser.wait": "等待页面条件",
  "tools.browser.hover": "悬停到 {target}",
  "tools.browser.hoverAny": "悬停到页面元素",
  "tools.browser.select": "把 {target} 设为 “{value}”",
  "tools.browser.selectAny": "下拉选择 “{value}”",
  "tools.browser.drag": "把 {from} 拖到 {to}",
  "tools.browser.dragAny": "拖拽元素",
  "tools.browser.back": "后退",
  "tools.browser.forward": "前进",
  "tools.browser.reload": "刷新页面",
  "tools.browser.history": "历史导航",
  "tools.browser.tabs.list": "列出标签页",
  "tools.browser.tabs.new": "新开标签页",
  "tools.browser.tabs.select": "切到标签页 [{id}]",
  "tools.browser.tabs.close": "关闭标签页 [{id}]",
  "tools.browser.tabs.any": "标签页操作",
  "tools.browser.evaluate": "执行 JS：{expression}",
  "tools.browser.upload": "上传 {count} 个文件",
  "tools.browser.uploadTo": "给 {target} 上传 {count} 个文件",
  "tools.browser.cookies": "读当前页面的 Cookie",
  "tools.browser.network.list": "列出网络请求",
  "tools.browser.network.detail": "看请求 #{id} 的细节",
  "tools.browser.network.audit": "审计响应头安全配置",
  "tools.browser.replay": "重放 {method} {url}",
  "tools.browser.intercept.block": "拦截含 `{pattern}` 的请求",
  "tools.browser.intercept.fulfill": "伪造 `{pattern}` 的响应",
  "tools.browser.intercept.list": "列出拦截规则",
  "tools.browser.intercept.clear": "清空拦截规则",
  "tools.browser.intercept.any": "拦截设置",
  "tools.browser.secrets": "扫描页面里的密钥泄露",
  "tools.browser.discover": "枚举页面的表单和链接",
  "tools.browser.fuzz": "fuzz {url}",
  "tools.browser.report": "生成渗透报告（{count} 条发现）",
  "tools.browser.crawl": "爬取 {url}",
  "tools.browser.handoff": "请用户操作：{prompt}",
  "tools.browser.handoffAny": "请用户接管操作",
};
