# Riot 全量代码 Review 清单

> 统计日期:2026-08-25。行数为当日 `wc -l` 实测值,全仓合计约 103k 行(含测试与样式)。
> 共 8 个分组、35 个批次。每批次控制在 1.5k~4.5k 行,预计单批 1~3 小时(按风险等级调整节奏)。

## 使用说明

- **顺序原则**:自底向上沿依赖方向推进(protocol 是叶子 crate,先读它建立词汇表),安全边界(B 组)紧随其后优先审。组间按 A → H 顺序;组内批次无强依赖时可并行分工。
- **风险分级**:
  - 🔴 极高:安全边界/核心枢纽,逐行精读,重点构造反例
  - 🟠 高:核心逻辑,精读
  - 🟡 中:常规逻辑,正常速度
  - 🟢 低:可快速浏览,抽查为主
- **生成物**(`schemas/protocol.json`、`src/bridge/generated.ts`)不逐行读,验证方式是重新运行生成命令后 `git diff` 为空。
- 每完成一批,勾选批次标题旁的复选框,并在下方"总览"表更新状态。
- 对照文档:`docs/ARCHITECTURE.md`(硬性约束标注 `[约束]`)、`docs/VERIFICATION.md`(不变量清单)。审查时发现实现偏离文档约束的,一律记录。

## 横切关注点(每一批都要过一遍)

### Rust 通用
- [ ] `unsafe` 块:是否必要、是否有 SAFETY 注释、边界是否成立
- [ ] 生产路径上的 `unwrap` / `expect` / `panic!` / `todo!`
- [ ] 错误处理遵循"错误是对话内容":工具错误转 `tool_result(is_error)`,主循环签名不出现 `Result`(ARCHITECTURE §5.3)
- [ ] 依赖方向约束:protocol 不依赖任何 workspace crate;tools 不依赖 core;core 不依赖 UI(ARCHITECTURE §3.1)
- [ ] async 中混入阻塞调用(`std::fs`、`std::process`、同步锁跨 await)
- [ ] tokio 任务泄漏;`select!` 分支的取消安全(drop 掉的 future 是否留下半完成状态)
- [ ] core 内禁止直接 `SystemTime::now()` / `tokio::time::sleep`,必须走 `Clock` 注入
- [ ] 解析器类代码的整数溢出、切片越界、无限循环

### 安全专项
- [ ] 所有外部输入视为不可信:模型输出、MCP 服务器返回、网页内容、用户工作区文件
- [ ] 路径处理:规范化后做前缀检查;防 `../` 穿越、符号链接逃逸、macOS 大小写不敏感绕过
- [ ] 命令构造不做字符串拼接;参数一律走 argv 数组
- [ ] 密钥 / token 不进日志、不进事件流、不落盘明文
- [ ] fail-closed:解析失败、未知命令、未知情况一律默认拒绝或问人,不放行
- [ ] 权限拒绝路径有测试覆盖(不只测 allow)

### 前端通用
- [ ] `dangerouslySetInnerHTML` / `innerHTML` 使用点(重点 Markdown / Mermaid 渲染)
- [ ] 打开外部 URL 前的校验
- [ ] 事件监听、Tauri channel 订阅的清理;`useEffect` 依赖数组
- [ ] 展示给用户的权限确认内容与实际执行内容一致(不因截断/转义产生误导)

### 通用
- [ ] `TODO` / `FIXME` / `HACK` 盘点并记录
- [ ] 死代码、未使用的 pub 项

---

## 总览

| 组 | 内容 | 批次 | 行数(约) | 最高风险 | 状态 |
|---|---|---|---|---|---|
| A | 协议与主循环 | 2 | 9.9k | 🟠 | ☐ |
| B | 安全边界(权限/沙箱) | 4 | 10.2k | 🔴 | ☐ |
| C | 工具层 riot-tools | 6 | 19.9k | 🔴 | ☐ |
| D | Provider / MCP / 浏览器进程 | 3 | 10.7k | 🟠 | ☐ |
| E | 内核 riot-kernel + 存储 | 6 | 18.3k | 🔴 | ☐ |
| F | 宿主层 src-tauri | 5 | 14.0k | 🟠 | ☐ |
| G | 前端 renderer | 6 | 22.8k | 🟡 | ☐ |
| H | 网站 / 脚本 / CI / 文档 | 3 | 7.0k | 🟡 | ☐ |

---

## A 组:协议与主循环(打底,先建立词汇表)

### ☐ A1 · riot-protocol 全部 —— 🟡 中 · 约 4.9k 行

所有跨进程类型的唯一定义处,是读懂后面一切的前提。

| 文件 | 行数 |
|---|---|
| `crates/riot-protocol/src/browser.rs` | 658 |
| `crates/riot-protocol/src/tool.rs` | 504 |
| `crates/riot-protocol/src/permission.rs` | 477 |
| `crates/riot-protocol/src/event.rs` | 421 |
| `crates/riot-protocol/src/web.rs` | 397 |
| `crates/riot-protocol/src/message.rs` | 357 |
| `crates/riot-protocol/src/rpc.rs` | 350 |
| `crates/riot-protocol/src/provider.rs` | 300 |
| `crates/riot-protocol/src/bin/gen_schema.rs` | 261 |
| `crates/riot-protocol/src/hostcall.rs` | 199 |
| `crates/riot-protocol/src/turn.rs` | 190 |
| `crates/riot-protocol/src/env.rs` | 139 |
| `crates/riot-protocol/src/id.rs` | 105 |
| `crates/riot-protocol/src/terminal.rs` | 93 |
| `crates/riot-protocol/src/vision.rs` | 89 |
| `crates/riot-protocol/src/changes.rs` | 83 |
| `crates/riot-protocol/src/runner.rs` / `lib.rs` / `hook.rs` / `compact.rs` | 233 |

