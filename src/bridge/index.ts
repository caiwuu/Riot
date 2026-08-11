/**
 * Bridge 层 —— 前端与 Tauri 宿主之间唯一的通道。
 *
 * [约束] 这是整个 `src/` 里唯一允许 import `@tauri-apps/api` 的目录。
 * 其余代码只能 import 本模块导出的函数。违反这条会让前端无法在浏览器里
 * 单独跑起来（调试、Storybook、组件测试全部失效），也让 mock 无处下手。
 *
 * 这条约束由 eslint 的 no-restricted-imports 强制，见 eslint.config.js。
 */

import { Channel, invoke } from "@tauri-apps/api/core";

import type {
  AgentEvent,
  Message,
  PermissionAsk,
  PermissionMode,
  PermissionResponse,
} from "./generated";

export type { AgentEvent, Message, PermissionAsk, PermissionMode, PermissionResponse };

/** 服务方协议。决定请求格式、认证头和哪些采样参数可发送。 */
export type Protocol = "openai" | "anthropic";

/** 采样参数。null/undefined = 不设置：provider 层表示用服务端默认，会话覆盖层表示继承 provider。 */
export interface Sampling {
  temperature?: number | null;
  topP?: number | null;
  /** 仅 Anthropic 协议发送。 */
  topK?: number | null;
  maxOutputTokens?: number | null;
}

/** 一个模型服务方。**不含 API key** —— 密钥存宿主侧的 auth.json。 */
export interface ProviderConfig {
  id: string;
  name: string;
  protocol: Protocol;
  baseUrl: string;
  /** 读 key 的环境变量名，同时是 auth.json 里的存储键。 */
  apiKeyEnv: string;
  /** 已添加的模型（手动或从 /models 接口挑的）。 */
  models: string[];
  fallbackModel?: string | null;
  /** 这个服务方的采样参数。会话可以临时覆盖单个字段。 */
  sampling: Sampling;
}

/** 联网能力。抓取和搜索分开开关。 */
export interface WebConfig {
  /** 允许 WebFetch 抓网页。 */
  fetchEnabled: boolean;
  /** 允许 WebSearch 搜索。还要有 searxngUrl 才真的可用。 */
  searchEnabled: boolean;
  /** SearXNG 实例地址，如 http://127.0.0.1:8080。 */
  searxngUrl: string;
  /** 蒸馏网页正文的辅助模型，格式 `providerId/model`。空 = 不蒸馏。 */
  distillModel: string;
}

/** 应用配置，整个结构持久化到 config.json。 */
export interface AppConfig {
  providers: ProviderConfig[];
  activeProvider: string;
  activeModel: string;
  /** 最近打开过的项目目录，最近的在前。 */
  projects: string[];
  /** 新会话的默认权限模式。 */
  defaultMode?: PermissionMode | null;
  /** 权限弹窗等多久算超时（秒）。超时按拒绝处理，宿主侧夹在 5–3600。 */
  askTimeoutSecs: number;
  web: WebConfig;
}

/** 侧边栏里的一个会话。会话从创建起绑定 root，永不改变。 */
export interface SessionInfo {
  id: string;
  root: string;
  title: string | null;
  seq: number;
  /** 会话级采样覆盖。空字段 = 继承 provider。 */
  sampling: Sampling;
  /** 宿主侧的当前权限模式。UI 显示必须以它为准，不能拿全局默认值顶替。 */
  mode: PermissionMode;
}

export interface ConfigStatus {
  config: AppConfig;
  /** provider id → "env" | "saved"。没配 key 的 provider 不出现。 */
  keyStatus: Record<string, string>;
  configPath: string;
  /** 本次启动时配置读不懂，原文件被挪到了这里。正常启动没有这个字段。 */
  configBackup?: string | null;
}

/** 当前激活的 provider 有没有可用的 key。 */
export function hasActiveKey(s: ConfigStatus): boolean {
  return Boolean(s.keyStatus[s.config.activeProvider]);
}

/** 事件流句柄。取消订阅只是停止分发，不会中断内核 —— 中断要显式调 interrupt。 */
export interface Subscription {
  unsubscribe(): void;
}

