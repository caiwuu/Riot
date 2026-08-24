# Windows 沙箱设计

> 状态：**设计定稿，未实现**。macOS 侧已落地（`riot-runtime/src/sandbox.rs`，
> seatbelt），本文是同一位置上 Windows 那块的实施方案。读者是实现者。
> 通用背景（为什么策略层不够、沙箱换来什么）见 ARCHITECTURE §9.6 和
> sandbox.rs 的模块文档，这里不重复。

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
激活序列：
1. 对 writable 里的每个目录 SetNamedSecurityInfoW 打 Low 标签
2. 清单落盘 <config>/sandbox-labels.json（目录 + 打标时间）
3. CreateRestrictedToken（去特权组、Low IL）
4. CreateProcessAsUserW + 挂进 Job Object（复用现有进程树清理语义）
任何一步失败 → 回滚已打的标签 → activate() 返回 None
```

`[取舍/已实现]` 清单**只记路径 + 打标时间，不记原 SACL**。因为只对
「当前是默认完整性（无显式 label）」的目录打 Low 标签，回滚 =
写空 label ACL 回到默认，没有"原状"要保存。本来就带非默认 label 的
目录（罕见）在步骤 1 检测到就跳过、整条激活降级。这把"记录原状"
简化成了"记录我动过谁" —— 清单逻辑见 `sandbox_labels.rs`
（`LabelLedger`，跨平台可测），打/去标签见 `sandbox_win::label`。

`[约束]` **temp 不给系统 `%TEMP%` 打标签**，在其下建 `riot-sbx-<会话>`
子目录打标签，并给沙箱进程重写 `TMP`/`TEMP` 环境变量指过去。给全局
temp 打标签影响的是整台机器上所有 Low 进程的攻击面，为一个会话动
全局状态不值得。macOS 版直接放开 `/tmp` 是因为 seatbelt 的授权只对
被包的那个进程生效 —— 标签是**对象**属性，对所有进程生效，两边的
授权模型不同，不能照抄。

`[约束]` 标签是持久的文件系统状态，**必须有孤儿回收**：正常退出时恢复
原标签；崩溃留下的残留由下次启动的清理例程按 `sandbox-labels.json`
兜底（对照 process_lifecycle 的哲学：无论怎么死，别往机器上漏东西）。
残留标签的实际危害很小 —— Medium 用户进程写 Low 目录不受 MIC 约束，
只是「其它 Low 进程也能写它」—— 但小不是零，要收。

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

## 4. 档位映射

| SandboxMode | macOS | Windows V1 |
|---|---|---|
| `Off` | 不激活 | 不激活 |
| `WorkspaceWrite` | seatbelt：写白名单 + 联网 | Low IL：写白名单 + 联网（MIC 不管网络，联网天然放开） |
| `WorkspaceWriteNoNet` | seatbelt + `(deny network*)` | **不激活**（activate None → 逐条询问） |

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
CI 驱动开发（mac 上写、CI 上验）。必备用例，全部对照 macOS 版
`工作区外的写被内核拒绝` 的「真跑」哲学：

1. **边界真跑**：工作区内写成功、工作区外写被拒、读系统文件成功
   （照抄 macOS 用例的三段式）；
2. **HKCU 写被拒**：`reg add HKCU\...` 非零退出 —— Windows 版多出来
   的持久化防线要有测试钉住；
3. **temp 重写**：沙箱内 `%TEMP%` 指向会话子目录且可写；
4. **降级诚实**：FAT32 卷（CI 上 `subst` 一个 VHD）→ activate None，
   `ctx.sandboxed == false`；
5. **标签回收**：kill 掉宿主进程 → 重启后清理例程把残留标签收干净
   （process_lifecycle 风格）；
6. **NoNet 档**：Windows 上 activate None + 界面标注文案存在。

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
   - ✅ 授权编排：`authorize_writable`（逐个打标签 + 记账，任一失败
     全部回滚），回滚正确性用假 labeler 跨平台单测过 —— 这是「激活
     任一步失败 → 回滚 → activate None」那条 §2 约束的落地。
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
   - ⏳ 仍缺：temp 子目录重写（§2）、`SandboxedRunner` 经 activate 的
     集成测试、config 的 `WorkspaceWriteNoNet` 在 Windows 的诚实降级
     文案（§4）。这些是收尾，不再有 FFI 运行时未知。
3. **M3 接线**：config 档位映射（含 NoNet 降级文案），用例 6 过，
   双平台 CI 全绿后发布。

`[实施注记]` M1 的 FFI 签名是在 Mac 上用一个只依赖 `windows` crate 的
临时 crate `cargo check --target x86_64-pc-windows-msvc` 逐个逼出来的
（reqwest→ring 的 C 交叉编译在 Mac 上跑不起来，整包 check 不通，但
`windows` 的元数据平台无关，隔离出来能查）。运行时行为仍以 Windows CI
为准 —— check 过不代表 `SetTokenInformation` 真生效。