**审查要点**
- serde 标签策略(`tag` / `rename_all`)全库一致;字段增删是否破坏旧会话反序列化
- `Event` 枚举完备性;`Done` 语义(必须是流的最后一个事件且必须出现)
- id 类型(`id.rs`)是否防混用(session id / turn id / tool call id 不能互换)
- `permission.rs` 中决策类型的语义:默认值是否 fail-closed
- JSON-RPC 帧定义(`rpc.rs`)与 newline-delimited 传输的转义处理
- 验证生成物:重跑 `gen_schema` 后 `schemas/protocol.json`、`src/bridge/generated.ts` 的 `git diff` 应为空

### ☐ A2 · riot-core 全部(含集成测试)—— 🟠 高 · 约 4.8k 行

| 文件 | 行数 | 说明 |
|---|---|---|
| `crates/riot-core/src/agent_loop.rs` | 678 | 主循环 |
| `crates/riot-core/src/invariants.rs` | 677 | 不变量断言 |
| `crates/riot-core/src/compactor.rs` | 523 | 上下文压缩 |
| `crates/riot-core/src/summarize.rs` | 436 | |
| `crates/riot-core/src/turn.rs` | 204 | |
| `crates/riot-core/src/state.rs` | 184 | |
| `crates/riot-core/src/guard.rs` | 120 | |
| `crates/riot-core/src/testing.rs` | 758 | 轻读 |
| `crates/riot-core/tests/`(golden / fault_injection / queued_input / stop_gate) | 1173 | 轻读,关注断言 |

**审查要点**
- 主循环无 `?` 抛穿;所有失败路径转对话内容
- 中断/取消后 tool_use 与 tool_result 的配对补齐(不变量)
- `Done` 在 panic 捕获路径也必须合成
- 压缩分层(落盘 → 清理 → 总结)触发条件;prompt cache 前缀稳定性是否被任何改动破坏
- `invariants.rs` 与 `docs/VERIFICATION.md` 一一对照,确认没有缺失或被弱化的断言,且 debug build 默认开启
- golden 测试是否真的锁住了事件序列;fault_injection 覆盖了哪些故障点

---

## B 组:安全边界(最高优先级,逐行精读)

### ☐ B1 · riot-permissions 决策链 —— 🔴 极高 · 约 2.5k 行

| 文件 | 行数 |
|---|---|
| `crates/riot-permissions/src/chain.rs` | 1107 |
| `crates/riot-permissions/src/rules.rs` | 460 |
| `crates/riot-permissions/src/safety.rs` | 404 |
| `crates/riot-permissions/src/fence.rs` | 383 |
| `crates/riot-permissions/src/testing.rs` / `lib.rs` | 124 |

**审查要点**
- 决策链每一环的默认值:遇到无法判断的情况是拒绝/问人,还是放行?
- 规则匹配优先级与短路顺序;deny 规则能否被更宽的 allow 规则意外覆盖
- fence 的路径边界:规范化时机、符号链接解析、`..` 处理、macOS 大小写不敏感文件系统
- 通配符/glob 规则的锚定(`*` 是否会匹配到路径分隔符之外)
- 构造绕过反例:相对路径、`~` 展开、UNC 路径(Windows)、尾部斜杠差异

### ☐ B2 · riot-permissions bash 静态分析 —— 🔴 极高 · 约 2.4k 行

| 文件 | 行数 |
|---|---|
| `crates/riot-permissions/src/bash/ast.rs` | 674 |
| `crates/riot-permissions/src/bash/decide.rs` | 494 |
| `crates/riot-permissions/src/bash/write_targets.rs` | 360 |
| `crates/riot-permissions/src/bash/readonly.rs` | 232 |
| `crates/riot-permissions/src/bash/mod.rs` | 15 |
| `crates/riot-permissions/src/bash/tests.rs` | 593 |

**审查要点**
- 解析不了的命令必须走"问人",绝不默认放行(设计哲学第 2 条)
- 绕过面逐项验证:命令替换 `$(...)` / 反引号、子 shell、管道、`&&`/`;` 链、重定向、`eval`、`xargs`、`sh -c`、`env VAR=x cmd`、别名、函数定义、heredoc
- 引号与转义的处理:`"$(rm -rf /)"` 这类嵌套是否被拆出来
- `readonly.rs` 白名单的保守性:每个"只读"命令是否真的无副作用(如 `sort -o`、`sed -i`、`git log` 的 pager)
- `write_targets.rs` 提取写目标的完备性:重定向目标、`-o` 参数、`tee`、`cp/mv` 目标位
- tests.rs 是否覆盖上述所有绕过面;缺哪些补哪些

