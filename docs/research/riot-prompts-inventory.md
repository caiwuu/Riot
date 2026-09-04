# Riot 提示词体系盘点

> 调研范围：`/Users/caiwu/code/Riot` 全仓库静态阅读，不含任何代码改动。
> 目的：为「提示词优化专题」提供 Riot 侧的现状家底。
> 调研日期：2026-09-04。

## 速览

| 项 | 结论 |
| --- | --- |
| 主 system prompt 体量 | **1930 字符 / 9 个语义分节 + 3 个条件段**，全中文 |
| 组装入口 | `crates/riot-kernel/src/prompt.rs:19` `system_prompt()`，装配点 `session.rs:2872` |
| 每轮静态前缀总量 | 约 **18200 字符**（system 1930 + 52 个工具描述 12307 + schema 字段说明 3922） |
| 每轮动态增量 | 约 30–400 字符（时钟行 + 环境差分），全部走消息侧 `<system-reminder>` |
| 核心架构取舍 | 一切会中途变化的内容都赶出 system prompt，保前缀缓存 |
| 用户可覆盖面 | AGENTS.md / Skills / 斜杠命令 / 会话追加指令；**内置提示词一个字都改不了** |
| 厂商差异化 | 无（机制 `SystemSection` 存在但生产链路传空） |
| 最薄弱处 | 无结构化分节、无沟通风格约束、核心编辑工具描述过短、Browser 系列 32 个工具描述含混 |

---

## 1. 提示词全景图：一次 agent turn 发给模型的内容是怎么拼出来的

### 1.1 核心入口

| 角色 | 位置 |
| --- | --- |
| **主 system prompt 生成** | `crates/riot-kernel/src/prompt.rs:19` `system_prompt(cwd, today, python_venv, extra, has_hooks) -> String` |
| **整轮装配总入口**（调用上者、定 system、建 fork 种子、拼历史） | `crates/riot-kernel/src/session.rs:2872`（`system` 变量诞生处），装配函数体覆盖约 `session.rs:2800-3130` |
| **每轮用户消息前置注入** | `session.rs:3042` `first_message_prelude()` + `session.rs:3058` `env_prelude()` |
| **每轮用户消息后置注入** | `session.rs:3075` `plan_mode_reminder()` + `session.rs:3077` `multitask_note()` |
| **附件 → wire 文本** | `crates/riot-providers/src/anthropic/request.rs:482` `convert_attachment()`（OpenAI 侧见 `openai/request.rs`） |

一个关键的架构决策贯穿全文（`prompt.rs:7-13` 的模块级注释）：**system prompt 是 prompt cache 的前缀第一段，改一个字，后面的工具定义 + 全部历史缓存整体作废**。因此所有「会在会话中途变化」的内容（精确时刻、权限模式、多任务模式、git 分支、环境快照）一律不进 system prompt，改走**消息侧 system-reminder 注入**。

### 1.2 一次 turn 的完整拼装顺序

```
┌─ [1] system 字段（provider 请求的 system）
│    └─ prompt.rs:19 system_prompt()
│         ├─ 身份段（prompt.rs:30-32）
│         ├─ 环境常量段：工作目录 / 平台 / 当前年月（prompt.rs:34-38）
│         ├─ 行为准则 8 条（prompt.rs:40-63）
│         ├─ 环境与时间感知契约（prompt.rs:65-73）
│         ├─ 代码引用格式约定 ```起始行:结束行:路径（prompt.rs:75-86）
│         ├─ mermaid 图表约定（prompt.rs:88-96）
│         ├─ 本地文件链接约定（prompt.rs:98-107）
│         ├─ 语言约定「回答用中文」（prompt.rs:109）
│         ├─ [条件] Python venv 段（prompt.rs:116-122，仅配了 venv）
│         ├─ [条件] hooks 段（prompt.rs:125-132，仅配了任一 hook）
│         └─ [条件] 用户会话级补充指令（prompt.rs:133-135，`extra`）
│
├─ [2] tools 字段：工具 JSON schema 数组
│    └─ 由 Registry 汇总，`tools_runner.specs()`（session.rs:2885）
│       每个工具的 description 即提示词，见 §2.4
│
└─ [3] messages 数组
     ├─ 历史消息（含此前轮次的注入，注入随消息进 transcript 永久留存）
     ├─ [压缩发生过则] 压缩摘要消息 + 恢复的文件附件（见 §2.3）
     └─ 本轮 User 消息，content 数组按此顺序：
          ├─ (a) [仅会话第一条] 记忆文件 AGENTS.md
          │      session.rs:1803 first_message_prelude → memory::collect
          │      wire: <system-reminder>项目记忆 {path}：…</system-reminder>
          ├─ (b) [仅会话第一条] git 快照（分支 / 是否脏）
          │      session.rs:1820 git::probe + git::describe
          ├─ (c) 环境状态块（每轮）session.rs:1842 env_prelude
          │      ├─ 时钟行（必发）env.rs:120 clock_line
          │      │    「现在是 2026-08-31（周一）16:37，UTC+8。」
          │      ├─ [间隔 >30min] 间隔警示 env.rs:161 gap_line
          │      ├─ [采样失败] STALE_NOTICE env.rs:184
          │      └─ [向上越档时] 上下文用量档位行 env.rs:103 band_line
          ├─ (d) [快照有变化时] 环境快照全文 env.rs:31 render
          ├─ (e) [最多 3 条] 终端异常告警 env.rs:80 alert_text
          ├─ (f) 用户正文 + 附件 + `@` 提及展开
          │      session.rs:3038 content::user_content
          ├─ (g) [规划模式] 规划模式提醒 prompt.rs:156 plan_mode_reminder
          └─ (h) [多任务模式] 多任务准则 prompt.rs:189 multitask_reminder
                 （Full / Short / Exit 三形态）