/**
 * 订阅序号。每次订阅取一个更大的值，宿主靠它分辨新旧。
 *
 * `[约束]` 必须单调递增，且**全局**唯一（不是每个会话一份）—— 同一个
 * 会话先后两次订阅要能比出先后，这是它唯一的用途。
 */
let subscribeEpoch = 0;

/**
 * 订阅一个会话的事件流。
 *
 * 用 `Channel` 而不是全局 `emit`：后者底层是拼 JS 字符串然后 eval，
 * 官方文档明说不适合高吞吐。Channel 保证有序，且能跨 command 长期持有。
 *
 * `[约束]` 必须带 `epoch`。两次订阅在宿主侧是两个并发任务，落地顺序
 * 没有保证；不带序号的话，先发的那次可能后落地，宿主于是把事件发给
 * 一个前端已经弃用的 channel —— 表现为发完消息永远转圈。见
 * `AppState::attach_sink`。
 */
export function subscribeSession(
  sessionId: string,
  onEvent: (event: AgentEvent) => void,
  onError?: (message: string) => void,
): Subscription {
  let active = true;
  const epoch = ++subscribeEpoch;
  const channel = new Channel<AgentEvent>();
  channel.onmessage = (event) => {
    if (active) onEvent(event);
  };

  // [约束] 这里必须处理失败。之前写的是 `void invoke(...)`，于是订阅被
  // ACL 拒绝时什么都不发生 —— 界面正常显示、发消息也不报错，只是永远
  // 收不到任何事件。那个 bug 藏了整整一个开发回合。
  invoke("subscribe_session", { sessionId, epoch, onEvent: channel }).catch(
    (e: unknown) => {
      if (active) onError?.(String(e));
    },
  );

  return {
    unsubscribe() {
      active = false;
    },
  };
}

/**
 * 发一轮用户输入。
 *
 * 立刻返回，不等这一轮跑完 —— 整轮可能要几分钟，等待期间用户按不了停止键。
 * 结果全部走事件流。
 */
export function sendTurn(sessionId: string, text: string): Promise<string> {
  return invoke<string>("send_turn", { sessionId, text });
}

/** 中断当前轮。内核会补齐所有悬空的 tool_result，见 invariants::check_tool_pairing。 */
export function interrupt(sessionId: string): Promise<void> {
  return invoke("interrupt", { sessionId });
}

/** 回应一个权限询问。askId 来自 PermissionRequest 事件。 */
export function respondPermission(
  sessionId: string,
  askId: string,
  response: PermissionResponse,
): Promise<void> {
  return invoke("respond_permission", { sessionId, askId, response });
}

/** 会话级采样覆盖。空字段继承 provider 的设置；下一轮生效。 */
export function setSessionSampling(sessionId: string, sampling: Sampling): Promise<void> {
  return invoke("set_session_sampling", { sessionId, sampling });
}

export function setPermissionMode(
  sessionId: string,
  mode: PermissionMode,
): Promise<void> {
  return invoke("set_permission_mode", { sessionId, mode });
}

export function getConfig(): Promise<ConfigStatus> {
  return invoke<ConfigStatus>("get_config");
}

export function setConfig(config: AppConfig): Promise<ConfigStatus> {
  return invoke<ConfigStatus>("set_config", { config });
}

/** 保存某个 provider 的 API key（宿主写进 0600 的 auth.json）。空字符串删除。 */
export function setApiKey(providerId: string, key: string): Promise<ConfigStatus> {
  return invoke<ConfigStatus>("set_api_key", { providerId, key });
}

/** 拉取某个 provider 的可用模型列表（GET /v1/models）。 */
export function listModels(providerId: string): Promise<string[]> {
  return invoke<string[]>("list_models", { providerId });
}

/** 登记一个项目目录（验证并规范化），返回 canonical 根。不创建会话。 */
export function addProject(path: string): Promise<string> {
  return invoke<string>("add_project", { path });
}

/** 在某个项目下开新会话。返回的会话从创建起绑定这个目录。 */
export function createSession(root: string): Promise<SessionInfo> {
  return invoke<SessionInfo>("create_session", { root });
}