### ☐ B3 · riot-runtime 沙箱 —— 🔴 极高 · 约 3.0k 行

| 文件 | 行数 |
|---|---|
| `crates/riot-runtime/src/sandbox_win.rs` | 1372 |
| `crates/riot-runtime/src/sandbox_labels.rs` | 668 |
| `crates/riot-runtime/src/sandbox.rs` | 536 |
| `crates/riot-runtime/src/sandbox_macos.rs` | 192 |
| `crates/riot-runtime/src/sandbox_cmdline.rs` | 187 |
| `crates/riot-runtime/src/lib.rs` | 38 |

对照:`docs/SANDBOX_WINDOWS.md`、`scripts/check-windows-sandbox.sh`

**审查要点**
- macOS:seatbelt profile 的生成与字符串转义(路径中含 `"` 或换行会不会破坏 profile 语法);deny-by-default 还是 allow-by-default
- Windows:restricted token / job object / ACL 的组合是否闭合;继承句柄、环境变量、命名管道等逃逸面
- 沙箱初始化失败时的行为:必须拒绝执行,不能静默降级为无沙箱
- `sandbox_labels.rs` 的标签→策略映射:每个标签的实际能力与名字是否一致
- `sandbox_cmdline.rs` 命令行组装的注入面
- 文档与实现的一致性:SANDBOX_WINDOWS.md 声称的限制逐条在代码里找到对应

### ☐ B4 · riot-runtime 执行原语 —— 🟠 高 · 约 1.9k 行

| 文件 | 行数 |
|---|---|
| `crates/riot-runtime/src/web.rs` | 397 |
| `crates/riot-runtime/src/fs.rs` | 375 |
| `crates/riot-runtime/src/proc.rs` | 297 |
| `crates/riot-runtime/tests/real_process.rs` | 376 |
| `crates/riot-runtime/tests/stdin_isolation.rs` | 81 |

**审查要点**
- `proc.rs`:进程组管理;kill 时是否杀干净子进程树;僵尸进程回收;超时后的清理
- stdin 隔离(结合 stdin_isolation 测试):子进程不能读到内核进程的 stdin
- `fs.rs`:TOCTOU 窗口;原子写(临时文件+rename);权限位保留
- `web.rs`:SSRF 防护(内网 IP、localhost、link-local、DNS rebinding);重定向次数与跨域重定向

---

## C 组:工具层 riot-tools

### ☐ C1 · 调度与注册骨架 —— 🟠 高 · 约 3.0k 行

| 文件 | 行数 | 说明 |
|---|---|---|
| `crates/riot-tools/src/scheduler.rs` | 1349 | |
| `crates/riot-tools/src/registry.rs` | 313 | |
| `crates/riot-tools/src/partition.rs` | 321 | |
| `crates/riot-tools/src/redact.rs` | 242 | |
| `crates/riot-tools/src/testing.rs` | 706 | 轻读 |
| `crates/riot-tools/src/lib.rs` | 20 | |

**审查要点**
- 并发批次划分:默认不可并发是否由 trait 默认方法强制;写操作绝不与其他操作同批
- 取消传播:调度中的任务收到取消后,已启动的工具如何收尾;tool_result 配对是否仍然成立
- `redact.rs`:遮蔽规则覆盖哪些密钥形态(env、URL 内嵌凭据、header);漏检后果
- registry 的重名/覆盖行为

### ☐ C2 · shell 执行工具 —— 🔴 极高 · 约 2.3k 行

| 文件 | 行数 |
|---|---|
| `crates/riot-tools/src/tools/bash.rs` | 603 |
| `crates/riot-tools/src/tools/bash_tests.rs` | 615 |
| `crates/riot-tools/src/tools/terminal.rs` | 539 |
| `crates/riot-tools/src/tools/fakeproc.rs` | 130 |
| `crates/riot-tools/src/tools/precondition.rs` | 158 |
| `crates/riot-tools/tests/bypass_behavior.rs` | 264 |

**审查要点**
- 与 B2 决策链的衔接:执行前是否所有路径都过了 `bash::decide`;有没有旁路入口
- bypass 模式(`bypass_behavior.rs`)的边界:哪些情况下允许绕过,是否可被模型自主触发
- 输出截断与二进制输出处理;超长输出的内存上限
- 后台进程:登记、查询、终止的生命周期;工作目录与环境变量的传递
- terminal(PTY)与 bash 的能力差异;PTY 路径是否同样过权限

### ☐ C3 · 文件系统工具 —— 🟠 高 · 约 3.6k 行

| 文件 | 行数 | 说明 |
|---|---|---|
| `crates/riot-tools/src/tools/search.rs` | 611 | |
| `crates/riot-tools/src/tools/read.rs` | 438 | |
| `crates/riot-tools/src/tools/edit.rs` | 373 | |
| `crates/riot-tools/src/tools/grep.rs` + `grep_tests.rs` | 741 | 测试轻读 |
| `crates/riot-tools/src/tools/glob.rs` + `glob_tests.rs` | 497 | 测试轻读 |
| `crates/riot-tools/src/tools/text.rs` | 267 | |
| `crates/riot-tools/src/tools/write.rs` | 211 | |
| `crates/riot-tools/src/tools/memfs.rs` | 209 | |
| `crates/riot-tools/src/tools/path.rs` | 149 | |
| `crates/riot-tools/src/tools/shrink.rs` | 116 | |

