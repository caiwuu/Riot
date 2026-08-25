# Windows 沙箱设计

> 状态：**已落地**（M1–M5，见 §7）。macOS 侧同样已落地
> （`riot-runtime/src/sandbox.rs`，seatbelt）。本文是 Windows 那块的方案
> 与取舍记录，读者是维护者。通用背景（为什么策略层不够、沙箱换来什么）
> 见 ARCHITECTURE §9.6 和 sandbox.rs 的模块文档，这里不重复。
>
> `[约束]` 沙箱的可写集里有几处「在边界之内、却能换来边界之外执行权」的
> 目标（`.git/hooks/`、`.riot/hooks.json`、`~/.cargo/config.toml`）。它们
> 由**策略层**排除在放宽档之外，不是靠这里的边界 —— 理由见 ARCHITECTURE
> §9.6.1。改这份文档里的可写集之前先读那一节。

## 0. 要对齐的现状（改这里之前先读）

macOS 版定下的形状，Windows 版**必须**装进同一套接口：

```text
SandboxMode(config) → SandboxPolicy{Off, WorkspaceWrite{writable, allow_network}}
    → activate() → Option<ActiveSandbox>     # None = 这台机器做不到
    → SandboxedRunner(装饰 ProcessRunner)     # 命令真正被关进边界
    → ctx.sandboxed = sandbox.is_some()       # 决策链据此放宽（bash::decide）
```

`[约束]` **`activate()` 返回 `None` 时 `sandboxed` 必须保持 false。**
这条在 macOS 版是「sandbox-exec 不在就别谎报」；Windows 版的失败面更宽
（打标签失败、token 创建失败、FAT32 无 ACL），每一种都要走同一条退路：
不激活、决策链回到逐条询问、界面标注「沙箱未生效」。谎报的后果是
决策链放行一批本该问人的命令，而且悄无声息。

## 1. 机制选型：Restricted Token + Low IL，不是 AppContainer

`[取舍]` Windows 有两条现成路线，选**受限令牌 + 低完整性级别（Low IL）**：

| | Low IL（选它） | AppContainer |
|---|---|---|
| 默认语义 | 允许为默认，**写**被 MIC 拦（no-write-up） | **拒绝**为默认，读写都要显式授权 |
| 与 macOS 档对齐 | 恰好等于 seatbelt 的 `allow default` + `deny file-write*` | 语义相反 |
| 读系统工具链 | 不受影响（读不设 no-read-up） | 要给 rustup / node / python / 系统 DLL 逐个授权，漏一个就是一次没头绪的失败 |
| 网络 | 不隔离 | 缺省断网（capability 才开） |
| 先例 | Chromium 旧沙箱、IE 保护模式 | Chromium 新沙箱、UWP |

决定性的理由是第三行：agent 跑的是**任意开发命令**，读白名单没法枚举 ——
`cargo build` 要读 `~/.rustup`，`npm` 要读全局 node，Python 要读
site-packages，每台机器还都不一样。AppContainer 下漏授权的表现是
「编译莫名其妙失败」，用户的第一反应就是把沙箱关掉 —— 那这层就白做了。
Low IL 的失败面只有写，而写白名单是我们本来就要维护的那张
（`SandboxPolicy::workspace_write` 的 writable 列表）。

顺带的收获：Low IL 连 **HKCU 注册表写**一起挡了（Run 键、文件关联这类
持久化面），这是 macOS 档没有的一层。

`[约束]` AppContainer 不删除，记为 V2 的 `WorkspaceWriteNoNet` 候选 ——
它缺省断网的特性正好是那一档要的。V1 不做，见 §4。

## 2. 写授权：给可写目录打 Low 完整性标签

MIC 的规则是「进程 IL < 对象 IL 时拒写」。对象缺省 Medium，所以 Low
进程默认哪儿都写不了；给目录打 **Low 标签**（SACL 里的
`SYSTEM_MANDATORY_LABEL_ACE`，SID `S-1-16-4096`，带容器继承）之后，
Low 进程就能写它 —— 这就是 writable 白名单的落地方式。

