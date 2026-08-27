# Windows 沙箱设计

底层实现是 vendored 的 `srt-win`（Apache-2.0，见 `vendor/srt-win/NOTICE.md`）。
Riot 侧只做编排，代码在 `crates/riot-runtime/src/sandbox_win.rs`。

跨平台的策略层、`SandboxedRunner` 装饰器、决策链的放宽档见
`crates/riot-runtime/src/sandbox.rs` 与 ARCHITECTURE.md §9.6。

## 0. 这份文档换过一次设计

`[前提]` 改这里之前先读这一节：**上一版是「受限令牌 + Low 完整性级别」，
已经废弃。** 它被咬过两次真实事故：

1. 给 `~/.rustup` 打 Low 标签之后，**宿主机自己的 cargo 全废**（os error 5），
   残留后重启、重装 rustup 都救不回来。
2. 沙箱内 `docker` 连不上 daemon —— Low 进程对 Medium 完整性的 named pipe
   没有写权限，而 MIC 的 no-write-up 是内核规则，宿主侧放什么权限都没用。

根因是同一个：**Low 标签是对象属性，对全机所有进程生效**。想让沙箱进程能
写一个目录，就得把那个目录降到 Low，于是机器上任何一个 Low 进程都能写它，
而从那个目录里的 exe 启动的进程还会被连带降权。

调研过两家的做法（Anthropic 的 `srt-win`、OpenAI 的 `codex-windows-sandbox`），
架构几乎相同，而且**都不用 Low IL**。srt-win 的 `token.rs` 写得很直白：

> Integrity Level = Medium (same as a normal user process) — so Schannel /
> LSA / registry edge cases that fire at Low IL don't apply.

## 1. 换的是隔离轴，不是实现细节

| | 旧（Low IL） | 新（专用账户） |
|---|---|---|
| 主体 | 同一个用户，降到 Low IL | **另一个本地账户**，Medium IL |
| 客体 | 给目录打 Low 标签（影响所有人）| 给沙箱 SID 加**附加 ACE**（只影响它）|
| 撤销 | 撤标签（要引用计数 + 孤儿回收）| 撤 ACE（srt-win 的状态库按 holder 记账）|
| 敏感面 | 打标时挖「保护洞」跳过 | 不需要：不 grant 就够不着 |

最后一行是这次收益最大的地方。旧模型是「先把整棵树放宽，再把敏感的几处挖
洞排除」——开放集合上的枚举，漏一条就是静默的洞。新模型是「默认够不着，
按需授权」：沙箱账户对宿主用户的文件本来就没有任何权限。

附加 ACE 从不重写路径原有的安全描述符（srt-win 的 `acl.rs`：
"additive explicit ACEs … never a `PROTECTED` rewrite or SD snapshot"），
所以宿主用户的访问一点不变，上面那两个事故的根因就此消失。

## 2. 只用它的一半

srt-win 的完整方案是「文件系统隔离 + WFP 出网栅栏」。Riot **只用前一半**：

- 用：专用本地账户、附加 ACE、Medium IL 受限令牌、job object、独立桌面
- 不用：`wfp` 子命令、`cert_store`（MITM CA，为 TLS 终止服务）

WFP 栅栏会拦掉沙箱账户的**全部**外连、只放行 loopback 上的代理端口段——它
假设调用方跑着 HTTP/SOCKS5 代理。Riot 没有代理层，也刻意保留
`allow_network: true`（理由见 `sandbox.rs` 的取舍）。装了它沙箱内会彻底断网，
`npm install` / `cargo build` 全死。

`[约束]` 裁剪**靠不调用，不靠改代码**。`Wfp` / `Acl` / `Exec` / `User` /
`Install` 在它的 CLI 里是分开的子命令，`exec` 路径上没有 WFP 检查，所以这个
子集是成立的。vendored 的 `src/` 与上游逐字节相同，同步上游就是整个目录重拷
一遍。

代价：**NoNet 档在 Windows 继续诚实降级**（`activate` 返回 `None` → 决策链
回到逐条询问）。硬装成"断网了"就是假隔离。

## 3. 编排的形状

### 3.1 和 macOS 同构

```
SandboxedRunner::run → sandbox.win.wrap(spec) → inner.run(wrapped)
```