**审查要点**
- `edit.rs`:old_string 唯一匹配的语义;替换后编码/换行符保持;并发编辑同一文件的防护
- `write.rs`:覆盖已有文件前的 read 前置校验(`precondition.rs` 联动)
- `path.rs`:工作区边界检查是所有文件工具共用的吗?有没有工具自己拼路径绕开它
- read 的行号前缀、大文件截断、二进制探测
- grep/glob 对 `.gitignore` / 隐藏文件的默认策略;正则 DoS(灾难性回溯)

### ☐ C4 · web 工具集 —— 🟠 高 · 约 3.2k 行

| 文件 | 行数 | 说明 |
|---|---|---|
| `crates/riot-tools/src/tools/web/url.rs` | 567 | |
| `crates/riot-tools/src/tools/web/search.rs` | 361 | |
| `crates/riot-tools/src/tools/web/fetch.rs` | 287 | |
| `crates/riot-tools/src/tools/web/pipeline.rs` | 263 | |
| `crates/riot-tools/src/tools/web/cache.rs` | 259 | |
| `crates/riot-tools/src/tools/web/consent.rs` | 244 | |
| `crates/riot-tools/src/tools/web/preapproved.rs` | 216 | |
| `crates/riot-tools/src/tools/web/markdown.rs` | 229 | |
| `crates/riot-tools/src/tools/web/date.rs` + `mod.rs` | 142 | |
| `crates/riot-tools/src/tools/web/tests.rs` | 669 | 轻读 |

**审查要点**
- `url.rs`:URL 规范化与校验(IDN/punycode 混淆、`@` 用户信息段、IP 字面量各种进制写法)
- `consent.rs`:同意流程能否被并发请求或缓存绕过;同意的粒度(域名级?URL 级?)
- `preapproved.rs`:预批准清单逐条评估;子域名匹配是否过宽
- `cache.rs`:缓存键是否含用户身份;投毒与过期策略
- 抓取内容注入对话前的清洗(prompt injection 面)

### ☐ C5 · browser 工具 —— 🟠 高 · 约 4.0k 行

| 文件 | 行数 |
|---|---|
| `crates/riot-tools/src/tools/browser.rs` | 3985 |

**审查要点**
- CDP 方法的允许/拒绝清单:cookie、storage、下载、文件上传、target 管理等敏感方法是否拦截
- 注入 JS(`Runtime.evaluate`)的能力边界;返回值大小限制
- 截图/快照的数据量控制与落盘位置
- 与 riot-browser 进程的协议(对照 D3);浏览器进程崩溃时工具的错误路径
- 锁定(lock)语义:并发会话抢占同一 tab 的行为

### ☐ C6 · 交互与元工具 —— 🟡 中 · 约 4.4k 行

| 文件 | 行数 | 说明 |
|---|---|---|
| `crates/riot-tools/src/tools/tool_search.rs` | 608 | |
| `crates/riot-tools/src/tools/diagnostics.rs` | 535 | |
| `crates/riot-tools/src/tools/ask.rs` | 523 | |
| `crates/riot-tools/src/tools/pentest.rs` | 389 | 重点 |
| `crates/riot-tools/src/tools/skill.rs` | 320 | |
| `crates/riot-tools/src/tools/todo.rs` | 288 | |
| `crates/riot-tools/src/tools/plan.rs` | 186 | |
| `crates/riot-tools/src/tools/mod.rs` | 93 | |
| `crates/riot-tools/src/tools/tests.rs` | 1088 | 轻读 |
| `crates/riot-tools/tests/with_agent_loop.rs` | 406 | 轻读 |

**审查要点**
- `pentest.rs`:这是敏感能力,启用条件、权限门槛、能力范围逐行确认
- `ask.rs`:问人请求的超时与默认答案(超时后默认拒绝还是放行?)
- `skill.rs`:skill 文件的加载路径是否限定;skill 内容作为 prompt 注入的边界
- 每个工具的 JSON schema 与实际参数解析一致(模型看到的和代码做的一致)

---

## D 组:Provider / MCP / 浏览器进程

### ☐ D1 · providers 传输层 + Anthropic —— 🟠 高 · 约 4.4k 行(可拆两次读)

| 文件 | 行数 |
|---|---|
| `crates/riot-providers/src/retry.rs` | 484 |
| `crates/riot-providers/src/sse.rs` | 394 |
| `crates/riot-providers/src/watchdog.rs` | 231 |
| `crates/riot-providers/src/http.rs` | 212 |
| `crates/riot-providers/src/transport.rs` | 200 |
| `crates/riot-providers/src/endpoint.rs` | 126 |
| `crates/riot-providers/src/lib.rs` | 40 |
| `crates/riot-providers/src/anthropic/request.rs` | 1044 |
| `crates/riot-providers/src/anthropic/decode.rs` | 766 |
| `crates/riot-providers/src/anthropic/provider.rs` | 644 |
| `crates/riot-providers/src/anthropic/wire.rs` | 213 |