```text
激活序列（经 LabelRegistry，逐目录）：
1. 记账落盘 <config>/sandbox-labels.json（目录 + 打标时间）
2. SetNamedSecurityInfoW 打 Low 标签（首个引用才真打，已有引用只 +1）
3. CreateRestrictedToken（去特权组、Low IL）
4. CreateProcessAsUserW + 挂进 Job Object（复用现有进程树清理语义）
任何一步失败 → 退回本次引用（归零即撤标签）→ activate() 返回 None
```

`[取舍/已实现]` 清单**只记路径 + 打标时间，不记原 SACL**。因为只对
「当前是默认完整性（无显式 label）」的目录打 Low 标签，回滚 =
写空 label ACL 回到默认，没有"原状"要保存。这把"记录原状"
简化成了"记录我动过谁" —— 清单逻辑见 `sandbox_labels.rs`
（`LabelLedger`，跨平台可测），打/去标签见 `sandbox_win::label`。

`[约束/已实现]` 上一条的**前提是打标签前先体检**，`sandbox_win::label::
current_label_rid` 读回目录当前的 mandatory label：没有（默认完整性）才
打；已经是 Low 的放行（要么是我们上次崩溃的残留、要么本来就等于要设的值，
重打幂等，不然一个收不掉的残留会把沙箱永久卡死）；**任何别的级别一律
拒绝**，整条激活降级成不隔离。少了这一步，`untag` 会把用户原有的标签
抹掉，而清单里没有任何信息能还原它 —— 那正是"只记路径"这个简化会翻车的
唯一情形。读 label 只要 `READ_CONTROL`，不需要 `SE_SECURITY_NAME`
（那是审计 ACE 才要的特权），普通用户下做得了。

`[约束]` `SetNamedSecurityInfoW` 对容器对象会把可继承 ACE **传播到已有
子对象**。`~/.cargo` 的 registry 缓存动辄十万文件，所以沙箱按**会话**
激活一次、跨轮复用（`Session::active_sandbox`），不是每轮打一次撤一次；
`WinSandbox::drop` 里的归还也挪进 `spawn_blocking`，别把 tokio 的工作
线程堵在一棵大树上。

`[约束/已实现]` **标签按进程级引用计数管理**（`LabelRegistry`）。沙箱是
每轮对话激活一次的，而多会话共享一个内核进程（ARCHITECTURE §2.4），
writable 里有跨会话共享的目录（同项目工作区、`~/.cargo` 这类缓存）——
各自打/撤的话，会话 A 一轮结束就把 B 正用着的目录撤了标签，B 的 Low
进程构建到一半 ACCESS_DENIED。首个引用打标签、归零才撤；注册表同时是
清单的**单写者**（全部记账在同一把锁里），否则两个会话并发激活时全量
覆盖写会互相盖掉记录（lost update），崩溃后清单缺的那条就是永远收不到
的孤儿。记账顺序是**先记账、再打标签**：两步间崩溃留下的是"记了账没
打成"，回收空撤一次无害；反过来是"打了没记"，回收永远找不到。

`[约束/已实现]` writable 只收**存在的目录**（`dedup_existing` 过滤）：
`SetNamedSecurityInfoW` 对不存在的路径直接失败，而授权全有或全无 ——
一条不存在的缓存路径（没装 Rust 的机器上的 `~/.cargo`）就会让沙箱永远
激活不了。缓存表按平台各一张：Unix 系工具在 `~` 下建点目录，Windows
的 npm/pip/pnpm 走 `%LOCALAPPDATA%`；主目录读 `USERPROFILE`（`HOME`
是 Unix 约定，GUI 启动的进程环境里没有）。

`[约束]` **temp 不给系统 `%TEMP%` 打标签**，在其下建 `riot-sbx-<会话>`
子目录打标签，并给沙箱进程重写 `TMP`/`TEMP` 环境变量指过去。给全局
temp 打标签影响的是整台机器上所有 Low 进程的攻击面，为一个会话动
全局状态不值得。macOS 版直接放开 `/tmp` 是因为 seatbelt 的授权只对
被包的那个进程生效 —— 标签是**对象**属性，对所有进程生效，两边的
授权模型不同，不能照抄。

