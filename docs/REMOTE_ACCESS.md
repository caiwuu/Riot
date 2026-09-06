# 远程访问（网页版）设计

> 在手机或另一台电脑的浏览器里使用同一个 Riot。本文面向实现者：凡是标注
> `[约束]` 的是硬性要求，`[取舍]` 说明为什么不选另一条路。

---

## 1. 目标与非目标

**目标**：桌面 Riot 开着的时候，主人能从浏览器里操作它 —— 同一批会话、同一个
终端面板、同一个浏览器面板、同一份设置。桌面窗口和网页是**同一个 Riot 的两个
观看者**，任何一边的操作另一边立刻看得到。

**非目标**：
- 不做独立部署的"Web 服务版"。宿主仍是 Tauri 桌面进程，网页版随它起停。
- 不做多用户。一台机器一个 Riot 一个主人，令牌就是主人的身份。
- 不做 TLS（理由见 §5）。
- 不做移动端专属布局。现有布局在窄屏上能用（侧栏可收起），专门的手机排版另立专题。

---

## 2. 架构

```text
┌───────────────────────┐        ┌────────────────────────┐
│  桌面窗口 (WKWebView)  │        │  浏览器 (手机 / 另一台机)  │
│  React + bridge       │        │  同一份 React + bridge   │
│  transport = Tauri IPC│        │  transport = WebSocket  │
└───────────┬───────────┘        └────────────┬───────────┘
            │ invoke / Channel                  │ ws://host:7823/ws
            ▼                                   ▼
┌────────────────────────────────────────────────────────────┐
│  宿主进程 (src-tauri)                                        │
│  lib.rs 的 #[tauri::command] 函数 ◀── remote/dispatch.rs     │
│  AppState（会话表、终端、浏览器面板）—— 事件出口按观看者扇出    │
│  remote/：axum 路由、鉴权、连接生命周期                        │
└────────────────────────────────────────────────────────────┘
```

三条设计决定撑起整件事：

1. **`[约束]` 零业务代码复制。** 远程分发表（`remote/dispatch.rs`）调的是 lib.rs 里
   `#[tauri::command]` 修饰的**同一个函数**（宏保留了原函数）。业务逻辑改在
   lib.rs / state.rs，两边自动一致。`tests/acl.rs` 盯着分发表和 `generate_handler!`
   的清单相等 —— 少一条的表现是网页上某个按钮报"未知命令"而桌面一切正常。
2. **`[约束]` 前端只换传输层。** `src/bridge/transport/` 是 bridge 与宿主之间唯一
   的接口：`invoke` + `channel` + `listen`。Tauri 实现是透传；Web 实现把三样搬到一条
   WebSocket 上。bridge/index.ts 之外的组件不知道自己跑在哪。
3. **事件出口按观看者扇出。** 原先"一个会话一个 Channel、后订阅的顶掉先订阅的"
   在多观看者下会让桌面窗口悄悄收不到事件。现在会话事件、终端输出、浏览器画面帧
   都按 `viewer` 登记（桌面 `webview:main`，每条远程连接 `remote:<n>`），广播给所有人，
   连接断开由 `AppState::detach_viewer` 一次收干净。

`[取舍]` 服务放在宿主而不是内核。网页版要操作的不只是会话：项目列表、终端、浏览器
面板、设置、定时任务，这些的权威全在宿主 `AppState`，内核只有会话运行时。放内核等于
把宿主重写一遍。放宿主里则一行业务逻辑都不用动，只把"事件出口"从一份变成按观看者一份。

`[取舍]` 用 `tauri::ipc::Channel` 作为远程连接的出口类型。`Channel::new(闭包)` 是公开
API，闭包收到 `InvokeResponseBody`（JSON 或原始字节）后写进这条连接的队列。于是
`AppState`、终端、浏览器面板持有的仍是 `Channel<T>`，不需要第二种出口类型，桌面路径
一个字节都没变。

---

## 3. 线协议

权威定义在 `src-tauri/src/remote/protocol.rs`，前端 `src/bridge/transport/web.ts` 照着解。

| Tauri IPC | WebSocket |
|---|---|
| `invoke(cmd, args)` | 文本帧 `{"t":"call","id":n,"cmd":…,"args":{…}}` |
| 命令返回 JSON | `{"t":"ok","id":n,"result":…}` |
| 命令返回 `ipc::Response`（字节） | 二进制帧 `[1][id:u32 LE][bytes]` |
| 命令报错（字符串） | `{"t":"err","id":n,"error":"…"}` |
| `Channel<T>` 参数 | args 里一个 `"__RIOT_CHANNEL__:<id>"` 串 |
| `Channel::send(JSON)` | `{"t":"channel","ch":id,"data":…}` |
| `Channel::send(Raw)` | 二进制帧 `[2][ch:u32 LE][bytes]` |
| `app.emit` / `listen` | `{"t":"event","name":…,"payload":…}` |