**审查要点**
- SSE 解析:跨 chunk 的事件边界、UTF-8 断字、异常帧的恢复
- 重试:哪些错误可重试;重试是否幂等(流式响应中途失败后重发会不会重复扣 token / 重复事件);退避上限
- watchdog:静默超时的阈值与误杀;超时后资源清理
- API key 的流转路径:确认不出现在日志、错误消息、事件流
- `anthropic/request.rs`:消息序列化、`cache_control` 打点位置(prompt cache 前缀稳定性)、工具定义转换
- `decode.rs` 状态机:乱序/缺失事件的容错;`content_block` 配对

### ☐ D2 · OpenAI + provider 集成测试 —— 🟡 中高 · 约 2.9k 行

| 文件 | 行数 | 说明 |
|---|---|---|
| `crates/riot-providers/src/openai/provider.rs` | 373 | |
| `crates/riot-providers/src/openai/request.rs` | 355 | |
| `crates/riot-providers/src/openai/decode.rs` | 280 | |
| `crates/riot-providers/src/openai/wire.rs` + `mod.rs` | 227 | |
| `crates/riot-providers/src/openai/tests.rs` | 861 | 轻读 |
| `crates/riot-providers/tests/end_to_end.rs` | 358 | 轻读 |
| `crates/riot-providers/tests/real_http.rs` | 398 | 轻读,注意是否依赖真实网络 |

**审查要点**
- 与 Anthropic 路径的行为一致性(同一内部事件模型两边映射是否对称)
- OpenAI 工具调用格式差异(function calling)转换的正确性
- real_http 测试的开关条件(CI 里跑不跑,凭据从哪来)

### ☐ D3 · riot-mcp + riot-browser 进程 —— 🟡 中高 · 约 3.4k 行

| 文件 | 行数 |
|---|---|
| `crates/riot-mcp/src/tool.rs` | 750 |
| `crates/riot-mcp/src/lib.rs` | 393 |
| `crates/riot-mcp/src/client.rs` | 380 |
| `crates/riot-mcp/src/hub.rs` | 322 |
| `crates/riot-mcp/src/wire.rs` + `stdio.rs` | 250 |
| `crates/riot-browser/src/osr.rs` | 360 |
| `crates/riot-browser/src/dispatch.rs` | 333 |
| `crates/riot-browser/src/main.rs` | 235 |
| `crates/riot-browser/src/paths.rs` | 134 |
| `crates/riot-browser/src/mac.rs` / `cdp.rs` / `helper.rs` / `wire.rs` | 263 |

**审查要点**
- MCP:子进程 spawn 时的环境变量传递(会不会把宿主全部 env 泄给第三方 MCP);stdio 死锁(两端都在等对方读)
- MCP 工具名与内置工具的冲突处理;不可信 MCP 返回内容的大小限制与清洗
- MCP 服务器崩溃/挂起的超时与重启策略
- riot-browser:与宿主的 IPC 协议版本;崩溃后孤儿进程清理;`paths.rs` 的 profile 目录隔离

---

## E 组:内核 riot-kernel + 存储

### ☐ E1 · session.rs —— 🔴 极高 · 约 3.8k 行(单文件,核心枢纽)

| 文件 | 行数 |
|---|---|
| `crates/riot-kernel/src/session.rs` | 3829 |

**审查要点**
- 会话状态机:所有状态迁移画出来,找不可达/卡死状态;turn 的开始/结束边界
- 事件发出顺序与持久化时机的关系(先持久化还是先发事件?崩溃后重放是否一致)
- 排队输入(queued input)与正在进行的 turn 的交互;中断语义
- 多会话并发时的隔离(共享内核进程模式下)
- 与 gate(权限)、scheduler(工具)、provider(模型)三方衔接处的错误路径
- 这个文件近 4k 行,顺带评估:是否有清晰的内聚分段,还是需要拆分的技术债

### ☐ E2 · 内核骨架:RPC / 权限门 / 管理 —— 🟠 高 · 约 2.8k 行

| 文件 | 行数 | 说明 |
|---|---|---|
| `crates/riot-kernel/src/gate.rs` | 645 | 权限门 |
| `crates/riot-kernel/src/content.rs` | 532 | |
| `crates/riot-kernel/src/manager.rs` | 516 | |
| `crates/riot-kernel/src/lib.rs` | 465 | |
| `crates/riot-kernel/src/bridge.rs` | 396 | |
| `crates/riot-kernel/src/main.rs` | 53 | |
| `crates/riot-kernel/tests/stdio_smoke.rs` | 191 | 轻读 |

**审查要点**
- `gate.rs`:内核侧权限决策与 riot-permissions 的职责边界;问人请求在 UI 无响应时的行为;决策缓存的键(同名不同参的命令会不会复用错误决策)
- `bridge.rs` / RPC 分发:未知方法的处理;请求 id 配对;并发请求的顺序保证
- `manager.rs`:会话创建/销毁的资源清理;孤儿会话

### ☐ E3 · 配置系统 —— 🟡 中高 · 约 3.0k 行

| 文件 | 行数 |
|---|---|
| `crates/riot-kernel/src/config.rs` | 2267 |
| `crates/riot-kernel/src/models.rs` | 325 |
| `crates/riot-kernel/src/env.rs` | 210 |
| `crates/riot-kernel/src/packs.rs` | 184 |