`[约束/已实现]` 标签是持久的文件系统状态，**必须有孤儿回收**：正常退出
时归还引用（归零即撤）；崩溃留下的残留由下次内核启动的
`recover_orphan_labels` 按 `sandbox-labels.json` 兜底（对照
process_lifecycle 的哲学：无论怎么死，别往机器上漏东西）。回收只在
**独占**拿到 `sandbox-labels.lock` 时动手 —— 拿不到说明同机还有另一个
内核活着，它的会话可能正引用这些标签，批量撤等于踩它（锁柄持有到进程
退出）。撤失败/目录被占的账**保留**，下次启动再试。残留标签的实际危害
很小 —— Medium 用户进程写 Low 目录不受 MIC 约束，只是「其它 Low 进程
也能写它」—— 但小不是零，要收。双开内核时激活/释放的跨进程互踩是
接受的残余风险（计数在各自进程内存里），正常部署一宿主一内核。

`[约束]` 打标签失败的常见现场要逐个确认过再发布：非 NTFS 卷（FAT32/
exFAT 没有 ACL）、OneDrive 重定向的用户目录、企业组策略锁 SACL、
非管理员账户改共享目录。任何一种 → activate None，不硬闯。

## 3. spawn 集成：这是工程量最大的一块

macOS 的 `wrap()` 只改 argv（前面垫 `sandbox-exec -p`），进程还是
`SystemProcessRunner` 起的。**受限 token 改的是 spawn 本身**，
`tokio::process` / `std::process` 都不暴露「用这个 token 起进程」——
所以 Windows 版的 `SandboxedRunner` 不是装饰 spec，而是**平台特化的
执行器**：自己调 `CreateProcessAsUserW`，管道接 stdout/stderr，
超时/取消/进程组语义对齐 `riot-runtime/src/proc.rs` 现状。

```text
riot-runtime/src/sandbox.rs        # 策略与激活（跨平台形状不变）
riot-runtime/src/sandbox_macos.rs  # wrap-argv 实现（现 sandbox.rs 主体迁入）
riot-runtime/src/sandbox_win.rs    # token + spawn 实现（新）
```

依赖已备好：workspace 里的 `windows` crate（`Win32_System_Threading`
已启用，再加 `Win32_Security` 一族）。Job Object 的用法可以照抄
process-wrap 的实现，但**不能直接用 process-wrap** —— 它包装的是
std/tokio 的 Command，接不进自定义 token。

`[约束]` CreateProcess 系的坑（宿主 spawn 点已经踩过一遍的）这里全部
适用：`CREATE_NO_WINDOW`、句柄继承白名单、命令行引号规则
（`CommandLineToArgvW` 的反解语义）。写实现前先读 riot-runtime 现有
spawn 代码的注释。

### 3.1 句柄继承：两个方向，只治得了一个

`CreateProcessAsUserW` 必须 `bInheritHandles=true`（stdio 要传进去），
而它默认继承**本进程当前所有可继承句柄**。两个方向各有毛病，代价不同：

| 方向 | 后果 | 手段 |
|---|---|---|
| 别人的句柄漏进**我们的**子进程 | ① 继承到另一条 spawn 还没关的管道写端 → 它等不到 EOF；② **沙箱漏洞**：MIC 只在 `open` 时检查，一个继承来的、指向 Medium 对象的可写句柄，Low 进程照样能拿它写 | `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` 显式白名单，彻底治掉 ✅ |
| 我们的写端漏进**别人的**子进程 | 我们的读端等不到 EOF，命令挂到超时 | 治不了（见下），靠收输出的宽限期兜底 ✅ |

第二行治不了，因为白名单里的句柄**必须**是可继承的 —— CreateProcess
期间那两个写端就是敞着的，而同进程里 `std`/`tokio` 的 spawn（hooks、
MCP、非沙箱会话、终端面板）一律 `bInheritHandles=true` 且不带白名单。

