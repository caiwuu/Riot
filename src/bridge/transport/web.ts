/**
 * WebSocket 传输：浏览器里的那份。对端是宿主的 `remote` 模块，线协议见
 * `src-tauri/src/remote/protocol.rs`（那份是权威，这里照着解）。
 *
 * 三件事要做对：
 *
 * 1. **请求/应答配对**。每条 `invoke` 一个递增 id，应答按 id 落回 Promise。
 *    连接一断，所有未完成的调用立刻以 `TransportDisconnected` 拒绝 ——
 *    不能让它们挂着等一条永远不会来的应答（bridge 的期限会兜，但那要等
 *    15 秒，而且报的是"宿主没响应"，方向不对）。
 * 2. **通道**。一条通道就是一个 id + 一个回调；宿主往里 send 的每条消息以
 *    `{"t":"channel","ch":id,"data":…}`（JSON）或二进制帧（画面）到达。
 *    通道是**连接级**的：连接断了宿主那头的 Channel 全部作废，前端要靠
 *    `onReconnect` 重新订阅。这里不替订阅方自动重订 —— 重订阅对不同东西
 *    意味着不同的对账（会话要拉快照、终端要回放缓冲），只有订阅方自己知道。
 * 3. **重连**。指数退避 + 抖动，网络恢复 / 页面回到前台时立刻试一次。
 *    每次连上先看宿主的 boot id：变了就是宿主重启过，本地一切引用（终端
 *    id、浏览器面板）全作废，整页刷新最干净。
 *
 * 令牌从哪来：首次访问的链接把它放在 `#token=…`（片段不进 HTTP 请求，
 * 服务端日志看不到），这里收进 localStorage 并从地址栏抹掉；之后每次打开
 * 直接用存的。被拒（换过令牌）就进 `auth-required`，RemoteGate 让用户重填。
 */

import { t } from "../../i18n";
import { type UiTextPayload, renderUiText } from "../errors";
import type { HostChannel, LinkStatus, Transport } from "./types";
import { TransportDisconnected } from "./types";

const LS_TOKEN = "riot.remote.token";
const LS_BOOT = "riot.remote.boot";
/** 开发时把网页指到别处的宿主（默认同源）。正式用法不需要。 */
const LS_ENDPOINT = "riot.remote.endpoint";

/** 和宿主 `protocol.rs` 的 CHANNEL_PREFIX 一致。 */
const CHANNEL_PREFIX = "__RIOT_CHANNEL__:";
const BIN_CALL_RESULT = 1;
const BIN_CHANNEL = 2;

/** 心跳。宿主每 20s 也会 ping 一次；这里再发一次是为了在**前端**这边有
 *  一个"多久没听到宿主"的判据 —— 浏览器不暴露协议层的 pong。 */
const PING_EVERY_MS = 20_000;
/** 这么久没有任何来自宿主的帧就当连接死了，主动断开重连。 */
const SILENCE_LIMIT_MS = 50_000;
/** 重连退避：起点、上限。 */
const BACKOFF_MIN_MS = 500;
const BACKOFF_MAX_MS = 10_000;

interface Pending {
  resolve: (v: unknown) => void;
  reject: (e: unknown) => void;
}

class WebChannel<T> implements HostChannel<T> {
  onmessage: (msg: T) => void = () => {};
  constructor(readonly id: number) {}
  /** 放进 args 后宿主看到的就是这个占位串。 */
  toJSON(): string {
    return `${CHANNEL_PREFIX}${this.id}`;
  }
}

type Frame =
  | { t: "ready"; viewer: string; boot: string; version: string }
  | { t: "denied"; reason: UiTextPayload }
  | { t: "ok"; id: number; result: unknown }
  | { t: "err"; id: number; error: unknown }
  | { t: "channel"; ch: number; data: unknown }
  | { t: "event"; name: string; payload: unknown }
  | { t: "pong" };

export class WebTransport implements Transport {
  readonly kind = "web" as const;

  private ws: WebSocket | null = null;
  private linkStatus: LinkStatus = "connecting";
  private everOpened = false;
  private token: string | null;

  private nextId = 1;
  private readonly pending = new Map<number, Pending>();
  /** 连接没通时攒着的调用，连上再发。只在 connecting/reconnecting 期间攒。 */
  private readonly queued: { id: number; text: string }[] = [];

  private nextChannel = 1;
  private readonly channels = new Map<number, WebChannel<unknown>>();
  private readonly listeners = new Map<string, Set<(payload: unknown) => void>>();