**审查要点**
- 配置来源合并优先级(默认值 < 全局 < 项目 < 会话?)与文档一致
- 每个安全相关配置项的默认值(沙箱开关、权限模式、web 开关)必须是安全侧
- 密钥字段的存取路径(keychain?明文 toml?);配置文件权限位
- `env.rs` 对照 `docs/ENV_DESIGN.md`;环境变量白名单/黑名单
- 配置热更新时正在运行的会话用旧值还是新值

### ☐ E4 · 扩展系统 —— 🟠 高 · 约 3.2k 行

| 文件 | 行数 |
|---|---|
| `crates/riot-kernel/src/hooks.rs` | 960 |
| `crates/riot-kernel/src/subagent.rs` | 852 |
| `crates/riot-kernel/src/skills.rs` | 735 |
| `crates/riot-kernel/src/slash.rs` | 635 |

**审查要点**
- hooks:hook 命令的执行环境(过不过沙箱?过不过权限?);超时;hook 输出注入对话的清洗;hook 能否阻止/篡改工具调用
- subagent:子 agent 的权限收窄是否强制(设计说只做只读 Explore);子 agent 的工具集裁剪;递归深度限制;取消传播
- skills / slash:发现→注册→执行是否共用同一管道(设计哲学第 5 条);目录扫描的路径边界;同名冲突

### ☐ E5 · 上下文与记忆 + 存储 —— 🟡 中高 · 约 3.1k 行

| 文件 | 行数 |
|---|---|
| `crates/riot-kernel/src/mentions.rs` | 783 |
| `crates/riot-kernel/src/memory.rs` | 416 |
| `crates/riot-kernel/src/vision.rs` | 387 |
| `crates/riot-kernel/src/prompt.rs` | 364 |
| `crates/riot-kernel/src/classifier.rs` | 244 |
| `crates/riot-store/src/lib.rs` | 917 |

**审查要点**
- `mentions.rs`:@ 引用解析出的路径必须过工作区边界检查;二进制/超大文件的处理
- `memory.rs`:记忆写入的目标文件位置;模型能否诱导写到任意路径
- `prompt.rs`:系统提示组装顺序的稳定性(prompt cache);用户内容与系统内容的边界
- riot-store:SQLite schema 与迁移(旧版本库打开新库?);事务边界;WAL 模式;单文件 917 行承载所有持久化,评估内聚性
- `vision.rs`:图片尺寸/格式校验,解码库的输入是不可信数据

### ☐ E6 · 变更追踪与 web 检索 —— 🟡 中 · 约 2.4k 行

| 文件 | 行数 | 说明 |
|---|---|---|
| `crates/riot-kernel/src/changes.rs` | 625 | |
| `crates/riot-kernel/src/git_changes.rs` | 464 | |
| `crates/riot-kernel/src/git.rs` | 227 | |
| `crates/riot-kernel/src/web/mod.rs` | 219 | |
| `crates/riot-kernel/src/web/searxng.rs` | 309 | |
| `crates/riot-kernel/src/web/distill.rs` | 211 | |
| `crates/riot-kernel/src/web/tests.rs` | 385 | 轻读 |

**审查要点**
- git 命令构造(argv 数组,不拼字符串);对含空格/非 UTF-8 文件名的处理
- changes 的文件快照与磁盘实际状态漂移时的行为
- searxng:实例地址来源(对照 `deploy/searxng`);查询词是否泄漏敏感上下文
- distill 的 HTML 清洗(脚本/样式剥离)

---

## F 组:宿主层 src-tauri

### ☐ F1 · 状态与内核进程管理 —— 🟠 高 · 约 3.1k 行

| 文件 | 行数 |
|---|---|
| `src-tauri/src/state.rs` | 2085 |
| `src-tauri/src/kernel/client.rs` | 422 |
| `src-tauri/src/kernel/supervisor.rs` | 374 |
| `src-tauri/src/kernel/coalesce.rs` | 246 |
| `src-tauri/src/kernel/mod.rs` | 8 |

**审查要点**
- supervisor:内核崩溃检测与重启;重启风暴的退避;重启后会话状态恢复
- client:RPC 请求超时;stdout 解析对半行/超长行的容错
- coalesce:事件合并会不会丢关键事件(权限请求、Done)
- state.rs 2k 行:锁的粒度与持锁跨 await;评估拆分必要性

### ☐ F2 · Tauri 入口与环境 —— 🟠 高 · 约 2.6k 行 + 配置

| 文件 | 行数 |
|---|---|
| `src-tauri/src/lib.rs` | 1053 |
| `src-tauri/src/gui_env.rs` | 440 |
| `src-tauri/src/fence.rs` | 300 |
| `src-tauri/src/persist.rs` | 282 |
| `src-tauri/src/env_probe.rs` | 257 |
| `src-tauri/src/update.rs` | 211 |
| `src-tauri/src/main.rs` | 18 |
| `src-tauri/tauri.conf.json` + 平台变体 + `capabilities/default.json` | — |

**审查要点**
- **每个 `#[tauri::command]` 都是攻击面**:在 lib.rs 里列全清单,逐个确认参数校验与权限;renderer 被 XSS 后能通过 command 做到什么
- capabilities 最小化;CSP 设置;`assetProtocol` scope
- update.rs:更新包签名验证;下载渠道是否 HTTPS + 固定域名
- fence.rs 与 riot-permissions 的 fence 是否同一套逻辑(两处实现会漂移)