`[约束]` 所以 `create()` 里那把 `SPAWN_LOCK` **只解决一半**：它让我们
自己的并发 spawn 不互相偷句柄（最常见的情形），对进程里别的 spawn 点
无能为力。真正的兜底是收输出时的宽限期（`DRAIN_GRACE`）：杀完组之后只
等有限时间，到点就把已经攒下的输出交出去。最坏情况从"整条命令挂死到
超时"降级成"丢几行尾巴"。这也是为什么读任务往共享缓冲里写、而不是靠
`JoinHandle` 的返回值 —— `spawn_blocking` 卡在 `ReadFile` 上取消不了。

### 3.2 清理：所有句柄归 RAII

`[约束]` 进程、Job、管道端全部由带 `Drop` 的结构独占（`spawn::Child` /
`OwnedHandle`），**包括「外层 future 中途被 drop」那条路径**。调度器用
`FuturesOrdered` 内联持有工具 future，用户按一次停止就把整批丢掉；
`proc.rs` 那条路径有 `kill_on_drop(true)` 兜底，这条路径只有 Drop。

漏掉它的三种表现（都发生过或差点发生）：future 被丢弃后 Job 句柄不关 →
`KILL_ON_JOB_CLOSE` 不触发 → 子进程活到关机；收输出报错早退 → 后面的
`CloseHandle` 跑不到；超时分支里主流程关句柄而 waiter 线程还卡在
`WaitForSingleObject` 上 → 未定义行为，且句柄值可能已被另一条 spawn
复用。最后一条另外靠进程句柄是 `Arc`：waiter 持同一份所有权，最后一个
放手的才真关。

`Child::drop` 里那次 `TerminateJobObject` 还兼着一个作用：三个
`spawn_blocking` 线程都取消不了，只能靠"让它们等的东西真的发生"来收 ——
waiter 等到进程退出，两个 drain 等到管道 EOF。

## 4. 档位映射

| SandboxMode | macOS | Windows V1 |
|---|---|---|
| `Off` | 不激活 | 不激活 |
| `WorkspaceWrite` | seatbelt：写白名单 + 联网 | Low IL：写白名单 + 会话 temp 子目录 + 联网（MIC 不管网络，联网天然放开）✅ |
| `WorkspaceWriteNoNet` | seatbelt + `(deny network*)` | **不激活**（activate None → 逐条询问）✅ |

`[取舍]` `NoNet` 档在 Windows V1 诚实降级，不做假隔离。断网的现实选项
都太重：WFP 过滤要驱动或管理员、防火墙规则污染全局配置、AppContainer
则整个换授权模型。降级的代价是这一档在 Windows 上退回「每个写操作
询问」—— 慢但不撒谎。界面在模式选择处标注「此档在 Windows 暂不隔离
网络，将退回逐条询问」。V2 若做，AppContainer 是首选（见 §1）。

## 5. 决策链与界面：零改动

`ctx.sandboxed` 的语义、`bash::decide` 的放宽分支、`DecisionReason::Sandbox`、
session.rs 里「请求了沙箱但没激活 → System 消息告知」的降级提示 ——
全部照用。这正是当初把接口按「能替换」留好的回报，实现时**不要**在
这些层加 Windows 特判。

## 6. 验证计划

不需要本地 Windows 机器：**GitHub Actions 的 windows runner 就是真机**，
CI 驱动开发（mac 上写、CI 上验）。改完先在本机跑
`scripts/check-windows-sandbox.sh` —— 它拿一个只依赖 `windows` crate 的
隔离壳 include 真实源码、`clippy --target x86_64-pc-windows-msvc`
（整包 check 在 mac 上跑不动，见 §7 实施注记）。它挡的是 FFI 签名写错、
`cfg(windows)` 分支里类型不匹配这类**在 mac 上改代码时完全看不见**的错；
运行时行为仍以 CI 为准。

必备用例，全部对照 macOS 版 `工作区外的写被内核拒绝` 的「真跑」哲学：