`wrap` 把 `ProcessSpec` 改写成一次 `srt-win exec --quiet --env … -- <原命令>`，
交给 `proc.rs` 那套执行器跑。管道、超时、取消、输出封顶全部复用。

上一版为此手写了 490 行 `CreateProcessAsUserW` + 管道泵 + Job Object，现在
一行都不用 —— 那些事 srt-win 的 `launch.rs` 已经做了，而且做得更多（进程
缓解策略、句柄白名单、独立桌面）。

`[约束]` 改写后 `spec.env` **必须清空**，环境变量只走 `--env`。留着的话
`inner` 会把它设到 srt-win（broker）自己的环境上，而沙箱子进程是另一个
进程，根本看不到 —— 表现是「设了环境变量但命令里读不到」。有测试钉住。

### 3.2 为什么起子进程而不是在进程内调

`srt-win` 是库 + 二进制两用的，`run_from_args` 可以直接链接。但 `exec` 的
broker 半边会**把子进程的 stdout/stderr 泵到自己的 stdio**（见它的
`logon.rs`）—— 在进程内调，沙箱命令的输出就直接串进内核自己的标准输出了，
没法按命令捕获。

### 3.3 multicall：不额外发一个 exe

上游为「链接进宿主的二进制、按 `argv[1]` 分发」这条路专门导出了
`SRT_WIN_DISPATCH_ARG1` + `run_from_args`。分发点有两个，因为
`current_exe()` 取决于内核跑在哪：

- `crates/riot-kernel/src/main.rs`（阶段 B：独立内核进程）
- `src-tauri/src/main.rs`（阶段 A：内核嵌在宿主里）

`[约束]` 分发必须抢在**一切**之前 —— 日志、panic hook、tokio 运行时都不能
先跑。这条路径上进程的身份是「srt-win 的 broker 或 runner」，任何抢先写
stderr 的东西都会污染那条通道。

开发和 CI 可以用 `RIOT_SRT_WIN` 指向独立的 `srt-win.exe`
（`cargo build -p srt-win`），绕开 multicall。

分发没接上也不会静默出错：`activate` 起手就是一次 `srt-win user status`，
那一步失败就返回 `None`，决策链退回逐条询问。

### 3.4 会话时序

| 时机 | 动作 |
|---|---|
| 内核启动 | `acl recover`（回收上次崩溃残留的 ACE）|
| 会话激活 | `user status` 查装机 → 建会话 temp → `acl grant`（可写目录 + temp）→ `acl stamp`（逃逸面 DENY）→ 冒烟 |
| 每条命令 | `srt-win exec --quiet --env … -- <命令>` |
| 会话结束 | `acl revoke` + `acl restore` + 删会话 temp |

`grant` 和 `stamp` 是一对，回收也必须成对（`revoke` 撤 ALLOW，`restore` 撤
DENY）。只撤一边会留下另一边，而留下 DENY 尤其糟：下次会话的 grant 压不过它，
表现是沙箱内的 cargo 莫名其妙写不了 `.cargo\bin`。

holder 是**内核进程的 pid**，不是 `srt-win acl` 那个短命进程的。srt-win 按
路径引用计数，同机另一个会话正用着同一个工作区时，它的 ACE 不会被连坐撤掉。

`recover` 不带 `--force`：那个会无视 holder 存活情况横扫，同机双开会踩到另一
个内核进程正在用的授权。

### 3.5 会话 temp 为什么还在

沙箱账户有自己的 `%TEMP%`，但 Riot 在 Windows 上跑的是 Git Bash，MSYS 那套
**先看 `TMPDIR`**：不设的话 `mktemp`、编译器的中间文件会落到沙箱账户够不着
的地方。所以仍然建一个会话专属 temp（在真实用户的 `%TEMP%` 下），单独授权
给沙箱账户，并用 `--env` 把 `TMP`/`TEMP`/`TMPDIR` 三个都指过去。

## 4. 需要一次提权安装，而且是**两步**

```powershell
srt-win install         # 建本地账户 + 组 + DPAPI 凭证；顺带装 WFP 过滤器
srt-win wfp uninstall   # 把 WFP 过滤器摘掉，账户和凭证留着
```