### ☐ F3 · 浏览器宿主:访问控制 —— 🟠 高 · 约 2.3k 行

| 文件 | 行数 |
|---|---|
| `src-tauri/src/browser/access.rs` | 2307 |

**审查要点**
- URL 访问决策的完整路径;决策缓存;重定向后二次校验
- 与 C4 web 工具、C5 browser 工具的策略一致性(三处都做 URL 判断,标准是否统一)
- 单文件 2.3k 行的内聚性评估

### ☐ F4 · 浏览器宿主:操作与网络 —— 🟡 中高 · 约 2.5k 行

| 文件 | 行数 |
|---|---|
| `src-tauri/src/browser/ops.rs` | 1450 |
| `src-tauri/src/browser/mod.rs` | 467 |
| `src-tauri/src/browser/netlog.rs` | 364 |
| `src-tauri/src/browser/taps.rs` | 171 |

**审查要点**
- ops 暴露给内核/前端的操作清单;输入合成(键鼠)的目标校验
- netlog 记录的内容(URL 参数里的 token 会不会落盘);日志轮转
- taps 的事件监听范围

### ☐ F5 · 终端 / 输入 / 能力包 —— 🟠 高 · 约 3.5k 行

| 文件 | 行数 |
|---|---|
| `src-tauri/src/term.rs` | 901 |
| `src-tauri/src/packs/download.rs` | 640 |
| `src-tauri/src/packs/mod.rs` | 540 |
| `src-tauri/src/packs/install.rs` | 436 |
| `src-tauri/src/askpass.rs` | 364 |
| `src-tauri/src/term_access.rs` | 331 |
| `src-tauri/src/vibrancy.rs` | 178 |
| `src-tauri/src/pasteboard.rs` | 111 |

**审查要点**
- term:PTY 会话与内核工具执行的关系;term_access 的访问控制;转义序列注入(终端输出渲染到 UI)
- askpass:凭据从输入到使用的全路径;内存中是否及时清零;绝不落日志
- packs:下载校验(哈希/签名,对照 `scripts/build-doc-pack.mjs` 的产出);**解压路径穿越(zip slip)**;安装目录权限;降级攻击(旧版本包重放)
- pasteboard:剪贴板读取的触发条件(不能静默常驻读)

---

## G 组:前端 renderer

### ☐ G1 · bridge 与会话 hook —— 🟡 中高 · 约 4.1k 行

| 文件 | 行数 | 说明 |
|---|---|---|
| `src/bridge/index.ts` | 1126 | |
| `src/bridge/generated.ts` | 1264 | 生成物:重新生成后 diff 验证 |
| `src/hooks/useSession.ts` | 1694 | |

**审查要点**
- bridge 是"唯一允许调用宿主的地方"(架构约束):全局搜 `invoke(`/`Channel` 确认没有组件绕过 bridge 直调
- useSession:事件流的订阅/退订与组件生命周期;乱序/重复事件的容错;重连后的状态重建
- 与 protocol 类型的一致性(靠 generated.ts 保证,确认没有手写的平行类型)

### ☐ G2 · 主界面 —— 🟡 中 · 约 2.7k 行

| 文件 | 行数 |
|---|---|
| `src/components/Composer.tsx` | 1547 |
| `src/App.tsx` | 1092 |
| `src/main.tsx` + `src/pathDisplay.ts` | ~100 |

**审查要点**
- Composer:粘贴图片/文件的处理路径;@ 提及与 / 命令的补全数据来源;IME 输入法兼容
- App:全局状态的传递方式;路由/窗口焦点管理;快捷键冲突

### ☐ G3 · 会话渲染 —— 🟡 中 · 约 2.6k 行

| 文件 | 行数 |
|---|---|
| `src/components/Transcript.tsx` | 801 |
| `src/components/ToolCard.tsx` | 472 |
| `src/components/Markdown.tsx` | 387 |
| `src/components/ProcessFold.tsx` | 326 |
| `src/components/FileChangeList.tsx` | 262 |
| `src/components/Mermaid.tsx` | 217 |
| `src/components/TodoPanel.tsx` | 145 |

**审查要点**
- **Markdown/Mermaid 渲染的 XSS 面**:模型输出是不可信输入;链接的 `javascript:` 协议;图片 src 的本地路径泄漏
- 长会话的渲染性能(虚拟化/memo);流式增量更新的重排
- ToolCard 展示的参数与实际执行参数一致(截断提示要明确)

### ☐ G4 · 面板与通用组件 —— 🟡 中 · 约 3.1k 行

| 文件 | 行数 |
|---|---|
| `src/components/BrowserPanel.tsx` | 854 |
| `src/components/TerminalPanel.tsx` | 688 |
| `src/components/chrome.tsx` | 330 |
| `src/components/Sidebar.tsx` | 254 |
| `src/components/pickers.tsx` | 223 |
| `src/components/icons.tsx` | 192 |
| `src/components/GitChangesPanel.tsx` | 171 |
| `src/components/FieldSelect.tsx` | 170 |
| `src/components/Modal.tsx` | 142 |
| `src/components/Welcome.tsx` | 110 |

