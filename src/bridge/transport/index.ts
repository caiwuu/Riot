/**
 * 选一条传输：有 `__TAURI_INTERNALS__` 就是桌面窗口，否则是浏览器。
 *
 * 判据只看这一个全局：它由 Tauri 在 webview 里注入，普通浏览器没有。
 * 不看 UA —— Tauri 的 webview UA 和系统浏览器几乎一样。
 *
 * 单例：整个前端只有一条连接。传输在模块加载时就建好（浏览器那份立刻
 * 开始连），App 挂载前 RemoteGate 就能看到状态。
 */

import { TauriTransport } from "./tauri";
import type { Transport } from "./types";
import { WebTransport } from "./web";

export type { HostChannel, LinkStatus, Transport } from "./types";
export { TransportDisconnected } from "./types";

function detect(): Transport {
  if (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window) {
    return new TauriTransport();
  }
  return new WebTransport();
}

export const transport: Transport = detect();

/** 浏览器里的那份传输（令牌管理只对它有意义）。桌面里是 null。 */
export const webTransport: WebTransport | null =
  transport instanceof WebTransport ? transport : null;

/**
 * 宿主能力表。界面按它决定某些入口画不画：手机网页上"在访达中显示"
 * 没有对象，系统文件对话框也不存在。
 */
export const host = {
  kind: transport.kind,
  /** 前端拿到的路径是不是这台设备上的路径（拖放、剪贴板、系统对话框）。 */
  nativePaths: transport.kind === "tauri",
  /** 能不能让宿主机在本地打开文件 / 目录（访达、默认应用）。 */
  openLocal: transport.kind === "tauri",
  /** 有没有原生窗口（红绿灯让位、窗口标题、全屏）。 */
  nativeWindow: transport.kind === "tauri",
} as const;