两步都要跑，**第二步不能省**。`install` 没有 `--no-wfp` 之类的开关，它把
账户和出网栅栏一起装。而那道栅栏会拦掉沙箱账户的**全部**外连、只放行
loopback 上的代理端口段（默认 60080-60089）—— Riot 没有代理层，留着它的
后果是沙箱内彻底断网：`npm install`、`cargo build` 全死，而策略层还以为
`allow_network` 是 true。这是最糟的一类不一致：策略说通，现实说不通，
而报错里没有任何东西指向沙箱。

`wfp uninstall` 只调 `wfp::uninstall_filters`，账户、组、凭证、装机标记
都不动。之后 `srt-win user status` 照样报 `cred_present: true`，
`activate` 就能拿到 SID。

没装时 `activate` 返回 `None` 并打一句能照做的日志。不在 `activate` 里偷偷
装：那需要提权，而这里可能跑在后台会话里。

## 4.5 边界比 macOS 弱一档，这是模型决定的

`[约束]` 附加 ACE 只**增加**权限，不移除已有的。沙箱账户是 `BUILTIN\Users`
成员，所以**凡是对 Users 开放写的位置，它照样写得进** —— `C:\Windows\Temp`、
ACL 松散的数据盘目录，都不在保护范围内。

这和旧模型是两种性质：

| | 旧（Low IL） | 新（专用账户 + ACE） |
|---|---|---|
| 默认 | **拒**（没打标签的一律写不进） | **随对象原有 ACL** |
| 授权 | 把对象降到 Low（全机生效） | 给沙箱 SID 加 ALLOW（只对它生效）|

换来的是不再误伤宿主（rustup 事故、docker pipe），代价是「未授权 ≠ 写不进」。
macOS 那侧是 `(deny file-write*)` 打底再逐个放行，属于旧模型那一类。

**真正被挡住的是要紧的那些**：真实用户的 profile（另一个用户的目录，沙箱
账户没有任何权限）、Program Files、系统目录（`srt-win install` 还会额外
给 11 条系统路径打 ambient write-deny）。

**残余风险**：工作区如果放在一个对 Users 开放写的位置（比如数据盘根目录下
的 `D:\projects`），它的兄弟目录沙箱也写得进。想收紧只能靠 `srt-win acl
stamp` 逐个打 DENY —— 而那是个开放集合，枚举不完，所以没做。

实测（win-sandbox-e2e，GitHub runner）：未授权的兄弟目录 `exit=0`、文件真被
建出来，它的 ACL 是

```
BUILTIN\Users:(I)(OI)(CI)(RX)
BUILTIN\Users:(I)(CI)(AD)
BUILTIN\Users:(I)(CI)(WD)
```

`AD`（AppendData / AddSubdirectory）和 `WD`（WriteData）都继承自上级，而沙箱
账户是 `BUILTIN\Users` 成员 —— 这就是「未授权 ≠ 写不进」的具体样子。

e2e 测试因此把判据定在「真实用户的主目录碰不到」上，而不是「任何未授权
路径都碰不到」；后者只作诊断打印。

### 4.6 主目录缓存不授权 —— 沙箱账户看不到它们

`[约束]` 两个平台的隔离轴不同，缓存表因此不能照抄。

macOS 的 seatbelt 包住**同一个进程**，身份不变，`$HOME` 还是真实用户的，
所以放行 `~/.cargo` 是实打实的。Windows 换成了专用账户：srt-win 用
`CreateProcessWithLogonW(…, LOGON_WITH_PROFILE, …)` 且 `lpEnvironment = NULL`，
沙箱进程拿到的是**沙箱账户自己的** profile 环境 —— `USERPROFILE` /
`LOCALAPPDATA` 全换了人。而 cargo / npm / pip / go 的缓存位置无一例外由这两个
变量推出来。

真机实测（win-sandbox-e2e，2026-08-27）：

```
沙箱内 USERPROFILE = C:\Users\srt-sandbox   （宿主 = C:\Users\runneradmin）
CARGO_HOME 未设 → cargo 的家 = C:\Users\srt-sandbox\.cargo
cargo build → exit 0，stderr："Updating crates.io index / Downloaded cfg-if v1.0.4"
```

真实用户的 registry 里明明已经有那个 `.crate`，沙箱还是重下了一遍。授权那棵树
换不到任何东西，只换来三样代价：每会话多花约 4 秒（可继承 ACE 要传播到几万个
文件）、凭空造出一个逃逸面（ALLOW 继承进 `.cargo\bin`）、以及为堵它再花约 13 秒
`acl stamp`。所以整张表清空，三样一起消失。