**审查要点**
- TerminalPanel:xterm(或等价物)的转义序列处理;剪贴板集成
- BrowserPanel:截图/画面数据的传输量;交互事件转发的坐标换算

### ☐ G5 · 设置与权限 UI —— 🟡 中高 · 约 3.6k 行

| 文件 | 行数 |
|---|---|
| `src/components/PermissionDialog.tsx` | 436 |
| `src/components/SessionSettings.tsx` | 313 |
| `src/components/ModelDialog.tsx` | 215 |
| `src/components/Settings.tsx` | 214 |
| `src/components/settings/ProviderEditor.tsx` | 508 |
| `src/components/settings/McpPane.tsx` | 485 |
| `src/components/settings/PermissionPane.tsx` | 326 |
| `src/components/settings/ProviderPane.tsx` | 293 |
| `src/components/settings/PacksPane.tsx` | 197 |
| `src/components/settings/WebPane.tsx` | 169 |
| `src/components/settings/SkillsPane.tsx` | 120 |
| `src/components/settings/AboutPane.tsx` | 116 |
| `src/components/settings/CommandsPane.tsx` | 115 |
| `src/components/settings/HooksPane.tsx` | 109 |

**审查要点**
- **PermissionDialog:用户看到的命令/路径与内核将执行的完全一致**(不因省略号截断、HTML 转义、RTL 字符产生误导);默认聚焦按钮是拒绝还是允许
- ProviderEditor:API key 输入的回显控制;保存路径(走 keychain 还是明文)
- 各 Pane 修改后的生效时机与失败提示

### ☐ G6 · 工具库与样式 —— 🟢 低 · 约 7.0k 行(大部分快速过)

| 文件 | 行数 | 说明 |
|---|---|---|
| `src/lib/fileIcons.ts` | 416 | |
| `src/lib/promptText.ts` | 216 | |
| `src/lib/partialJson.ts` | 142 | 重点精读 |
| `src/styles.css` | 6062 | 快速过 |
| `index.html` / `vite.config.ts` / `tsconfig.json` | ~120 | |

**审查要点**
- `partialJson.ts`:流式半截 JSON 的解析健壮性(工具参数流式展示依赖它),构造畸形输入验证
- index.html 的 CSP meta;vite 构建的 sourcemap / minify 设置

---

## H 组:网站 / 脚本 / CI / 文档

### ☐ H1 · website —— 🟢 低 · 约 3.5k 行(快速过)

| 文件 | 行数 |
|---|---|
| `website/style.css` | 1708 |
| `website/script.js` | 1204 |
| `website/index.html` | 557 |

**审查要点**
- 下载链接指向与版本更新机制;外链 `rel="noopener"`
- script.js 里有没有引入第三方跟踪;表单提交目标

### ☐ H2 · 构建脚本与 CI —— 🟡 中 · 约 2.5k 行

| 文件 | 行数 | 说明 |
|---|---|---|
| `scripts/mutate.py` | 888 | 变异测试脚本 |
| `scripts/build-doc-pack.mjs` | 373 | 对照 F5 packs 校验逻辑 |
| `scripts/build-browser.sh` / `.ps1` | ~150 | |
| `scripts/check-windows-sandbox.sh` | 80 | 对照 B3 |
| `scripts/stage-kernel.mjs` / `stage-browser.mjs` | 85 | |
| `scripts/gen_icon.py` | 11 | |
| `.github/workflows/ci.yml` / `release.yml` | — | 重点 |
| `deploy/searxng/` | — | 对照 E6 |
| 根配置:`Cargo.toml` / `clippy.toml` / `package.json` / `pnpm-lock.yaml`(抽查) | — | |

**审查要点**
- ci.yml:是否跑全量测试 + clippy + fmt;失败是否阻塞;有没有 `continue-on-error` 掩盖问题
- release.yml:签名密钥的注入方式;产物校验和的生成与发布;tag 触发条件
- 脚本中的 shell 注入(变量未加引号)、`curl | sh` 模式
- clippy.toml 的 allow 清单是否过宽;Cargo.toml 的 workspace lints、profile 设置
- 依赖审计:`cargo deny`/`audit` 有没有接入;pnpm 锁文件里的可疑包

### ☐ H3 · 文档与实现一致性(可选收尾)—— 🟢 低

| 文件 | 说明 |
|---|---|
| `docs/ARCHITECTURE.md` | 逐条 `[约束]` 在代码中找到对应实现 |
| `docs/VERIFICATION.md` | 与 A2 的 invariants.rs 对照(已在 A2 做过则抽查) |
| `docs/ENV_DESIGN.md` | 与 E3 env.rs 对照 |
| `docs/SANDBOX_WINDOWS.md` | 与 B3 对照(已做过则跳过) |
| `README.md` / `AGENT_DESIGN.md` | 快速过,找过时描述 |

**审查要点**
- 文档声称但代码未实现的(虚假承诺)、代码实现但文档未提的(隐藏行为)

---

## 附:审查产出建议

每批完成后记录三类产出,便于汇总:

1. **缺陷**:文件 + 行号 + 问题描述 + 严重级(阻塞 / 高 / 中 / 低)
2. **疑问**:看不懂或存疑的设计,标记后向作者(或 AI)提问
3. **技术债**:不阻塞但应排期的(超长文件拆分、缺失测试、文档漂移)