/** 所有活着的会话。启动或刷新后用它对齐侧边栏。 */
export function listSessions(): Promise<SessionInfo[]> {
  return invoke<SessionInfo[]>("list_sessions");
}

/** 一个会话的完整历史，切回时重建对话流。 */
export function getHistory(sessionId: string): Promise<Message[]> {
  return invoke<Message[]>("get_history", { sessionId });
}

/** 删除会话（正在跑的轮子会被中断）。幂等。 */
export function deleteSession(sessionId: string): Promise<void> {
  return invoke("delete_session", { sessionId });
}

/** 重命名会话。空标题清除手动名，回退到第一条消息。 */
export function renameSession(sessionId: string, title: string): Promise<void> {
  return invoke("rename_session", { sessionId, title });
}

/**
 * 把项目从列表移除，连带关闭它下面的会话。**不删磁盘上的目录。**
 * 返回被关闭的会话 id。
 */
export function removeProject(root: string): Promise<string[]> {
  return invoke<string[]>("remove_project", { root });
}

/* ── 内置浏览器面板 ─────────────────────────── */

/** 一帧画面。`data` 是 base64 的 JPEG，直接能当 img 的 src。 */
export interface BrowserFrame {
  data: string;
  width: number;
  height: number;
}

/**
 * 面板发给页面的输入。
 *
 * 坐标是**页面坐标**（相对视口左上角的 CSS 像素）。面板负责把自己的 DOM
 * 坐标换算过来 —— 它知道当前缩放比例，宿主不知道。
 */
export type BrowserInput =
  | { kind: "click"; x: number; y: number; button: string }
  | { kind: "move"; x: number; y: number }
  | { kind: "scroll"; x: number; y: number; deltaY: number }
  | { kind: "text"; text: string }
  | { kind: "key"; key: string };

/**
 * 打开面板，开始接收画面。
 *
 * 返回的 unsubscribe 只停止本地分发；要让宿主停止编码必须调
 * {@link closeBrowser} —— 没人看的时候继续推是白烧 CPU。
 */
export function openBrowser(
  sessionId: string,
  onFrame: (f: BrowserFrame) => void,
): Subscription {
  let active = true;
  const channel = new Channel<BrowserFrame>();
  channel.onmessage = (f) => {
    if (active) onFrame(f);
  };
  invoke("browser_open", { sessionId, onFrame: channel }).catch((e: unknown) => {
    if (active) console.error("打开浏览器面板失败", e);
  });
  return {
    unsubscribe() {
      active = false;
    },
  };
}

export function closeBrowser(sessionId: string): Promise<void> {
  return invoke("browser_close", { sessionId });
}

export function browserNavigate(sessionId: string, url: string): Promise<void> {
  return invoke("browser_navigate", { sessionId, url });
}

export function browserInput(sessionId: string, input: BrowserInput): Promise<void> {
  return invoke("browser_input", { sessionId, input });
}

/** 在系统文件管理器（访达/资源管理器）里显示这个目录。 */
export async function revealInFinder(path: string): Promise<void> {
  const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
  await revealItemInDir(path);
}

/** 弹系统的目录选择框。 */
export async function pickDirectory(): Promise<string | null> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ directory: true, multiple: false });
  return typeof picked === "string" ? picked : null;
}

/**
 * 发一个最小请求，验证 base URL / key / 模型名。不传参数测当前激活的；
 * 设置页传"正在编辑的那个"。成功返回一句人话；失败时错误信息里已带原因。
 */
export function testConnection(providerId?: string, model?: string): Promise<string> {
  return invoke<string>("test_connection", {
    providerId: providerId ?? null,
    model: model ?? null,
  });
}

/**
 * 测 SearXNG 地址通不通。传的是正在编辑的地址，不用先保存。
 *
 * 会真发一次查询而不是只打首页 —— 首页 200 说明不了 JSON 输出开没开，
 * 而那正是最容易配错的一处。
 */
export function testSearchBackend(baseUrl: string): Promise<string> {
  return invoke<string>("test_search_backend", { baseUrl });
}

/** 窗口标题跟随当前项目 —— 多开窗口时用户靠标题分辨哪个是哪个。 */
export async function setWindowTitle(title: string): Promise<void> {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().setTitle(title);
}