代价是沙箱账户第一次构建要重下依赖 —— 但它的 profile 是真实目录、跨会话留着，
所以这是**每台机器一次**，不是每会话一次。

想加回来的话，光加路径没用 —— 必须同时用 `--env` 把 `CARGO_HOME` /
`RUSTUP_HOME` / npm 那套变量显式指过去，并重新面对上面第二、三条代价。

**清空这张表不等于工具链也够不着** —— 那是两件事，读的授权见 §4.7。

三轮实测下来的账：

| | activate | 说明 |
|---|---|---|
| 原来（整棵 `.cargo` 可写） | 17.8s | grant 5.1s + stamp 12.7s |
| 清空表（工具链够不着） | 0.26s | 快，但 Rust 全废 |
| 现在（缓存不给、工具链只读） | 2.55s | grant 2.4s + stamp 0 |

剩下那 2.4s 主要是 `~/.rustup` 的树传播。还能再收（只授权当前 active 的
toolchain），但它是**每会话**一次不是每命令一次，暂时没动。

### 4.7 读也要显式授权，清单由用户手填（allowRead，上游语义）

`[约束]` **Windows 没有「读默认放开」这回事。** macOS 的 seatbelt 包住同一个
进程，读全开、只收紧写；Windows 换成专用账户之后，能不能读某个对象完全由它
自己的 ACL 说了算，而真实用户的 profile 只授权给他本人 —— `BUILTIN\Users` 一点
权限都没有。于是 PATH 上解析得到的 `C:\Users\<真人>\.cargo\bin\cargo.exe`
**打不开**，沙箱里的 Rust 全废。

出路走 `acl grant` 的 **read** 那一列（`FILE_GENERIC_READ|EXECUTE`），清单来自
设置页手填的「沙箱内额外可读的目录」（`AppConfig.sandbox_allow_read`，语义
对齐上游的 `filesystem.allowRead`，默认为空）。路径会先 canonicalize ——
nvm/fnm 那类目录是符号链接，给链接本身打 ACE 落不到真身上；不存在的路径直接
丢掉（给不存在的路径写 ACE 失败，而 grant 全有或全无）。

**这里曾经自动从 PATH 推清单**（收 PATH 里属于用户 profile 的目录 + `.rustup`
等 shim 目标树），2026-08-28 撤掉了，别加回来。真机事故：用户把 anaconda 整棵
（40 万文件）挂在 PATH 上，加上工作区和 `.rustup`，一次会话激活要做五十多万次
文件级 ACE 传播 —— 首轮「正在生成」卡好几分钟，强杀后孤儿授权又把下一次内核
启动堵在同步清扫上，界面上是「会话列表在、消息全空」。自动推的清单**没有体量
上限**，而激活是每会话都要付的同步成本；上游（Claude Code）与 Codex 都不做
这件事。

代价照上游的方式记档：**per-user 安装的工具（nvm、Scoop、conda、
`pip install --user`、`%LOCALAPPDATA%\Programs\…`）在沙箱内默认打不开。**
命令失败时输出带 `[riot:sandbox]` 提示（§5），用户的出路是装机器级工具、
手填 allowRead，或对那条命令用 `sandbox: false`。另注意手填只解决「打得开」：
rustup / cargo 这类按 `%USERPROFILE%` 定位数据的工具在沙箱账户下看的是**它
自己的** profile，要在沙箱里跑 per-user 的 rust 还得把 `RUSTUP_HOME` /
`CARGO_HOME` 设成机器级环境变量（CI runner 正是这么配的）。

只读不构成逃逸面：改不了 `cargo.exe` 就换不到沙箱外的执行权，所以 §4.8 那套
DENY 不必跟上来（`stamp_targets` 只筛**可写**授权下的敏感面）。

### 4.8 授权的树里要再挖掉逃逸面

`acl grant` 写的是 `(OI)(CI)` **可继承** ALLOW。所以「把 `~/.cargo` 整棵放开」
不只放开了 registry 缓存，也一路放开了 `~/.cargo\bin` —— 沙箱账户能顶掉那里的
`cargo.exe` / `rustc.exe`（rustup shim 全家，就在用户 PATH 上），用户下一次在
**沙箱外**构建就执行了它。