握手：连上后第一帧必须是 `{"t":"auth","token":…}`；通过回 `{"t":"ready",viewer,boot,version}`，
否则 `{"t":"denied",reason}` 然后关闭。心跳：宿主每 20s 协议层 ping（浏览器自动回 pong），
前端每 20s 发 `{"t":"ping"}` 收 `pong`；任一侧 50–65s 没听到对方就断开重连。

`[约束]` 二进制不走 JSON。浏览器面板一帧几百 KB，base64 再 `JSON.parse` 一遍是主线程上
最贵的一刀，桌面那边的取舍（lib.rs `browser_open`）在这里同样成立。

`[约束]` 通道消息手拼不重编。`Channel::send` 给的已经是 JSON 串，宿主直接嵌进帧里；
token 流每秒上百条，parse 一遍只为重新序列化是纯浪费。

---

## 4. 连接生命周期

**宿主侧**（`remote/conn.rs`）：一条连接 = 一个观看者。鉴权 → 建出口队列 → 转发全局事件
（`listen_any`）→ 读循环（每条命令 `tokio::spawn`，浏览器跳转合法地要等几十秒，不能堵
停止键）→ 收尾（`detach_viewer`、退订事件、关写任务）。

`[约束]` 服务停止（关开关、换端口 / 绑定）必须把已建立的连接一起断掉。axum 的 graceful
shutdown 管不到它们 —— 升级成 WebSocket 之后 hyper 的连接 future 就结束了，连接活在另一个
任务里。所以每个运行中的服务带一个 `watch` 信号（`Running::kick`），读循环多听一路；
`Remote::stop` 先翻信号、再停 accept、然后**等 serve 任务退出**再绑新地址 —— 只发信号不等
的话，端口不变只改绑定范围会撞上 EADDRINUSE。

出口队列的两道护栏：
- 原始字节的通道消息**每通道最多积压两条**（画面帧自成一体，丢旧帧比排队播放旧帧好）；
- 总积压超过上限就断连接 —— 客户端已经慢到没法用，断开让它重连、按快照对账，比拖着
  一条越来越滞后的连接强。

**前端侧**（`transport/web.ts`）：
- 连接断开时所有未完成的调用立刻以 `TransportDisconnected` 拒绝。不能让它们挂着等一条
  永远不会来的应答（bridge 的期限会兜，但那要 15 秒、且文案方向是"宿主没响应"）。
- 重连按指数退避（0.5s → 10s，带抖动），`online` / 回到前台时立刻试一次。
- **`[约束]` 不替订阅方自动重订。** 通道是连接级的，宿主那头的 Channel 随旧连接作废；
  但重订阅对不同东西意味着不同的对账 —— 会话要拉快照（`useSession.ensureLive`），
  终端要回放缓冲（`term_attach`，先 `reset` 屏幕免得叠两遍），浏览器面板要重开推流
  （宿主侧推流若还在跑，`browser_open_for` 会让浏览器补发一帧，否则静态页面上新观看者
  一直黑着）。所以传输层只发一个 `onReconnect`，各订阅方自己决定怎么补。
- 连接断开时前端的通道表一并清空（`failAllPending`）：旧通道再也收不到消息，留着只是
  一张随每次重订阅越攒越大的表。
- **boot id。** `ready` 帧带宿主本次启动的随机标识。重连时它变了 = 宿主重启过，终端 id、
  浏览器面板、会话水合状态全作废 —— 整页刷新，比局部对账干净。

---

## 5. 安全模型

一句话：**令牌 = 主人身份，拿到令牌的人能做桌面前的人能做的一切。** 因此：

| 措施 | 实现 | 为什么 |
|---|---|---|
| 默认关 | `RemoteConfig::enabled = false` | 这是一扇能远程执行命令的门，必须由用户亲手开 |
| 令牌只在 `auth.json`（0600） | `config::REMOTE_TOKEN_KEY`，同名环境变量可覆盖 | 和 API key 同一性质，不进可分享的 `config.json` |
| 登录链接把令牌放 `#` 后面 | `RemoteStatus::login_url` | 片段不进 HTTP 请求，服务端日志和中间代理看不到 |
| 前端收进 localStorage 后抹掉地址栏 | `takeTokenFromHash` | 留在地址栏会进历史、被截图、被"分享此页" |
| 常数时间比较 | `auth::token_matches` | 不泄漏"对了几位" |
| Origin 必须等于 Host | `auth::origin_allowed` | 挡住同一浏览器里第三方页面对本机端口的连接；反向代理用 `RIOT_REMOTE_ALLOWED_ORIGINS` 放行 |
| 同一来源连错 5 次锁 60s | `auth::Throttle` | 目的是让误配不刷屏，不是抗暴力破解（256 位令牌不需要） |
| 两档监听范围 | `RemoteBind::{Loopback, Lan}` | 回环给隧道 / 代理接，局域网给同 Wi-Fi 直连；不让用户填任意 IP |