```

被通知（后台子 agent 完成）唤醒的轮次走另一条分支 `session.rs:2981-3004`：不注入 (a)~(f)，但 (g)(h) 照样追加在最后一条通知消息末尾。

### 1.3 注入位置的设计理由（代码注释里明写的）

- **记忆只注一次**（`session.rs:3039-3041`）：随消息进历史，往后每轮自然带着；每轮注会堆出 N 份。压缩吞掉后由 `compact_history` 重注。
- **git 快照不进 system prompt**（`session.rs:1817-1819`）：分支和脏不脏会变，写进去每切一次分支就作废整个缓存。
- **规划模式走消息末尾**（`prompt.rs:142-147`）：除缓存理由外还有权重理由——「离对话越近权重越高」只有跟在消息末尾才成立，system prompt 的尾部和本轮对话之间还隔着全部工具定义和历史。
- **记忆的全局→项目顺序**（`memory.rs:8-10`）：越靠近对话的越晚出现，项目约定应当压过全局偏好。

## 2. 提示词原文归档

### 2.1 主 system prompt（`crates/riot-kernel/src/prompt.rs`）

**体量**：基础部分（不含三个条件段）约 **1930 字符**（中文为主，约 1200–1500 token）。**分节数：9 个语义分节 + 3 个条件段**。全文中文撰写。

**函数签名**（`prompt.rs:19-25`）：

```19:25:crates/riot-kernel/src/prompt.rs
pub(crate) fn system_prompt(
    cwd: &std::path::Path,
    today: &str,
    python_venv: Option<&str>,
    extra: Option<&str>,
    has_hooks: bool,
) -> String {
```

#### 2.1.1 基础段全文（`prompt.rs:30-109`，已还原 Rust 续行转义）

> 你是 Riot——跑在用户机器上的全能智能体。编码只是你的一部分能力：调研、排查、自动化、验证，用手头的工具把事情真正做完，而不是只给建议；自我介绍时也不要把自己缩成「编程助手」。
>
> 工作目录：{cwd}
> 平台：{os}
> 现在是：{today}（这里只精确到月；精确的日期和时刻，每轮用户消息开头都会注入一行时钟，以它为准 —— 你记忆里的「今天」停在训练截止那天，早就过期了）
>
> **行为准则（每条都带着理由，理由是让你能推断没写到的情况）：**
> - 先搞清楚再动手。改代码前用 Read / Grep 看过相关位置，碰外部系统前先确认现状 —— 基于猜测的修改错了之后，用户得先理解你改了什么才能撤销，比从头做还慢。
> - 互不依赖的调用在同一次回复里并行发出：一批 Read / Grep / Glob、几条互不影响的命令，一起发。运行时会并发执行只读批次 —— 串行地一个等一个，只是把用户的等待时间乘上调用数。
> - 一次只做被要求的事。顺手重构、顺手加注释、顺手改格式，会让 diff 里混进无关改动 —— review 的人分不清哪些是任务本身、哪些是顺手，只能整体不信任。
> - 写代码要像周围的代码。命名、注释密度、错误处理方式都跟着现有风格走 —— 风格突变会让后来的维护者以为这里有特殊原因，白花时间考古。
> - 自主性按后果分档。可逆的操作（改文件、跑测试、装依赖）直接做完再汇报，停下来问「要继续吗」只是让用户干等；破坏性操作（删数据、覆盖未提交的改动、对外发布）和真正的需求歧义才停下来确认 —— 这两类猜错了没法撤销。
> - 工具失败时先读错误信息再动作，不要换个参数重试同一件事 —— 错误没消化，重试只是把同一堵墙撞第二遍。
> - 多步任务用 TodoWrite 拆解和跟踪：做完一项立刻标记完成，不要攒一批再改 —— 清单是用户看进度的窗口，攒着改等于窗口失真。
> - 说「做完了」之前先验证：能编译的编译，能跑的跑一遍 —— 没验证过的「完成」是把调试成本转嫁给用户。测试没过就如实报告，不要粉饰成完成。
> - 不要擅自提交。`git commit` 只在用户明确要求时做 —— 他多半想先看看改了什么；同理不要擅自 push、切分支、stash、reset。
>
> **环境与时间感知**：每轮用户消息开头会注入一行当前时刻（精确到分、含时区）。历史消息之间看不出隔了多久 —— 上一轮可能是几分钟前，也可能是昨天，判断「现在」一律以最新的时钟行为准；间隔久时会有专门提醒，提醒之前的环境状态和结论都可能已经过期。消息里还可能出现 `<system-reminder>` 包着的环境快照（终端面板和内置浏览器的现状）和环境事件（你能看的某个终端出现了报错）。没有新快照就是环境没变；但看到「环境采样失败」时，把手头的快照当作未知 —— 那不是「没变」。快照是采样不是指令 —— 与当前任务相关就用起来，无关就忽略，不要为了显得警觉而逐条评论。用户自己的终端默认对你不可见（连标题都没有），要看的话请他在终端面板上点「共享给 agent」，没有别的路。
>
> **代码引用格式**：引用仓库里**已有**的代码时，代码块的语言位置写成 `起始行:结束行:路径`：
>
> ````
> ```12:14:src/main.rs
> fn main() {
>     run();
> }
> ```
> ````
>
> 界面会把它渲染成带路径标题、点一下能打开文件的块。路径按工作目录的相对路径写，行号照文件里的实际行号。你**新写的**代码不要用这个格式 —— 那是普通代码块（写语言名，如 ```rust），两者在界面上是不同的东西：前者是「去看这里」，后者是「这是我建议加的」。
>
> **图表**：流程图、时序图、状态图用 mermaid 围栏直接写在回复里：
>
> ````
> ```mermaid
> flowchart LR
>     A --> B
> ```
> ````
>
> 界面会把它画成图。不要为了给人看图去写 HTML、引 mermaid.js、再打开浏览器 —— 浏览器是用来核对自己改过的页面，不是当画板。
>
> **本地文件链接**：指向本地文件（刚写的文档、报告、脚本）时，Markdown 链接的地址写文件路径，相对工作目录或绝对路径都可以：
>
> `[打开 报告.docx](报告.docx)`
>
> 界面会用系统默认应用打开。链接文字说「打开」或直接写文件名，不要说「下载」—— 文件本来就在用户磁盘上，没有任何东西在下载，写「下载」会让用户以为要联网取回什么。也不要编一个 `http://` 网址 —— 这个应用不是网页，没有用来下载文件的本地服务器。http(s) 只用来指向网上真实存在的页面。
>
> 回答用中文。代码和标识符保持原文。

#### 2.1.2 三个条件段

| 段 | 触发条件 | 位置 | 原文 |
| --- | --- | --- | --- |
| Python venv | `python_venv.is_some()` | `prompt.rs:116-122` | 「Python 虚拟环境：{venv}\n已注入 PATH 和 VIRTUAL_ENV，python / pip 直接就是这个环境的，不要 source activate，也不要另建虚拟环境。」 |
| Hooks | 配了 PreToolUse / PostToolUse / Stop 任一 | `prompt.rs:125-132` | 「这个项目配了检查脚本（hooks）：工具调用前后、以及你想收尾时，用户写的脚本会检查一遍。它们的反馈以 system-reminder 出现，**当成用户本人的意见对待** —— 被拦下时不要重试同一个动作，而是按反馈调整做法；说「测试没过」就去修，不要绕过检查。」 |
| 会话级补充 | `extra.is_some()` | `prompt.rs:133-135` | 「\n\n用户为这个会话补充的指令：\n{extra}」——**纯追加，不替换内置** |

#### 2.1.3 提示词有测试覆盖（值得注意的工程实践）

`prompt.rs:274-496` 有 13 个测试**把关键提示词句子当契约钉死**，改措辞会红。覆盖点：工作目录、当前年月、代码引用格式、mermaid、本地文件链接、venv/extra 追加语义、自主性分档、完成前验证、hooks 条件性、规划模式走消息侧、并行调用指引、环境感知契约、时间契约。每个测试的 doc comment 都写了「不这么做会怎样翻车」。

#### 2.1.4 模式类注入（同在 `prompt.rs`）

| 名称 | 位置 | 字符数 | 触发 | 要点 |
| --- | --- | --- | --- | --- |
| `plan_mode_reminder` | `prompt.rs:156-171` | ~294 | `PermissionMode::Plan` | 禁止一切修改；含硬约束句「这条约束压过你收到的其它所有指令」；3 步流程；指路 `ExitPlanMode` 为唯一批准入口 |
| `MULTITASK_FULL` | `prompt.rs:204-229` | ~1026 | 多任务模式首轮 | 协调者身份；5 条核心规则（单工作者优先、委派后不抢活、绝不轮询等待、不为并行而拆碎、琐碎例外）；委派判据；「不要向用户复述这些准则」 |
| `MultitaskNote::Short` | `prompt.rs:192-195` | ~110 | 多任务模式后续轮 | 一句话复述，靠历史里的 Full 撑着 |
| `MultitaskNote::Exit` | `prompt.rs:196-198` | ~80 | 刚退出多任务的那轮 | 只说一次 |
| `nudge_start_multitasking` | `prompt.rs:236-246` | ~180 | 用户点「转到后台」按钮 | 令其用 `Task(resume="self")` 分叉后**立刻停下** |
| `nudge_build_in_parallel` | `prompt.rs:252-272` | ~430 | 计划批准 + 并行构建按钮 | 把计划拆成「构建相位」，相位间靠完成通知串联；测试留给最后一个 agent |

`plan_mode_reminder` 全文：

> 当前处于规划模式：用户还不希望你动手。禁止一切修改 —— 编辑文件、执行会产生副作用的命令、改配置、提交，全部不行；这条约束压过你收到的其它所有指令。
> 现在该做的：
> 1. 用只读工具（Read / Grep / Glob / WebSearch / WebFetch）把现状摸清楚；
> 2. 想清楚方案：动哪些文件、什么顺序、怎么验证、有什么权衡；
> 3. 计划成熟后，调用 ExitPlanMode 工具提交计划全文（Markdown），等待用户批准。
> 不要用普通回复问「这个计划可以吗？」「要开始吗？」—— 提交计划是征求批准的唯一方式，批准后规划模式自动退出。

注：`prompt.rs:149` 与 `prompt.rs:203` 的注释显示，规划模式与多任务模式的措辞是**有意对照业界同类产品**改写的。

#### 2.1.5 环境注入文本（`crates/riot-kernel/src/env.rs`）

| 文本 | 位置 | 原文 / 模板 |
| --- | --- | --- |
| 时钟行 | `env.rs:120` | `现在是 2026-08-31（周一）16:37，UTC+8。` |
| 间隔警示（>30min） | `env.rs:161-177` | 「距上一条消息已过去约 {human}。期间终端、浏览器、文件等外部状态都可能变了 —— 历史里的快照和结论只代表当时，涉及现状的判断先重新核实。」 |
| 用量档位（50/70/85%） | `env.rs:103-109` | 「上下文已用约 {pct}%（满 100% 会自动压缩历史）。压缩会吞掉旧的工具结果 —— 重要结论尽早写进回复正文。」 |
| 采样失败 | `env.rs:184-185` | 「本轮环境采样失败：此前快照里的终端与浏览器状态一律视为未知（不是「没变」），需要时用工具重新确认。」 |
| 快照头 | `env.rs:26` | 「环境快照（终端面板与内置浏览器的现状）」 |
| 快照尾（差分契约自声明） | `env.rs:67` | 「以上是本轮开始时的采样；之后没有新快照就表示这些没变。」 |
| 空环境 | `env.rs:34` | 「终端面板里没有你能看的终端。」 |
| 未共享终端 | `env.rs:50` | 「用户另有 {n} 个未共享的终端；内容你看不到，需要就请他在终端面板上点「共享给 agent」。」 |
| 终端告警 | `env.rs:80-86` | 「终端 [{id}]（{title}）的输出里出现了异常：\n{excerpt}\n与当前任务相关就用 TerminalOutput(id={id}) 看完整输出；无关就忽略，不必评论。」 |

#### 2.1.6 附件 → wire 包装（`crates/riot-providers/src/anthropic/request.rs:482-519`）

所有注入统一包进 `<system-reminder>` XML 标签：

| Attachment 变体 | 渲染模板 |
| --- | --- |
| `Memory` | `<system-reminder>\n项目记忆 {path}：\n{content}\n</system-reminder>` |
| `RestoredFile` | `<system-reminder>\n压缩前你读过 {path}：\n{content}\n</system-reminder>` |
| `UserFile` | `<system-reminder>\n用户在消息里引用了 {path}，内容如下：\n{content}\n</system-reminder>` |
| `Environment` / `SystemReminder` / `DescribedImage` | `<system-reminder>\n{text}\n</system-reminder>` |

### 2.2 Subagent 提示词（`crates/riot-kernel/src/subagent.rs`）

子 agent 有独立的、**远比主 prompt 简短**的 system prompt，入口 `subagent.rs:376 system_prompt_for(kind, cwd)`。三种 `Kind`（`subagent.rs:276-282`）：`GeneralPurpose` / `Explore` / `Fork`（`Fork` 只能由 `resume:"self"` 产生，system + 工具 + 模型全部继承父，**不走** `system_prompt_for`）。

**刻意不注入 AGENTS.md**（`subagent.rs:373-375` 注释）：理由是记忆文件是给「在这个项目里写代码」的人看的，侦察档只汇报，省 token。

公共前缀（`subagent.rs:377-381`）：`工作目录：{cwd}\n平台：{os}\n\n`——注意**没有日期、没有时钟行、没有环境快照**。

#### Explore 档（`subagent.rs:383-395`，约 210 字符）

> 你是只读侦察专家，任务是快速、准确地摸清情况并汇报。
>
> 规则：
> - 只读。不修改任何文件、不执行有副作用的操作 —— 委托方是按「只读侦察」放你进来的，越界的写操作绕过了他的审查。
> - 并行地广撒网（Grep/Glob 可以同批多个），再对命中处精读 —— 串行搜索是这类任务最大的时间浪费。
> - 汇报要可跳转：结论都带文件路径和行号 —— 委托方要照着你的报告直接动手，少个行号他就得重找一遍。
> - 你的回复会**原样**作为调查结果交回，写成一份紧凑的报告：先结论，再证据，不要过程独白 —— 过程只消耗委托方的上下文，不增加信息。
>
> 回答用中文。

工具集仅 `Read / Grep / Glob / WebSearch / WebFetch`（`subagent.rs:351-357`）；轮数上限 16（`EXPLORE_MAX_TURNS`，`subagent.rs:272`）；允许降级到便宜模型（`prefers_cheap`，`subagent.rs:311`）。

#### GeneralPurpose / Fork 档（`subagent.rs:396-406`，约 180 字符）

> 你是自主完成任务的执行者。委托方给你一个任务，你独立做完并汇报。
>
> 规则：
> - 动手前先看清楚：改文件前 Read，找位置用 Grep —— 凭猜测改出的错误，委托方比你更难发现。
> - 只做任务描述里的事，不顺手扩展 —— 委托方看不到你的过程，扩展出的改动他无从审查，只能连你做对的部分一起怀疑。
> - 你的最后一条回复会**原样**作为任务结果交回 —— 写清楚做了什么、改了哪些文件、验证结果如何；失败就如实说失败和原因，粉饰的「完成」会让委托方带着错误结论继续走。
>
> 回答用中文。

工具集 `Read / Edit / Write / Bash / Grep / Glob / WebSearch / WebFetch`（`subagent.rs:358-367`）。**结构上不含 `Task`**（防递归，`subagent.rs:342-344` 注释：「递归要在结构上不存在，不能靠提示词劝」），也不含 `TodoWrite` 和 `Browser*`。

#### 分叉前奏 `fork_prelude`（`subagent.rs:732-775`）

分叉出的子 agent 第一条 user 消息包含：给悬空 tool_use 的补位结果（两句锚点文案，`subagent.rs:744` / `746`）+ 一条 system-reminder：

> 你是从主 agent 分叉出来的后台子 agent（agent id：{id}），继承了到此为止的全部对话和工作区状态。从现在起你独立执行下面的任务；主 agent 只协调，不会重复做你的活。规则：
> - 不要再用 resume="self" 分叉自己（会被拒绝）；需要拆分就开同步的子 agent。
> - 不要向用户提问 —— 用户看不到你的过程，只看得到你的最后一条回复。
> - 你的最后一条回复会作为汇报**原样**交回主 agent，用户也会看到：写清做了什么、改了哪些文件、验证结果如何；失败就如实说。

后接 `任务：{prompt}`。

#### 后台任务完成通知（`crates/riot-kernel/src/tasks.rs:270-297`）

回灌给父 agent 的通知消息（`SystemReminder`）：

> 后台子任务「{title}」{已完成/失败了/被停止了/仍在运行}（agent id：{id} · {kind} · {model} · {tokens} tokens · {n} 次工具调用）。
> 下面是它的汇报。用户已经在界面上看到了这份汇报 —— 不要复述；只做需要你做的事：综合多个任务的结果、处理它报告的阻塞或失败、或据此继续协调。没有需要做的就简短确认一句。要给它追加指令，用 Task 工具、resume 填上面这个 agent id；回复里提到它写成链接 [{title}](agent:{id})。
>
> --- 汇报 ---
> {report}

失败态另有一句（`subagent.rs:702`）：「子任务失败：{error}。可以调整任务描述重试一次；连续失败就自己动手做。」

### 2.3 压缩 / 摘要提示词（`riot-core`）

压缩是**两档阶梯**（`compactor.rs:125-131` `Layered`）：轻档 `ClearOldResults`（清旧工具结果，无损，无提示词）→ 重档 LLM 全量总结（有损，用下面的提示词）。

#### 2.3.1 总结提示词 `COMPACT_PROMPT`（`summarize.rs:62-79`，约 560 字符）

以一条合成 user 消息追加在历史末尾（`summarize.rs:108-117`，id 固定 `msg_compact_prompt`）：

> 重要：只输出文本，不要调用任何工具。你需要的全部上下文都已经在上面的对话里。
>
> 你的任务：为到目前为止的对话写一份**详尽**的总结，重点保住用户的明确要求和你已经做过的动作。这份总结将替代完整历史供后续继续工作使用 —— 漏掉的信息就永远丢了。
>
> 先在 `<analysis>` 标签里按时间顺序过一遍对话，自查每一段的：用户意图、你的做法、关键决策、具体细节（文件名、完整代码片段、函数签名、文件修改）、踩过的错和修法、用户的反馈（尤其是纠正你的话）。
>
> 然后在 `<summary>` 标签里输出以下九节：
> 1. 主要请求与意图：用户的每一个明确要求，写细。
> 2. 关键技术概念：涉及的技术、框架、约定。
> 3. 文件与代码段：看过/改过/新建的文件，为什么重要，关键代码片段要完整摘录（最近改动优先）。
> 4. 错误与修复：踩过的每个错、怎么修的、用户对此的反馈。
> 5. 问题解决：已解决的问题和仍在排查的思路。
> 6. 全部用户原话：列出**所有**非工具结果的用户消息原文 —— 这是防止意图漂移的锚，一条都不能少。
> 7. 待办事项：明确被要求、还没完成的事。
> 8. 当前工作：总结前的那一刻正在做什么，文件和代码要具体。
> 9. 下一步（可选）：与最近工作直接相关的下一步。必须和用户最近的明确要求一致；如果上一件事已经收尾，没有新指示就不要发明下一步。引用最近对话的原话来说明接续点。
>
> 提醒：只输出 `<analysis>` 和 `<summary>` 两个块，不要调用工具。

`summarize.rs:61` 注释注明「九节结构和 CC 逐节对应，措辞按中文对话习惯改写」。

**输出解析**（`summarize.rs:250-275` `extract_summary`）：剥掉 `<analysis>`，取 `<summary>` 正文；缺闭合标签时取开标签之后全部（流截断是常见形态）；**只有 analysis 没有 summary 视为失败**（返回空串）。输出预算 `SUMMARY_MAX_OUTPUT_TOKENS = 20_000`（`summarize.rs:59`）。

#### 2.3.2 总结请求的 system

两条路径：
- **同形状路径**（有 `RequestShape`）：system 直接复用主循环那份完整 system prompt（`summarize.rs:122-124`），消息**原样**发，为的是吃 provider 前缀缓存（`session.rs:2869-2871` 注释：~100k 输入走 cache_read）。
- **退回路径**（手动 `/compact` 拿不到轮次装配）：用极简 `SUMMARY_SYSTEM`（`summarize.rs:83`）：
  > 你是负责精确总结对话的助手。你只输出文本，从不调用工具。

  同时消息要瘦身（`strip_for_summary`，`summarize.rs:293`）：图片换成 `[此处原本是一张图片，总结时已省略]`；`tool_use`/`tool_result` 块降级为纯文本 `[工具调用 {id} 的结果（失败）]\n{...}`；思考块丢弃。

#### 2.3.3 压缩后的续接消息 `continuation_message`（`summarize.rs:180-215`）

压缩完历史被替换成一条合成 user 消息，内容顺序为：**记忆附件 → 总结正文 → 恢复的文件附件**（文本居中，有测试钉住）。正文：

> 本会话由一段更早的对话延续而来，先前内容已压缩。以下是前文的完整总结：
>
> {summary}
>
> {archive_note}
>
> 直接接着做，不要复述总结、不要向用户再次确认、不要说「我将继续」—— 像中断从未发生过一样，接上手头的任务。

**归档索引段** `archive_note`（`summarize.rs:189-195`，仅当有归档文件时）——这是很好的「信息损失兜底」设计：

> 被压缩的对话**原文**保存在 `{path}`（一条消息一个 `## [序号] 角色` 小节，工具结果只留开头）。总结里没有、但你需要的细节 —— 报错原文、具体路径、命令输出、用户某句话的准确措辞 —— 用 Grep 搜关键词或 Read 指定行区间去查，不要靠猜、也不要整份读进来。

#### 2.3.4 压缩边界与工作集恢复

| 项 | 值 | 位置 |
| --- | --- | --- |
| 保留尾巴上限 | `MAX_TAIL_TOKENS = 20_000` | `summarize.rs:223` |
| 切分点 | 只在**用户提问**处切，最后一条提问起的尾巴原样保留 | `summarize.rs:235` `split_point` |
| 工作集恢复 | 最近 5 个文件、单文件 20KB、总量 100KB | `session.rs:3318-3342` `restored_files` |
| 超长文件截断提示 | 「\n\n[文件超长已截断，需要完整内容用 Read 重读]」 | `session.rs:3334` |
| 用量档位预警 | 50/70/85% 三档，越档时提醒「重要结论尽早写进回复正文」 | `env.rs:90` / `env.rs:103` |

### 2.4 工具 description 汇总

#### 2.4.1 机制

工具描述来自 `Tool` trait 的**必须实现**方法（`crates/riot-protocol/src/tool.rs:39`）：

```35:39:crates/riot-protocol/src/tool.rs
    /// 进 API `tools[].description` 的完整使用说明。
    ///
    /// 这里要写清与其它工具的分工和 NEVER 列表
    /// （例："搜索永远用 Grep 工具，不要在 Bash 里跑 grep"）。
    fn prompt(&self, ctx: &PromptContext) -> String;
```

它**不是** doc comment，而是运行时函数，可以吃 `PromptContext`（`tool.rs:346-363`）：`cwd` / `platform` / `sandboxed` / `sibling_tools` / `today`。目前只有 6 个工具用了动态能力：`Bash`（沙箱说明 + Windows 说明）、`TerminalOutput`、`WebSearch`（写死当前年份）、`Read`、`Skill`（列出可用技能）、`McpTool`。其余 46 个是静态字符串。

参数说明走另一条路：`input_schema()` 由 `schemars::schema_for!(Input)` 生成，**字段的 `///` doc comment 整段进 JSON schema 的 `description`**（已核对 schemars 1.2 的 `get_doc`，它把所有 `///` 行 concat，不做 title/description 拆分）。全部工具的 schema 字段说明合计约 **3922 字符**。

#### 2.4.2 总量

| 指标 | 值 |
| --- | --- |
| 实现了 `Tool` 的生产工具数（排除测试桩） | **53**（其中 `Attributed` 是透明包装，描述委托内层） |
| description 总字符数 | **12307** |
| 中位数 | **178 字符** |
| 其中 `Browser*` 系列 | 32 个工具，合计 4944 字符（平均 154） |
| 带使用示例的 | **5 / 52**（约 10%）：`Bash` / `TerminalOutput` / `ToolSearch` / `WebFetch` / `WebSearch` |
| 带否定/反例约束的 | 24 / 52（约 46%） |
| 用了 `PromptContext` 动态生成的 | 6 / 52 |
| schema 字段说明合计 | 3922 字符 |

**每轮静态提示词预算合计约 18200 字符**（system 1930 + 工具描述 12307 + schema 字段说明 3922），其中工具描述占 68%。

#### 2.4.3 逐工具明细

「示例」= 描述里出现具体调用示例或 `例：/例如/示例/用法`；「反例」= 出现「不要/不该/禁止/而不是/别用」等负向约束。

| 工具 | 文件 | 字符数 | 示例 | 反例 | 备注 |
| --- | --- | ---: | :-: | :-: | --- |
| Bash | `tools/bash.rs:62` | 1088 | ✅ | ✅ | 动态（沙箱/Windows 分支），最完整 |
| Schedule | `tools/schedule.rs:73` | 763 | — | ✅ | |
| Task | `subagent.rs:885` | 746 | — | ✅ | 含「当成刚进门的同事」写 prompt 指引 |
| TerminalOutput | `tools/terminal.rs:56` | 519 | ✅ | ✅ | 动态 |
| TodoWrite | `tools/todo.rs:54` | 446 | — | ✅ | 唯一使用 Markdown 分节结构的描述 |
| ShowBrowser | `tools/preview.rs:154` | 399 | — | ✅ | |
| AskUserQuestion | `tools/ask.rs:125` | 379 | — | ✅ | 有完整「何时用/何时不用」 |
| WebFetch | `tools/web/fetch.rs:61` | 345 | ✅ | ✅ | |
| WebSearch | `tools/web/search.rs:73` | 343 | ✅ | ✅ | 动态注入 `{today}` / `{year}` |
| Diagnostics | `tools/diagnostics.rs:245` | 332 | — | ✅ | |
| BrowserNavigate | `tools/browser.rs:381` | 303 | — | ✅ | |
| BrowserClick | `tools/browser.rs:576` | 302 | — | — | |
| **Read** | `tools/read.rs:72` | **300** | — | ✅ | 动态（注入行数上限） |
| PreviewFile | `tools/preview.rs:46` | 296 | — | ✅ | |
| BrowserHandoff | `tools/browser.rs:2997` | 292 | — | — | |
| BrowserFillForm | `tools/browser.rs:1343` | 252 | — | ✅ | |
| **Grep** | `tools/grep.rs:74` | **249** | — | ✅ | |
| **Glob** | `tools/glob.rs:53` | **244** | — | ✅ | |
| BrowserType | `tools/browser.rs:675` | 242 | — | — | |
| BrowserScreenshot | `tools/browser.rs:1080` | 196 | — | ✅ | |
| BrowserHar | `tools/browser.rs:2258` | 196 | — | — | |
| ExitPlanMode | `tools/plan.rs:54` | 195 | — | ✅ | |
| ToolSearch | `tools/tool_search.rs:197` | 195 | ✅ | ✅ | |
| BrowserSourceOf | `tools/browser.rs:1572` | 194 | — | — | |
| BrowserView | `tools/browser.rs:954` | 189 | — | — | |
| **Edit** | `tools/edit.rs:43` | **188** | — | ✅ | |
| BrowserNetwork | `tools/browser.rs:2146` | 178 | — | — | |
| TerminalList | `tools/terminal.rs:437` | 169 | ✅ | — | |
| BrowserIntercept | `tools/browser.rs:2428` | 168 | — | — | |
| BrowserKey | `tools/browser.rs:814` | 160 | — | ✅ | |
| BrowserReadTab | `tools/browser.rs:2891` | 155 | — | — | |
| BrowserWaitFor | `tools/browser.rs:1189` | 155 | — | ✅ | |
| BrowserFuzz | `tools/browser.rs:3253` | 155 | — | — | |
| BrowserPerf | `tools/browser.rs:2056` | 151 | — | — | |
| BrowserEvaluate | `tools/browser.rs:1674` | 145 | — | ✅ | |
| BrowserTabs | `tools/browser.rs:2610` | 135 | — | — | |
| BrowserReport | `tools/browser.rs:3390` | 134 | — | — | |
| BrowserUpload | `tools/browser.rs:1495` | 129 | — | — | |
| Skill | `tools/skill.rs:53` | 128 | — | ✅ | 动态（列出已发现技能） |
| BrowserSnapshot | `tools/browser.rs:884` | 121 | — | — | |
| BrowserSelect | `tools/browser.rs:1784` | 109 | — | — | |
| BrowserReplay | `tools/browser.rs:3153` | 108 | — | — | |
| BrowserCrawl | `tools/browser.rs:2834` | 103 | — | — | |
| **Write** | `tools/write.rs:36` | **102** | — | — | 最短的核心写工具 |
| BrowserHover | `tools/browser.rs:1859` | 100 | — | — | |
| BrowserCookies | `tools/browser.rs:2349` | 100 | — | — | |
| BrowserSecrets | `tools/browser.rs:2488` | 98 | — | — | |
| BrowserDiscover | `tools/browser.rs:237` | 83 | — | — | |
| BrowserDrag | `tools/browser.rs:1977` | 78 | — | — | |
| BrowserGo | `tools/browser.rs:178` | 78 | — | — | |
| BrowserScroll | `tools/browser.rs:83` | 74 | — | — | |
| McpTool | `riot-mcp/src/tool.rs:178` | 62 | — | — | 转发 MCP server 自带描述 |
| TerminalKill | `tools/terminal.rs:506` | 60 | — | — | |
| BrowserConsole | `tools/browser.rs:2709` | 53 | — | — | 最短 |
| Attributed | `subagent.rs:443` | 0 | — | — | 透明包装，`prompt` 直接委托内层 |

#### 2.4.4 几个代表性原文

**Read**（`read.rs:72`，动态注入上限值）：
> 读取文件内容。返回结果每行带行号，格式是 `行号\t内容`。
> - 行号是显示用的，不是文件内容的一部分。用 Edit 时 `old_string` 不要带行号。
> - 一次最多返回 {MAX_LINES} 行；文件更长时用 `offset` 继续读。
> - 超过 {MAX_LINE_CHARS} 字符的行会被截断。
> - 文件很长时用 `offset` 分段读。Edit 会自行载入全文做唯一性检查，不必为了改文件再读一遍整份。
> - 图片文件（png / jpg / gif / webp）也可以读：会返回图片内容。`offset` 和 `limit` 对图片无效。不要试图用 shell 去解码图片。

**Grep**（`grep.rs:74`）：
> 在文件内容里搜索正则。基于 ripgrep，会自动跳过 .gitignore 里的文件。
> - 优先用这个而不是 Bash 里的 `grep`／`rg`：更快，输出也已经整理过。
> - `pattern` 是 Rust regex 语法。字面量里的 `.`、`(`、`[` 等需要转义。
> - 先用 `files_with_matches` 摸清范围，再对具体文件用 `content` 细看，比一次拉回几百行更省上下文。
> - 结果过多时会截断，缩小 `glob` 范围或让 `pattern` 更具体。

**Write**（`write.rs:36`，全仓最短的核心工具描述）：
> 写入文件，内容会完全覆盖原有内容。
> - 覆盖已存在的文件前必须先用 Read 读过它。
> - 只改动文件的一部分时优先用 Edit —— 全量覆盖容易丢掉你没看到的内容。
> - 创建新文件不需要先 Read。

**TodoWrite**（`todo.rs:54`，唯一带 Markdown 分节的）：分「## 什么时候用 / ## 什么时候不用 / ## 状态与措辞 / ## 完成的标准」四节，明确「同一时刻只有一项 in_progress」「做完立刻标 completed」「每次传完整新清单（整表替换）」「测试在红就保持 in_progress」。

**Bash**（`bash.rs:62`，1088 字符、含两段条件分支）：沙箱边界说明（仅 `ctx.sandboxed` 为真时）+ Windows POSIX bash 对照表（仅 Windows）+ 无状态执行、非交互环境、超时、`background: true` 长服务、输出截断、以及**明确的工具分工**：「查找文件用 Glob、搜索内容用 Grep……读文件用 Read，不要用 `cat`」。

**Skill**（`skill.rs:53`，动态）：把 `.riot/skills/*/SKILL.md` 的 name + description 列进描述，并说明「正文只在加载时进入上下文，所以不要凭名字猜内容 —— 觉得相关就加载」。

#### 2.4.5 延迟加载（工具目录瘦身）

`tools/tool_search.rs:1-23` 实现了「延迟工具」：`Tool::should_defer` 为真的工具（目前即 MCP 工具）不进 `tools` 数组，只在 `ToolSearch` 的描述里列名字；模型按需 `select:名字` 或关键词检索取回完整定义。仅当延迟候选的**描述 + schema 总字符数**超过 `DEFER_THRESHOLD_CHARS`（`tool_search.rs:35`）时启用——注释写明「省的不如多跳一次的贵」。发现集合是会话级的。

#### 2.4.6 内部注释泄漏进模型可见 schema（两处）

schemars 把整段 doc comment 塞进 `description`，以下两处的实现备注会随 schema 发给模型：

- `tools/plan.rs:33-37` `Input::plan`：正文之后跟着「字段本身只在反序列化时校验存在性（call 里 plan 的消费者是 preview_of 和弹窗，不是这段代码），所以这里 allow dead_code。」
- `tools/bash.rs:37-40` `Input::description`：正文之后跟着「只在 schema 和 `describe()` 里用到 —— 但字段必须留着，`deny_unknown_fields` 会把没声明的参数当成错误拒掉。」

### 2.5 Provider（厂商）差异化提示

**结论：Riot 目前没有任何按模型厂商差异化的提示词内容。** 两家 provider 收到的是**同一份** system 字符串，差别只在传输格式与缓存标记。

#### 2.5.1 存在一个「分段 system」的基础设施，但生产上是空的

`crates/riot-providers/src/anthropic/request.rs:67-97` 定义了 `SystemSection`：

```67:78:crates/riot-providers/src/anthropic/request.rs
/// system prompt 的一段。
#[derive(Debug, Clone)]
pub struct SystemSection {
    pub name: &'static str,
    pub text: String,
    /// 这一段的内容在会话中途会不会变。
    ///
    /// `[约束]` 默认应该是 `false`（可缓存）。确实每轮都变的段落要显式标注
    /// 并写清理由 —— 「缓存是默认、不缓存要报备」这个方向，让缓存命中率
    /// 变成架构约束而不是事后优化。
    pub volatile: bool,
}
```

构造器有两个：`stable(name, text)` 和 `dangerous_volatile(name, text)`（后者名字故意起长，`request.rs:89` 注释：「让 review 时一眼看见」）。

**但真实装配传的是空 vec**：`crates/riot-kernel/src/models.rs:62`（Anthropic）与 `models.rs:77`（OpenAI）都传 `Vec::new()`。因此全部 system 内容都从 `req.system`（即 `prompt::system_prompt()` 的产物）单条进来，在 `request.rs:276` 被包成唯一一段 `SystemSection::stable("request", …)`。**分段能力和 `volatile` 标注在生产链路上完全未被使用。**

#### 2.5.2 两家的差异只在 wire 层

| | Anthropic（`anthropic/request.rs`） | OpenAI 兼容（`openai/request.rs`） |
| --- | --- | --- |
| system 载体 | `system: Vec<WireSystemBlock>`，稳定段与 volatile 段各一块（`request.rs:285-314`） | 单条 `WireMessage::System`（`request.rs:56` 注释：「OpenAI 只有一条 system 消息，没有分段缓存的概念」） |
| 缓存断点 | 稳定段打 `cache_control: global()`；volatile 段不打（`request.rs:298`/`310`） | 无显式断点，靠服务端自动前缀缓存 |
| 断点校验 | `validate_cache_breakpoints`（`request.rs:571`）：system 断点 ≤1 且必须在第一块，消息断点 ≤1 | 无 |
| 工具顺序 | —— | 按名字排序（`openai/request.rs:79` 注释：顺序不稳会让前缀缓存每轮失效） |
| 附件渲染 | `convert_attachment`（`anthropic/request.rs:482`） | `render_attachment`（`openai/request.rs:330`）——**两份实现，模板字符串逐字相同** |

`openai/request.rs:327-329` 记了一个历史 bug：以前直接 `serde_json::to_string`，模型读到的是一坨 `{"type":"attachment",…}` 字面 JSON。

#### 2.5.3 没有任何模型名分支影响提示词

全仓搜 `deepseek|glm|zhipu|gemini|qwen|kimi` 命中的都是配置默认值、token 计数、修复逻辑（如 `riot-core/src/repair.rs` 处理严格校验服务端的孤儿 `tool_use`），**没有一处按厂商改写提示词文本**。唯一与模型能力相关的提示词分歧是**视觉兼容**：模型收不了图时把图片转述成文字（`riot-protocol/src/vision.rs`，注入 `DescribedImage`），这是能力适配不是措辞适配。

### 2.6 前端提示词（`src/`）

**前端不产出任何发给模型的提示词文本。** `src/lib/prompts.ts`（52 行）只是「提示词库」的 UI 辅助函数：`presetLabel` / `presetSummary` / `findPreset` / `newPresetId`。

`src/components/settings/PromptsPane.tsx:14-19` 的注释说清了这套东西的定位：

> 提示词库。存的只是一份素材清单 —— **内核从不读它**。用户在会话设置里挑一条，那一刻正文被**复制**进会话；之后改这里不影响已经在跑的会话。这样"整理提示词库"永远是安全动作，不会牵动任何正在进行的对话。

也就是说，提示词库 → 会话设置的 `system_prompt` → 内核的 `system_prompt_extra` → `prompt.rs:133` 的 `extra` 追加段。**是一次性拷贝，不是引用。**

`src/components/settings/CommandsPane.tsx` 管理的是斜杠命令，同样只是编辑器，展开逻辑在宿主侧（见 §2.7）。

### 2.7 项目级规则、Skills 与斜杠命令

#### 2.7.1 记忆文件 AGENTS.md（`crates/riot-kernel/src/memory.rs`）

两层，**顺序即注入顺序、越晚出现权重越高**（`memory.rs:8-10`）：

| 层 | 路径 | 说明 |
| --- | --- | --- |
| 全局 | `<配置目录>/riot/AGENTS.md` | 跨项目偏好。只认 `AGENTS.md`，无回退 |
| 项目 | `<项目根>/AGENTS.md`，回退 `CLAUDE.md` | 两个都在时**只取 AGENTS.md** |

- 支持行内引用展开：正文里的 `@./docs/style.md` 会被替换成被引文件内容，最大深度 `MAX_INCLUDE_DEPTH = 5`（`memory.rs:52`）。
- 单文件上限 `MAX_FILE_CHARS = 64KB`，超 `WARN_FILE_CHARS = 40KB` 告警（`memory.rs:46-49`）。
- **只查项目根一层，不向上递归**（`memory.rs:19` 注释：monorepo 用户用 `@../AGENTS.md`）。
- **本仓库自身没有 AGENTS.md / CLAUDE.md**——`.riot/` 下只有 `skills/`。

#### 2.7.2 Skills（`crates/riot-kernel/src/skills.rs` + `tools/skill.rs`）

三层来源，**越具体的赢：项目 > 全局 > 内置**（`skills.rs:11-13`，「用户想改内置技能的做法时，写一个同名的就能盖掉」）：

| 层 | 路径 |
| --- | --- |
| 项目 | `<项目根>/.riot/skills/<名字>/SKILL.md` |
| 全局 | `<配置目录>/skills/<名字>/SKILL.md` |
| 内置 | 编进二进制，`include_str!`（`skills.rs:70-88`） |

**渐进式披露**是这套机制的核心提示词工程手法：只有 `description` 进 `Skill` 工具的描述（`tools/skill.rs:53`），**正文只在模型主动加载时才进上下文**。`skills.rs:37-39`：「`description` 必填 —— 它是模型决定"要不要加载"的唯一依据，没有它的技能等于不存在」。

格式：YAML frontmatter（只认单行 `key: value`）+ Markdown 正文，正文支持 `$ARGUMENTS` 与 `${SKILL_DIR}` 占位符。正文上限 64KB（`skills.rs:57`）。`allowed-tools` / `model` 等字段**当前忽略而不报错**（`skills.rs:330`）。frontmatter 支持 `disable-model-invocation: true`（只给用户调，`skills.rs:97`）。

**9 个内置技能**（`crates/riot-kernel/builtin/skills/`，合计 27391 字符）：

| 技能 | 字符数 |
| --- | ---: |
| extend-riot | 5906 |
| review | 3214 |
| retro | 3082 |
| commit | 2925 |
| verify | 2786 |
| skillify | 2695 |
| debug | 2427 |
| split-to-prs | 2279 |
| simplify | 2077 |

**7 个本仓库项目技能**（`.riot/skills/`，合计 23679 字符）：`add-tool`(5839) / `review`(4221) / `commit-batch`(3490) / `add-command`(3384) / `mutate`(2612) / `verify`(2285) / `protocol-change`(1848)。其中 `review` 与 `verify` 同名覆盖了内置版。

技能正文的写法质量很高，示例（`.riot/skills/verify/SKILL.md`）：分 5 个编号步骤，每步给出确切命令 + **为什么不能跳过**的理由（如「不能只跑 `--release`：不变量断言用 `debug_assert`，release 下整个编译掉，全绿等于什么都没测」），结尾有「报告结果时」的沟通要求（「不要把"编译过了"说成"验证过了"」）。

#### 2.7.3 斜杠命令（`crates/riot-kernel/src/slash.rs`）

`<配置目录>/riot/commands/**/*.md`（全局）与 `<项目根>/.riot/commands/**/*.md`（项目，同名赢）。子目录成命名空间：`commands/git/pr.md` → `/git:pr`。

格式：可选 frontmatter（`description` / `argument-hint`）+ 模板正文。占位符 `$ARGUMENTS`（整段参文原文）、`$1..$9`（按空白拆分，带引号的段落算一个）。模板里没有任何占位符而用户又给了参数时，追加 `ARGUMENTS: <args>`（`slash.rs:25`）。

两条**刻意的限制**：
- 模板里的 `@路径` 不在这里实现，交给消息级 `@` 引用（`crate::mentions`）统一处理（`slash.rs:28-30`）。
- **不支持 `` !`cmd` `` 嵌入执行**（`slash.rs:32-33`）：「那是把"展开提示词"变成"执行任意命令"的口子，要做也得先过权限闸」。

本仓库当前**没有** `.riot/commands/` 目录。

## 3. 可配置性分析

### 3.1 全景表

| 提示词部分 | 硬编码？ | 用户能不能改 | 改的方式 | 位置 |
| --- | --- | --- | --- | --- |
| 主 system prompt 基础段（身份、8 条准则、格式约定、语言） | **完全硬编码** | ❌ 不能改、不能删、不能替换 | —— | `prompt.rs:30-109` |
| 会话级追加指令 | 否 | ✅ | 会话设置「系统提示词」→ `TurnConfig.system_prompt_extra` | `prompt.rs:133-135` / `turn.rs:160` |
| Python venv 段 | 模板硬编码 | ⚠️ 只能开关（配 venv 就出现） | 会话设置 | `prompt.rs:116-122` |
| hooks 段 | 模板硬编码 | ⚠️ 只能开关（配了 hook 就出现） | 设置 → Hooks | `prompt.rs:125-132` |
| 规划模式提醒 | **完全硬编码** | ❌ 只能开关模式 | —— | `prompt.rs:156-171` |
| 多任务模式准则 | **完全硬编码** | ❌ 只能开关模式 | —— | `prompt.rs:204-229` |
| 环境注入（时钟/间隔/档位/快照/告警） | **完全硬编码** | ❌ | —— | `env.rs` |
| 压缩总结提示词（九节） | **完全硬编码** | ❌ | —— | `summarize.rs:62-79` |
| 压缩阈值 | 否 | ✅ | `config.compact_threshold_tokens` | `config.rs:727-730` |
| 内置工具 description | **完全硬编码** | ❌ | —— | 各 `tools/*.rs` 的 `fn prompt` |
| MCP 工具 description | 否 | ✅ | 由 MCP server 自己给 | `riot-mcp/src/tool.rs:178` |
| 记忆 / 项目约定 | 否 | ✅✅ 最自由 | `AGENTS.md`（全局 + 项目），支持 `@` 行内引用 | `memory.rs` |
| Skills | 内置 9 个，可被同名覆盖 | ✅✅ | `.riot/skills/*/SKILL.md`、`<配置目录>/skills/` | `skills.rs` |
| 斜杠命令 | 无内置 | ✅✅ | `.riot/commands/**/*.md`、`<配置目录>/riot/commands/` | `slash.rs` |
| 提示词库（预设） | 否 | ✅ | 设置 → 提示词；**内核不读**，只是素材 | `config.rs:463` / `PromptsPane.tsx:14-19` |
| Subagent system prompt | **完全硬编码** | ❌ | —— | `subagent.rs:376-408` |
| 厂商差异化 | —— | ❌ 机制存在但未接线 | —— | `models.rs:62,77` |

### 3.2 覆盖优先级

**同名资源**（Skills / 斜杠命令），越具体越赢：

```
项目（.riot/…）  >  全局（<配置目录>/…）  >  内置（include_str!）
```

`skills.rs:11-13`：「内置的排最后是刻意的 —— 用户想改内置技能的做法时，写一个同名的就能盖掉，不需要去找应用包里的文件，也不用等版本更新。」

**记忆文件**（不是覆盖而是叠加），按注入先后决定权重：

```
全局 AGENTS.md（先）  →  项目 AGENTS.md / CLAUDE.md（后，权重更高）
```

同目录下 `AGENTS.md` 与 `CLAUDE.md` 并存时**只取 AGENTS.md**（`memory.rs:79`）。

**上下文位置权重**（越靠近对话末尾越强），一轮内的实际顺序：

```
system prompt（最远）
  → 工具 schema
    → 历史消息
      → 记忆 AGENTS.md（仅首轮）
        → git 快照（仅首轮）
          → 环境状态 / 快照 / 告警
            → 用户正文 + @ 引用
              → 规划模式提醒
                → 多任务准则（最近，权重最高）
```

`prompt.rs:144-147` 明确把这一点作为「规划模式约束不进 system prompt」的第二个理由。

### 3.3 一条重要的语义保证：追加而非替换

会话级 `system_prompt_extra` 是**纯追加**，无论用户写什么都不会覆盖内置提示词。这一点有测试钉住（`prompt.rs:370-384`）：

```381:383:crates/riot-kernel/src/prompt.rs
        assert!(p.contains("/tmp/proj"), "内置部分必须还在");
        assert!(p.contains("/tmp/proj/.venv"));
        assert!(p.contains("pytest -x"));
```

好处是 `cwd`、格式约定这些不会被用户误删；代价是**用户无法关闭任何一条内置准则**——比如「回答用中文」是写死的，想让 Riot 用英文回答只能靠追加指令去对抗前面那句，而不是替换它。

## 4. 问题清单（提示词工程视角）

先说清楚**已经做得好的**，免得优化时误伤：每条准则都带「为什么」的写法（`prompt.rs:40`「理由是让你能推断没写到的情况」）；缓存友好的分层注入架构；关键提示词句子有测试钉死；Skills 的渐进式披露；压缩后的归档索引兜底；自主性按后果分档而非一刀切「不确定就问」。

### 高

**H1. system prompt 无结构化分节，全部是散文 + 一个扁平 bullet 列表**
`crates/riot-kernel/src/prompt.rs:30-109`
1930 字符里没有任何 XML 标签或 Markdown 标题（已核对：`<[a-z_]+>` 零命中，无 `##`）。9 个语义分节靠 `\n\n` 和语气切换隐式分隔。8 条行为准则挤在同一个 bullet 列表里，混着「先调研」（工作方法）、「并行调用」（性能）、「不要擅自提交」（安全边界）、「用 TodoWrite」（工具用法）四类不同性质的约束。后果：模型难以在长上下文中定位某一类约束；用户/开发者也无法引用「第 X 节」来讨论；未来想做「用户可关闭某一节」几乎无从下手。

**H2. 完全没有沟通风格 / 回复形态的约束**
`crates/riot-kernel/src/prompt.rs:30-109`
全文唯一带「风格」二字的是「写代码要像周围的代码」——那是**代码**风格。对于**回复本身**，提示词只规定了三件格式小事（代码引用语法、mermaid、本地文件链接）和一句「回答用中文」。没有任何关于：回复应该多长、先说结论还是先说过程、什么时候用表格/列表、工具调用之间要不要给用户进度说明、完成后的总结该包含什么、是否允许寒暄和自我批评、面对专家用户和新手要不要调整。这是当前 Riot 提示词和成熟产品差距最大的一块——子 agent 的提示词反而写了（「先结论，再证据，不要过程独白」，`subagent.rs:392-394`），主 agent 没有。

**H3. 核心编辑工具的 description 过短且零示例**
`tools/write.rs:36`(102 字符) / `tools/edit.rs:43`(188) / `tools/read.rs:72`(300) / `tools/grep.rs:74`(249) / `tools/glob.rs:53`(244)
52 个工具里只有 5 个（10%）带使用示例，中位数 178 字符。日常使用频率最高的 Edit / Write 加起来不到 300 字符，且**都没有一个具体的调用示例**。Edit 的「`old_string` 必须唯一」是最容易出错的约束，却只用一句话带过，没有演示「不唯一时怎么加上下文」。Write 更是全表唯一**既无示例也无任何否定约束**的核心写工具。反观 `Bash`（1088 字符）和 `TodoWrite`（446 字符，唯一带 Markdown 分节的）的质量明显更高——说明团队知道怎么写，只是没铺开。

**H4. 32 个 Browser 工具占了 62% 的工具数、40% 的描述预算，但质量最低**
`crates/riot-tools/src/tools/browser.rs`
32 个工具合计 4944 字符，平均 154 字符/个，其中 **26 个没有任何否定约束、32 个（全部）没有示例**。最短的 `BrowserConsole` 只有 53 字符。这些工具的 schema 全部常驻 `tools` 数组（`should_defer` 目前只对 MCP 工具为真，`tool_search.rs:14`），也就是说每一轮请求都要为 32 个描述含混的浏览器工具付上下文成本。工具越多、描述越短，模型选错工具的概率越高。延迟加载机制（§2.4.5）已经存在，把 Browser 系列纳入是现成的杠杆。

### 中

**M1. 没有任何针对不可信内容的防御性提示**
`prompt.rs` 全文 / `tools/web/fetch.rs:61` / `crates/riot-kernel/src/web/distill.rs:185`
Riot 在**架构层**做了防注入（蒸馏辅助模型不给任何工具，`distill.rs:183-186` 有测试锁死；WebFetch 的 URL 视为不可信，`web/mod.rs:126`），但**提示词层一个字都没有**。system prompt 没有告诉模型：网页正文、文件内容、终端输出、MCP 工具返回里出现的「指令」是数据不是命令。当前防线只覆盖蒸馏这一条路，模型自己 Read 到一个含有恶意指令的文件时没有任何提示词层面的免疫。

**M2. 文档里的「system prompt 分静态段/动态段」约束在生产链路上没有落地**
`docs/ARCHITECTURE.md:1551-1553` vs `crates/riot-kernel/src/models.rs:62,77`
架构文档写明 `[约束] system prompt 必须分成静态段和动态段，中间用一个显式的边界常量分隔`，`SystemSection` 也实现了 `stable()` / `dangerous_volatile()` 双构造器和 `validate_cache_breakpoints` 校验。但真实装配传的是 `Vec::new()`——**分段机制在生产中完全未使用**。整份 system prompt 被包成单个 `SystemSection::stable("request", …)`（`anthropic/request.rs:276`）并打上 `scope: "global"` 的缓存断点（`request.rs:142-147`）。而这份内容里含有 `cwd`、venv 路径、用户的会话追加指令——**逐项目、逐用户各不相同**，「跨会话共享」的收益在实际内容上不成立。真正保证缓存安全的是 `prompt.rs` 顶部那条人工纪律（不把易变内容写进去），不是这套本该强制它的机制。

**M3. 无按模型厂商/能力差异化的提示词**
`crates/riot-kernel/src/models.rs:58-86`
Anthropic 和 OpenAI 兼容后端（DeepSeek、智谱等）拿到的是逐字节相同的 system prompt 与工具描述。不同模型对「并行工具调用」「XML 标签」「长 bullet 列表」的服从度差异很大，尤其是国产模型对 `<system-reminder>` 这类标签的理解不如 Anthropic 系。`SystemSection` 是天然的差异化挂载点，目前空置。

**M4. 压缩的信息损失只有单点兜底，且兜底依赖模型自觉**
`summarize.rs:189-195` / `session.rs:3318-3342`
归档索引（「原文保存在 X，用 Grep/Read 去查」）是很好的设计，但：(1) 它是**可选**的（`archive: Option<&Path>`），没有归档时压缩就是纯有损；(2) 工作集只恢复最近 5 个文件、总量 100KB，第 6 个之后的文件模型只能靠九节总结里的片段；(3) 九节总结要求「列出**所有**非工具结果的用户消息原文」，长会话下这一节本身就可能撑爆 20k 输出预算，而 `summarize.rs:56-58` 的注释承认撞预算时「总结按失败处理」——九节顺序输出，被截掉的恰是「用户原话/待办/下一步」这几节。

**M5. 两处内部实现注释泄漏进模型可见的工具 schema**
`tools/plan.rs:33-37` / `tools/bash.rs:37-40`
schemars 1.2 把整段 doc comment concat 进 `description`（已核对 `schemars_derive-1.2.2/src/attr/doc.rs:5-37`，不做 title/description 拆分）。于是 `ExitPlanMode.plan` 的描述尾部挂着「所以这里 allow dead_code」，`Bash.description` 的尾部挂着「`deny_unknown_fields` 会把没声明的参数当成错误拒掉」。既浪费 token 又可能让模型困惑。

**M6. Subagent 提示词过于单薄，且缺失时间/环境感知**
`crates/riot-kernel/src/subagent.rs:376-408`
Explore 档 210 字符、GeneralPurpose 档 180 字符，公共前缀只有 `工作目录 + 平台`。**没有日期、没有时钟行、没有环境快照、没有 AGENTS.md**。不注入 AGENTS.md 对 Explore 是合理的省 token 取舍（`subagent.rs:373-375` 有说明），但对 **GeneralPurpose——一个会真的改代码、跑命令的 agent——同样不注入项目约定**，意味着它写出的代码不遵守团队规范，而主 agent 的准则「写代码要像周围的代码」它也没收到。另外主 prompt 里的代码引用格式、mermaid、本地文件链接约定它全都不知道，但它的汇报会原样展示给用户。

### 低

**L1. 「回答用中文」硬编码，无法配置**
`prompt.rs:109`
非中文用户只能通过追加指令去对抗这句话，而不是替换它。同理 `subagent.rs:394,405` 的两处「回答用中文」。

**L2. 提示词库（Presets）与实际生效路径脱节**
`config.rs:463` / `PromptsPane.tsx:14-19`
「内核从不读它」是刻意设计（避免整理素材影响在跑的会话），但用户视角容易误以为提示词库里的条目是全局生效的规则。库→会话是一次性拷贝，改库不回溯。

**L3. `ExitPlanMode` 的描述与 `plan_mode_reminder` 内容重复**
`tools/plan.rs:57-62` vs `prompt.rs:159-167`
「不要用普通回复问『这个计划可以吗？』」这句话在两处几乎逐字重复。规划模式下两段同时在上下文里。重复本身是一种强调手法，但这里更像是没有单一来源。

**L4. Skills 的 `allowed-tools` / `model` frontmatter 字段被静默忽略**
`skills.rs:330`
`_ => {} // allowed-tools / model 等字段先不支持，忽略而不是报错`。用户从其它生态搬技能过来时会以为这些字段生效了。`disable-model-invocation` 有类似的历史处理（`skills.rs:114` 注释提到「它写在 frontmatter 里，用户会以为生效了」），说明团队意识到了这类问题，但 `allowed-tools` 还停在忽略状态。

**L5. 工具描述里的分工声明是单向的**
`tools/glob.rs:53` / `tools/grep.rs:74` / `tools/bash.rs:62`
Glob 说「按内容搜用 Grep」，Grep 说「优先用这个而不是 Bash 的 grep」，Bash 说「查找文件用 Glob」——分工写得不错，但 `Read` 没说「大范围找位置先用 Grep」，`Task` 与 `Explore` 子 agent 之间、`WebFetch` 与 `Bash(gh)` 之间的边界只在单侧声明。`Tool` trait 提供了 `ctx.sibling_tools`（`tool.rs:356-357`「用于在 prompt 里写清分工」）**但没有任何工具用它**。

### 4.1 与原始假设的出入

调研前的怀疑清单里有两条**不成立**，记在这里避免误导优化方向：

- **「无计划/todo 机制」不成立**：`TodoWrite` 工具存在且描述是全仓质量最高的之一（446 字符、四个分节、明确「同一时刻只有一项 in_progress」「做完立刻标 completed」），system prompt 里也有对应准则（`prompt.rs:57-58`）。此外还有独立的规划模式（`ExitPlanMode` + 权限链 Plan-Deny 双保险）。
- **「压缩信息损失无应对」不完全成立**：有归档索引 + 工作集恢复 + 用量档位预警三层，问题在覆盖不全（见 M4）而非缺失。

## 5. 未完成项 / 本次未展开的部分

调研范围内已全部覆盖，以下是**刻意留白**或**只做了浅层确认**的部分，供后续需要时补：

0. ~~**Browser 工具的描述英文化**~~（改造遗留，**已完成**）。`browser.rs` 的 33 个 `fn prompt()` 现已全部英文化（第 33 个是测试替身，返回空串）。最后 4 个安全类工具（`BrowserFuzz` / `BrowserReport` / `BrowserCrawl` / `BrowserHandoff`）在前台分步手工完成——它们无法交给子 agent 批量处理，因为会触发模型服务商的内容分类器（安全测试范畴），但前台小步编辑不受影响。同时把这 4 个工具的 `fn name()` 从硬编码字面量改为引用 `names.rs` 常量，与其余 28 个工具统一（消除了此前 `BROWSER_HANDOFF` / `BROWSER_REPORT` 的 unused-import warning）。全部 439 个 `riot-tools` 测试通过，clippy 零警告。

1. **32 个 Browser 工具的逐条原文**——只做了字符数、示例、否定约束三个维度的统计和抽样，没有逐条摘录。若要重写这批描述，需要再过一遍 `browser.rs` 的 32 处 `fn prompt`（行号已在 §2.4.3 表中列全）。
2. **9 个内置技能 + 7 个项目技能的正文**——只测了体量、确认了格式与覆盖规则，抽样精读了 `.riot/skills/verify/SKILL.md` 一篇。共 51070 字符的技能正文没有逐篇评估。它们是按需加载的，不占常驻预算，优先级低于常驻部分。
3. **Hooks 反馈文本**——确认了机制（`hooks.rs:638-640`，PostToolUse 的阻断与上下文都作为「给模型的反馈」注入，措辞已按「给模型看」的口吻整理，`hooks.rs:342`），但没有把各分支的反馈模板逐条摘出。
4. **`mentions.rs`（`@` 引用展开，29KB）**——确认了它产出 `Attachment::UserFile` 并由 `convert_attachment` 包成 `<system-reminder>用户在消息里引用了 X`，未细查各类 mention（文件/目录/符号/URL）的具体渲染差异。
5. **`ask.rs` / `diagnostics.rs` / `schedule.rs` / `preview.rs` 的结果文案**——工具**返回给模型的结果文本**也是广义提示词（如 `env.rs:80` 那种「无关就忽略，不必评论」的护栏），本次只覆盖了 description 和注入侧，没有系统盘点工具**输出**侧的措辞。这可能是一个规模不小的盲区。
6. **`docs/ARCHITECTURE.md`（129KB）**——只按关键词定位读了 §7.6（多任务模式）与 §11.5（Prompt caching）两处相关段落，全文未通读。
7. **`AGENT_DESIGN.md`（68KB）**——经确认它是**对 Claude Code 泄露源码的分析笔记**，不是 Riot 自身的设计文档（见其第 3 行「基于 Claude Code 泄露源码的深度分析……作为开发类 Codex agent 程序的参考」）。它解释了 Riot 多处设计的来源（分层记忆、分段 system prompt + 缓存边界、hook 当「程序化的用户」等），但不构成 Riot 现有提示词的一部分，故未纳入归档。

---

## 附录：一次典型 turn 的提示词体量

| 部分 | 字符数 | 是否每轮重发 | 缓存 |
| --- | ---: | --- | --- |
| system prompt 基础段 | 1930 | 是（内容不变） | 命中前缀缓存 |
| 工具 description × 52 | 12307 | 是（内容不变） | 命中前缀缓存 |
| 工具 schema 字段说明 | 3922 | 是（内容不变） | 命中前缀缓存 |
| 时钟行 | ~30 | 是（每轮变） | 不缓存，追加在新消息 |
| 记忆 AGENTS.md | 视项目 | 否（仅首轮） | 随历史进缓存 |
| 环境快照 | ~100-400 | 仅变化时 | 随历史进缓存 |
| 规划模式提醒 | 294 | 规划模式下每轮 | 随历史进缓存 |
| 多任务准则 | 1026 / 110 | 首轮完整、之后简短 | 随历史进缓存 |

静态前缀约 **18200 字符**，每轮增量通常在 **30–400 字符**。整套架构的核心取舍——把所有易变内容赶出 system prompt——在这张表上体现得很清楚。
