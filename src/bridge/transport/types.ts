/**
 * bridge 与宿主之间的传输层抽象。
 *
 * bridge/index.ts 只认识这个接口。桌面窗口里它由 Tauri IPC 实现
 * （`tauri.ts`），浏览器里由 WebSocket 实现（`web.ts`）—— 同一份界面、
 * 同一批命令、同一套通道语义，只有这一层不同。
 *
 * 接口刻意贴着 Tauri 的形状（`invoke` + `Channel` + `listen`），这样 Tauri
 * 那份实现几乎是透传，而 Web 那份要做的就是把这三样搬到一条 WebSocket 上。
 */

import { t } from "../../i18n";

/** 宿主往前端推消息的通道。对应 `@tauri-apps/api/core` 的 `Channel`。 */
export interface HostChannel<T> {
  /** 收到一条消息。宿主保证同一通道内有序。 */
  onmessage: (msg: T) => void;
}

/** 连接状态。桌面 IPC 永远是 `open`；WebSocket 会在这几个之间切。 */
export type LinkStatus =
  /** 正在建第一条连接。 */
  | "connecting"
  /** 通着。 */
  | "open"
  /** 断了，正在按退避重连。 */
  | "reconnecting"
  /** 没有令牌或令牌被拒。要用户给一个新的（见 RemoteGate）。 */
  | "auth-required";

export interface Transport {
  readonly kind: "tauri" | "web";

  /** 调一条宿主命令。错误 reject 成宿主给的那个值（`UiError` 的 JSON），由 bridge 包成 HostError。 */
  invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T>;

  /**
   * 造一条通道，放进 `invoke` 的 args 里传给宿主。JSON 序列化后是宿主认得的
   * 占位串，宿主换成一条真正的 Channel，之后往里 send 的每条消息落到
   * `onmessage`。
   */
  channel<T>(): HostChannel<T>;

  /** 监听一个全局事件（对应宿主的 `app.emit`）。返回退订函数。 */
  listen<T>(event: string, cb: (payload: T) => void): Promise<() => void>;

  /** 当前连接状态。 */
  status(): LinkStatus;

  /** 状态变化。立刻用当前状态回调一次。返回退订函数。 */
  onStatus(cb: (s: LinkStatus) => void): () => void;

  /**
   * 连接**重新**建立（第一次连上不算）。宿主侧凡是绑在旧连接上的东西
   * （会话事件出口、终端出口、浏览器面板）都已随旧连接作废，订阅方收到
   * 这个回调后要重新订阅、重新对账。桌面 IPC 永不触发。
   */
  onReconnect(cb: () => void): () => void;
}

/** 连接断开时未完成的调用会以这个类拒绝。调用方能据此分辨"宿主说不行"和"线断了"。 */
export class TransportDisconnected extends Error {
  constructor(message = t("errors.disconnected")) {
    super(message);
    this.name = "TransportDisconnected";
  }

  /** 调用方普遍用 `String(e)` 铺文案；别让用户看到 "TransportDisconnected: " 前缀。 */
  override toString(): string {
    return this.message;
  }
}