1. ✅ **边界真跑**：工作区内写成功、工作区外写被拒
   （`sandbox.rs::windows_经装配路径的沙箱边界`，走完整装配；
   `sandbox_win::e2e_tests` 另有一份手动串底层的）；
2. ✅ **HKCU 写被拒**：`reg add HKCU\...` 非零退出 —— Windows 版多出来
   的持久化防线（同上那条集成用例里）；
3. ✅ **temp 重写**：`TMP`/`TEMP`/`TMPDIR` 三个都指向会话子目录且可写
   （`TMPDIR` 不能漏：Bash 工具跑的是 Git for Windows 的 bash，MSYS
   那套先看它）；
4. ⬜ **降级诚实**：FAT32 卷（CI 上 `subst` 一个 VHD）→ activate None，
   `ctx.sandboxed == false`。**还没做** —— 需要 CI 上造卷，目前只有
   `sandbox_labels` 用假 labeler 验了"打标签失败 → 整体回滚"的编排；
5. ✅ **标签体检与回收**：`label::tests` 验往返、验「已带非默认标签的
   目录拒绝打标签且原标签不动」、验「残留的 Low 标签可重复打」；
   `sandbox_labels::tests` 用假 labeler 验清单与孤儿回收的编排。
   真机上 kill 宿主再重启那条仍是手动清单；
6. ✅ **NoNet 档**：`activate` 直接返回 None（§4）；
7. ✅ **进程不逃逸**：`spawn::tests::future_被丢掉时子进程跟着死` ——
   §3.2 那条约束的回归钉；
8. ✅ **并发不串扰**：`并发起进程各拿各的输出不串扰` —— §3.1 的回归钉。

手动清单（发布前过一遍）：企业策略机器、OneDrive 重定向目录、
非管理员账户、杀软共存（低完整性进程常被 EDR 盯上，误报要有说法）。

## 7. 里程碑

1. **M1 令牌地基**（✅ 已落地）：`sandbox_win.rs::token` 造受限 + Low IL
   令牌，Windows CI 单元测试 `受限令牌是低完整性` 读回 RID 断言 0x1000。
   `supported()` 恒 false —— 令牌能造但没接 spawn，不谎报。跨平台
   façade 已拆好（`sandbox.rs` 核心 + `sandbox_macos.rs` 后端）。
2. **M2 spawn + 授权面**（授权面已落地，spawn 待做）：
   - ✅ 标签管理：`LabelLedger`（清单持久化 + 孤儿识别）、
     `sandbox_win::label::{tag_low, untag}`（Win32 FFI，隔离验签名 +
     Windows CI 编译过）、`WinLabeler`（接进跨平台 trait）。
   - ✅ 授权编排：`LabelRegistry`（进程级引用计数 + 清单单写者；首个
     引用打标签、归零才撤、任一失败退回本次引用），回滚/计数/并发
     正确性用假 labeler 跨平台单测过 —— 这是「激活任一步失败 → 回滚 →
     activate None」和「多会话共享目录不互踩」两条 §2 约束的落地。
   - ✅ spawn 机制：`sandbox_win::spawn::spawn_with_token` —— 用令牌走
     `CreateProcessAsUserW`，建管道、Job Object（KILL_ON_JOB_CLOSE）、
     并发读、超时/取消，语义对齐 `proc.rs`。命令行 / 环境块拼接是
     `sandbox_cmdline`（纯逻辑跨平台单测）。FFI 签名隔离验过，运行时靠
     Windows CI 的冒烟测试 `受限令牌起进程拿得到输出`（起 `cmd /c echo`
     验管道通路）。
   - ✅ 端到端边界验收（`e2e_tests`）：Low 进程只能写打了标签的目录，
     工作区外被 MIC 拦，Windows CI 真机跑过。M2 机制完全成立。