`[取舍]` 不做 TLS。自签证书在手机上是一堆警告，用户学会点"仍然继续"之后任何中间人都能
过；要域名证书就得管域名。局域网直连本来在自己的 Wi-Fi 里；出公网走 Tailscale Serve /
Cloudflare Tunnel / nginx，它们的证书管理比我们自己搞一套靠谱得多。设置页里写明了这一点。

`[约束]` 令牌不进日志。`remote_rotate_token`、`auth.rs` 里不允许有打印令牌的 tracing 调用。

---

## 6. 网页版的能力边界

浏览器里的用户不在宿主机前，下面这些在桌面上理所当然的事在网页上没有对象：

| 桌面 | 网页版 | 在哪处理 |
|---|---|---|
| 系统目录对话框选项目 | 应用内目录选择器（`browse_dirs` + `DirPicker`） | `useDirectoryPicker` 按 `host.nativePaths` 分流 |
| 拖放 / 系统对话框拿到宿主机路径 | 只有 `File`：图片直接读内容进附件条；其它文件请用 `@` 引用 | `subscribeDragDrop` 的 HTML5 分支、Composer 的 `<input type=file>` |
| 宿主机剪贴板里的文件路径 | 回空（远端用户的 ⌘V 和宿主剪贴板无关） | `dispatch.rs` 对 `clipboard_paths` 特判 |
| 在访达 / 默认应用里打开 | 拒绝，文案说明 | `revealInFinder` / `openPath` |
| 系统浏览器打开网址 | `window.open` | `openInBrowser` |
| 系统通知 | Web Notifications（非安全上下文多半不可用，静默） | `notify` |
| 窗口标题 / 全屏 / 红绿灯让位 | `document.title`；不让位 | `setWindowTitle`、`IS_MAC` |

能力表在 `src/bridge/transport/index.ts` 的 `host`，界面按它决定入口画不画。

---

## 7. 文件索引

```text
src-tauri/src/remote/
  mod.rs        生命周期：按配置起停、令牌落盘、状态与二维码
  protocol.rs   线协议
  auth.rs       令牌、Origin、限速
  server.rs     axum 路由：静态资源（asset resolver）、/ws 握手
  conn.rs       一条连接：鉴权 → 读写循环 → 收尾
  dispatch.rs   命令名 → lib.rs 同一个处理函数
src-tauri/src/dir_browse.rs   网页版目录选择器的宿主侧
src-tauri/src/state.rs        attach_sink(viewer,…) / detach_viewer / browser_*_for（面板扇出）
src-tauri/src/term.rs         按观看者的 sinks
src-tauri/src/kernel/client.rs  Sinks: session → viewer → Channel

src/bridge/transport/
  types.ts      Transport 接口、LinkStatus、TransportDisconnected
  tauri.ts      Tauri IPC 实现
  web.ts        WebSocket 实现（重连、心跳、令牌、boot id）
  index.ts      环境探测、单例、host 能力表
src/components/RemoteGate.tsx        令牌门 + 重连横幅
src/components/DirPicker.tsx         应用内目录选择器
src/components/settings/RemotePane.tsx  设置 → 远程访问
src/hooks/useHostLink.ts             useHostReconnectTick

vite.config.ts   dev 时 /ws 反代到宿主，网页在 dev 和正式包里连接方式一致（同源 /ws）
```

---

## 8. 开发与验证

- 开发：`pnpm tauri dev`，设置 → 远程访问打开开关。本机浏览器打开
  `http://127.0.0.1:1420/#token=<令牌>`。经 Tailscale Serve 从手机访问时：
  `tailscale serve --bg http://127.0.0.1:1420`，再打开
  `https://<机器名>.ts.net/#token=<令牌>`，并设
  `RIOT_REMOTE_ALLOWED_ORIGINS=https://<机器名>.ts.net`。
  **不要**把 Serve 指到 `:7823` —— dev 模式下 `:7823` 会 307 到 localhost，
  手机上就会跳到手机自己或下成空的 `document.txt`。
  直接访问 `:7823` 会跳到 Vite（dev 模式下磁盘上的 `dist/` 多半是陈货）。
- 正式包：`:7823/` 由宿主从嵌入的前端产物直接服务，不依赖 Vite。
- 单测：`cargo test -p riot-host --lib -- remote:: dir_browse::`，`cargo test -p riot-host --test acl`。
- 手测清单：扫码登录；两个观看者同时看一个会话、任一边发消息另一边同步；重连后终端
  回放、浏览器画面恢复；换令牌后旧页面被拒、输入新令牌恢复；关掉开关后网页断开。