这一层曾经漏过：从 Low IL 换到账户模型时，按「不 grant 就够不着」把 Windows 的
排除面整个删了 —— 而 `.cargo` 恰恰是 grant 的。现在由 `acl stamp` 的附加 DENY
ACE 补回来（DENY 在 DACL 求值里排在 ALLOW 之前），清单和 macOS 共用
`escape_surfaces()`，只对**授权过的**路径打（没 grant 的地方本来就够不着，白打
一遍还要在几万文件的树上传播 ACE）。

**一个补不上的洞。** srt-win 对缺失的 deny 目标会建空 placeholder，而 cargo 的
配置有「现代名」和「无扩展名的老名」两份，两份都在时**用老的**并警告一句。所以
老名不能预建：一个空的 `~/.cargo\config` 会把用户真实的 `config.toml` 整个屏蔽
掉（`credentials` 那对更糟，registry 凭证直接失效），而 placeholder 是永久的
（`acl restore` 只摘 ACE 不删文件）。预建的伤害大过它堵的洞，所以留着 —— 沙箱
可以创建一个老名配置来盖过用户的。**它不是静默的**：cargo 之后每次都会打印
`warning: both … exist. Using …`。macOS 那侧没有这个问题，seatbelt 的 deny 按
路径匹配，不要求对象存在。

## 5. 已知限制（从上游原样继承）

- **上游标 alpha。**
- **PATH 之外的 per-user 工具够不到。** 上游把这条列为最疼的限制：沙箱以专用
  账户跑，profile 下装的东西（nvm/fnm 的 Node、Scoop/winget 包、
  `pip install --user`）在 PATH 上解析得到却**打不开**，出路是改用机器级安装
  或手填一份路径清单。Riot 这边把常见情况自动覆盖了 —— 见 §4.7 —— 但真装在
  profile 深处、又不在 PATH 上的东西仍然够不着。
- **NoNet 档不隔离网络**（见 §2）。
- **包缓存不共享，沙箱账户各下各的。** 见 §4.6：它的 `USERPROFILE` 是自己的，
  cargo / npm / pip 都在它自己的 profile 下建缓存。第一次构建要重下依赖，之后
  跨会话复用。想共享得显式传 `CARGO_HOME` 那套变量，代价见 §4.6。
- **沙箱内装不了 rust 工具链（macOS）。** `~/.rustup\toolchains` 在逃逸面清单上
  （见 §4.8），所以 `rustup toolchain install` 和 `rust-toolchain.toml` 的自动
  安装会失败。收窄到 `*/bin` 修不了：`lib` 下的动态库同样由 rustc 加载执行。
  出路是在沙箱外先装好。Windows 上这条不适用 —— 那边 `.rustup` 不在授权表里，
  逃逸面清单也就不会去碰它（`stamp_targets` 只对授权过的路径打）。
- **工作区不能在映射盘 / 网络盘上。** seclogon 为沙箱账户建的登录会话里没有
  per-user 的盘符映射，`CreateProcessWithLogonW` 指向那种路径会直接失败
  （srt-win 退 16，`code: mapped_drive_cwd`）。`activate` 的冒烟会提前发现
  并退回不隔离 —— 不然症状是**每条命令**都吐一段模型读不懂的 JSON。

## 6. 怎么验证

三层，从便宜到贵：

| 层 | 命令 | 挡住什么 |
|---|---|---|
| 纯逻辑单测 | `cargo test -p riot-runtime sandbox_win` | 命令行拼装、grant 载荷、装机判据。**mac 上就能跑** |
| 交叉编译 | `scripts/check-windows-sandbox.sh` | FFI 签名、cfg(windows) 分支的类型错 |
| 真机冒烟 | CI 的 `win-sandbox-smoke` job | 账户真建出来了没、ACE 真写下去又真撤干净了没 |

`[约束]` 前两层过了**不代表跑得对**。真机那层是手动触发的（Actions →
CI workflow → Run workflow），因为它会真的建账户、装 WFP 过滤器、改 ACL。
它跑的是上游自带的 6 个冒烟脚本，收尾的 `cleanup.ps1` 挂了 `if: always()`。

本地在 Windows 上跑同一套：

```powershell
cargo build -p srt-win --release
pwsh vendor/srt-win/ci/smoke.ps1 target\release\srt-win.exe
pwsh vendor/srt-win/ci/cleanup.ps1 target\release\srt-win.exe   # 务必收尾
```