3. **M3 接线**（✅ 已落地，待 CI 确认真机编译）：
   - `ActiveSandbox` 按平台持后端：macOS 持策略、Windows 持 `WinSandbox`
     （受限令牌 + 标签守卫，Drop 回滚）。
   - `SandboxPolicy::activate(setup)` 接 `SandboxSetup`（ledger 路径 +
     时间，macOS 忽略）；Windows 分支调 `sandbox_win::activate` 打标签 +
     建令牌，任一失败回滚并返回 None（不谎报）。
   - `SandboxedRunner::run` 的 Windows 分支用 `WinSandbox::run`（令牌
     spawn），macOS 仍垫 argv。
   - session.rs 装配传入 `<config>/sandbox-labels.json` 作 ledger 路径。
   - ✅ 收尾（已落地）：
     - temp 子目录重写（§2）：Windows 的 `workspace_write` 不再放全局
       %TEMP%；`activate` 现建 `<%TEMP%>/riot-sbx-<pid>-<纳秒>`、打标签、
       `WinSandbox::run` 把进程 `TMP`/`TEMP` 指过去、`Drop` 整个删掉。
     - `WorkspaceWriteNoNet` 诚实降级：`allow_network == false` 时
       Windows `activate` 直接返回 None（Low IL 不隔离网络，见 §4）。
     - 集成测试 `windows_经装配路径的沙箱边界`：走完整
       activate → SandboxedRunner → run，验边界（区别于 sandbox_win 的
       e2e 手动串底层）。sandbox_win 的生产代码本地用隔离 crate 的
       windows clippy 验过，集成测试的运行时靠 CI host。
   - M3 全部完成。Windows 沙箱从设计到接线闭环。
4. **M4 并发与回收加固**（✅ 已落地）：
   - 标签生命周期从"每轮各自打/撤"改成进程级 `LabelRegistry` 引用
     计数（见 §2）：修多会话并发下的清单 lost update 和共享目录
     （工作区、构建缓存）的撤标签互踩。
   - 孤儿回收例程落地：内核启动时（`main.rs`，任何会话激活之前）
     `recover_orphan_labels` 按清单撤残留，`sandbox-labels.lock`
     独占锁挡同机双开。
   - `home_dir` 按平台取 `USERPROFILE`/`HOME`（原来 Windows 恒 None，
     缓存目录进不了 writable）；缓存表按平台分两张（Windows 的
     npm/pip/pnpm 在 `%LOCALAPPDATA%`）；`dedup_existing` 真过滤
     不存在的路径（原来只是名字这么叫 —— 不修的话上一条修完，
     `Library/Caches` 这类 Windows 上永不存在的路径会让激活 100% 失败，
     两个 bug 互相掩盖）。

5. **M5 正确性加固**（✅ 已落地）：
   - 装配层次改对：`SandboxedRunner` 从链条最外层挪到**最里层**。
     Windows 分支用令牌自己起进程、根本不调 `inner`，装外层等于把
     venv / 能力包两层装饰器整个短路 —— 而沙箱默认开着，表现是
     「Windows 上会话设的 Python venv 静默失效」。顺带修好了 venv 与
     能力包的 PATH 先后（`prepend_path` 往队首插，外层先跑，所以 venv
     必须在里层才排得到前面），并把装配抽成 `session::process_chain`
     让顺序能被用例钉住。
   - spawn 清理改成 RAII（§3.2）、句柄白名单 + 收输出宽限期（§3.1）。
   - 打标签前体检已有 mandatory label（§2）。
   - 沙箱提到会话级复用、`Drop` 走 `spawn_blocking`（§2）。
   - 会话 temp 补 `TMPDIR`；清单临时文件名带 pid（同机双开时两个内核
     会写同一个 `.json.tmp` 再 rename，原子性就没了）。

`[实施注记]` FFI 签名是在 Mac 上用一个只依赖 `windows` crate 的
临时 crate `cargo check --target x86_64-pc-windows-msvc` 逐个逼出来的
（reqwest→ring 的 C 交叉编译在 Mac 上跑不起来，整包 check 不通，但
`windows` 的元数据平台无关，隔离出来能查）。运行时行为仍以 Windows CI
为准 —— check 过不代表 `SetTokenInformation` 真生效。