  private readonly statusCbs = new Set<(s: LinkStatus) => void>();
  private readonly reconnectCbs = new Set<() => void>();

  private backoff = BACKOFF_MIN_MS;
  private reconnectTimer: number | undefined;
  private pingTimer: number | undefined;
  private lastHeard = 0;
  /** 用户主动登出 / 令牌被拒后不自动重连，等新令牌。 */
  private halted = false;
  /** 上一次被宿主拒绝的原因（"令牌不对"、"尝试太频繁"）。成功连上就清掉。 */
  private denied: UiTextPayload | null = null;

  constructor() {
    this.token = takeTokenFromHash() ?? localStorage.getItem(LS_TOKEN);
    window.addEventListener("online", () => this.kick());
    document.addEventListener("visibilitychange", () => {
      if (!document.hidden) this.kick();
    });
    // 页面已经开着、又在地址栏里贴了一条带 `#token=` 的链接：hash 变化
    // 不触发整页加载，构造函数看不到它，这里补上。
    window.addEventListener("hashchange", () => {
      const fresh = takeTokenFromHash();
      if (fresh) this.setToken(fresh);
    });
    if (this.token) {
      this.connect();
    } else {
      this.setStatus("auth-required");
    }
  }

  /* ── Transport ─────────────────────────────── */

  invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    if (this.linkStatus === "auth-required") {
      return Promise.reject(new TransportDisconnected(t("errors.authRequired")));
    }
    const id = this.nextId++;
    const text = JSON.stringify({ t: "call", id, cmd, args: args ?? {} });
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (v: unknown) => void, reject });
      if (this.ws && this.ws.readyState === WebSocket.OPEN && this.linkStatus === "open") {
        this.ws.send(text);
      } else {
        this.queued.push({ id, text });
      }
    });
  }

  channel<T>(): HostChannel<T> {
    const ch = new WebChannel<T>(this.nextChannel++);
    this.channels.set(ch.id, ch as WebChannel<unknown>);
    return ch;
  }

  listen<T>(event: string, cb: (payload: T) => void): Promise<() => void> {
    let set = this.listeners.get(event);
    if (!set) {
      set = new Set();
      this.listeners.set(event, set);
    }
    const wrapped = cb as (payload: unknown) => void;
    set.add(wrapped);
    return Promise.resolve(() => {
      set?.delete(wrapped);
    });
  }

  status(): LinkStatus {
    return this.linkStatus;
  }

  onStatus(cb: (s: LinkStatus) => void): () => void {
    this.statusCbs.add(cb);
    cb(this.linkStatus);
    return () => {
      this.statusCbs.delete(cb);
    };
  }

  onReconnect(cb: () => void): () => void {
    this.reconnectCbs.add(cb);
    return () => {
      this.reconnectCbs.delete(cb);
    };
  }

  setAppearance(): Promise<void> {
    // 浏览器里没有原生窗口可钉；页面自己的配色由 `<html data-theme>` 管。
    // 也不该发给宿主 —— 手机上切个主题不能把桌面那扇窗的外观一起换掉。
    return Promise.resolve();
  }

  /* ── 令牌管理（RemoteGate 用）───────────────── */

  /** 用一枚新令牌（重新）连接。 */
  setToken(token: string): void {
    const trimmed = token.trim();
    if (!trimmed) return;
    this.token = trimmed;
    localStorage.setItem(LS_TOKEN, trimmed);
    this.halted = false;
    this.backoff = BACKOFF_MIN_MS;
    this.closeSocket();
    // closeSocket 摘掉了旧 socket 的 onclose，挂在它上面的调用不会再有人
    // 拒绝 —— 这里替它做，别让它们等到 bridge 的 15 秒期限才报"宿主没响应"。
    this.failAllPending(new TransportDisconnected(t("errors.reconnectingWithToken")));
    this.connect();
  }

  /** 忘掉令牌并断开。 */
  clearToken(): void {
    this.token = null;
    localStorage.removeItem(LS_TOKEN);
    this.halted = true;
    this.closeSocket();
    this.failAllPending(new TransportDisconnected(t("errors.signedOut")));
    this.setStatus("auth-required");
  }

  hasToken(): boolean {
    return this.token !== null;
  }

  /**
   * 上一次被拒的原因，给令牌表单显示。没有 = 从来没被拒过（首次访问没带
   * 令牌），或者被拒之后已经成功连上过。
   */
  deniedReason(): UiTextPayload | null {
    return this.denied;
  }

  /* ── 连接生命周期 ───────────────────────────── */

  private endpoint(): string {
    const override = localStorage.getItem(LS_ENDPOINT);
    if (override) return override;
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    return `${proto}//${location.host}/ws`;
  }

  private connect(): void {
    if (this.halted || !this.token) return;
    if (this.ws && this.ws.readyState <= WebSocket.OPEN) return;
    window.clearTimeout(this.reconnectTimer);
    this.reconnectTimer = undefined;
    this.setStatus(this.everOpened ? "reconnecting" : "connecting");

    let ws: WebSocket;
    try {
      ws = new WebSocket(this.endpoint());
    } catch (e) {
      console.warn("Failed to create WebSocket", e);
      this.scheduleReconnect();
      return;
    }
    ws.binaryType = "arraybuffer";
    this.ws = ws;

    ws.onopen = () => {
      // 第一帧必须是鉴权（宿主 conn.rs 的规矩）。
      ws.send(JSON.stringify({ t: "auth", token: this.token }));
      this.lastHeard = Date.now();
    };
    ws.onmessage = (ev: MessageEvent<string | ArrayBuffer>) => {
      this.lastHeard = Date.now();
      if (typeof ev.data === "string") this.onText(ev.data);
      else this.onBinary(ev.data);
    };
    ws.onclose = () => {
      if (this.ws !== ws) return; // 已经被替换（setToken / 主动重连）
      this.ws = null;
      this.stopPing();
      this.failAllPending(new TransportDisconnected());
      if (this.halted) return;
      this.scheduleReconnect();
    };
    ws.onerror = () => {
      // 紧接着一定有 onclose，那里统一处理。
    };
  }

  private onText(raw: string): void {
    let f: Frame;
    try {
      f = JSON.parse(raw) as Frame;
    } catch {
      console.warn("Failed to parse host frame", raw.slice(0, 200));
      return;
    }
    switch (f.t) {
      case "ready": {
        this.onReady(f.boot);
        return;
      }
      case "denied": {
        // 令牌不对（或换过了）。停下来等用户给新的，别对着宿主刷失败。
        // 原因要留给表单显示：否则用户看到的只是"输入 → 转一圈 → 表单又
        // 空了"，分不清是密码错了还是网络不通。
        console.warn("Host refused the connection:", f.reason.key);
        this.denied = f.reason;
        this.halted = true;
        this.closeSocket();
        this.failAllPending(
          new TransportDisconnected(t("errors.hostDenied", { reason: renderUiText(f.reason) })),
        );
        this.setStatus("auth-required");
        return;
      }
      case "ok": {
        const p = this.pending.get(f.id);
        if (p) {
          this.pending.delete(f.id);
          p.resolve(f.result);
        }
        return;
      }
      case "err": {
        const p = this.pending.get(f.id);
        if (p) {
          this.pending.delete(f.id);
          // 原样给出去：这是宿主的 UiError JSON，和 Tauri 那条线一样由
          // bridge 的 invoke 统一包成 HostError。
          p.reject(f.error);
        }
        return;
      }
      case "channel": {
        this.channels.get(f.ch)?.onmessage(f.data);
        return;
      }
      case "event": {
        const set = this.listeners.get(f.name);
        if (set) for (const cb of set) cb(f.payload);
        return;
      }
      case "pong":
        return;
    }
  }

  private onBinary(buf: ArrayBuffer): void {
    if (buf.byteLength < 5) return;
    const head = new DataView(buf, 0, 5);
    const kind = head.getUint8(0);
    const id = head.getUint32(1, true);
    const payload = buf.slice(5);
    if (kind === BIN_CALL_RESULT) {
      const p = this.pending.get(id);
      if (p) {
        this.pending.delete(id);
        p.resolve(payload);
      }
    } else if (kind === BIN_CHANNEL) {
      this.channels.get(id)?.onmessage(payload);
    }
  }

  private onReady(boot: string): void {
    const prevBoot = localStorage.getItem(LS_BOOT);
    localStorage.setItem(LS_BOOT, boot);
    if (prevBoot && prevBoot !== boot && this.everOpened) {
      // 宿主重启过：终端 id、浏览器面板、会话水合状态全变了。前端的
      // 局部对账救不回这些，整页刷新是唯一干净的路。
      location.reload();
      return;
    }
    const wasOpenBefore = this.everOpened;
    this.everOpened = true;
    this.denied = null;
    this.backoff = BACKOFF_MIN_MS;
    this.setStatus("open");
    // 先把攒着的调用发出去，再通知重连 —— 订阅方在 onReconnect 里发的
    // 新订阅要排在旧调用后面，顺序才和它们发起时一致。
    const ws = this.ws;
    if (ws) {
      for (const q of this.queued.splice(0)) ws.send(q.text);
    }
    this.startPing();
    if (wasOpenBefore) {
      for (const cb of this.reconnectCbs) cb();
    }
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer !== undefined || this.halted || !this.token) return;
    this.setStatus(this.everOpened ? "reconnecting" : "connecting");
    // 抖动：同一台宿主重启时几个标签页别在同一毫秒一起撞上去。
    const jitter = Math.random() * 0.25 * this.backoff;
    const delay = this.backoff + jitter;
    this.backoff = Math.min(BACKOFF_MAX_MS, this.backoff * 2);
    this.reconnectTimer = window.setTimeout(() => {
      this.reconnectTimer = undefined;
      this.connect();
    }, delay);
  }

  /** 外界有理由相信网络回来了（online、回前台）：别等退避，立刻试。 */
  private kick(): void {
    if (this.halted || !this.token) return;
    if (this.ws && this.ws.readyState <= WebSocket.OPEN) {
      // 连接看着还在。回前台时补一次心跳检查：锁屏期间 TCP 可能已经
      // 死了但浏览器还没察觉。
      if (Date.now() - this.lastHeard > SILENCE_LIMIT_MS) this.closeSocket();
      else return;
    }
    window.clearTimeout(this.reconnectTimer);
    this.reconnectTimer = undefined;
    this.backoff = BACKOFF_MIN_MS;
    this.connect();
  }

  private startPing(): void {
    this.stopPing();
    this.pingTimer = window.setInterval(() => {
      const ws = this.ws;
      if (!ws || ws.readyState !== WebSocket.OPEN) return;
      if (Date.now() - this.lastHeard > SILENCE_LIMIT_MS) {
        // 宿主那头没声了。关掉触发 onclose → 重连。
        this.closeSocket();
        this.scheduleReconnect();
        return;
      }
      ws.send('{"t":"ping"}');
    }, PING_EVERY_MS);
  }

  private stopPing(): void {
    window.clearInterval(this.pingTimer);
    this.pingTimer = undefined;
  }

  private closeSocket(): void {
    const ws = this.ws;
    this.ws = null;
    this.stopPing();
    if (ws) {
      ws.onclose = null;
      ws.onmessage = null;
      ws.onerror = null;
      try {
        ws.close();
      } catch {
        // 已经关了
      }
    }
  }

  /**
   * 连接断了：未完成的调用全部拒绝，通道表清空。
   *
   * 通道是连接级的 —— 宿主那头绑在这条连接上的 Channel 已随连接作废，
   * 前端这边的 WebChannel 再也不会收到消息，留着只是一张越攒越大的表
   * （每次 ensureLive 重订阅、每次 termAttach 都新建一条）。攒在 queued
   * 里还没发出去的调用连带它们的通道也一起清：调用刚刚被拒了，订阅方
   * 会在 onReconnect 里重新造一条。
   */
  private failAllPending(err: Error): void {
    for (const p of this.pending.values()) p.reject(err);
    this.pending.clear();
    this.queued.length = 0;
    this.channels.clear();
  }

  private setStatus(s: LinkStatus): void {
    if (this.linkStatus === s) return;
    this.linkStatus = s;
    for (const cb of this.statusCbs) cb(s);
  }
}

/**
 * 从地址栏的 `#token=…` 取令牌并抹掉它。
 *
 * 抹掉是必要的：留在地址栏里会进浏览器历史、会被截图、会被"分享此页"
 * 一起发出去。存进 localStorage 之后地址栏只剩干净的首页。
 */
function takeTokenFromHash(): string | null {
  const hash = location.hash.startsWith("#") ? location.hash.slice(1) : location.hash;
  if (!hash) return null;
  const params = new URLSearchParams(hash);
  const token = params.get("token")?.trim();
  if (!token) return null;
  localStorage.setItem(LS_TOKEN, token);
  params.delete("token");
  const rest = params.toString();
  history.replaceState(null, "", `${location.pathname}${location.search}${rest ? `#${rest}` : ""}`);
  return token;
}
