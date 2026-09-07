/**
 * Tauri IPC 传输：桌面窗口里的那份。几乎是透传 —— 这一层存在的意义是让
 * bridge 能在它和 WebSocket 之间切换，而不是给 IPC 加任何东西。
 */

import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import type { HostAppearance, HostChannel, LinkStatus, Transport } from "./types";

export class TauriTransport implements Transport {
  readonly kind = "tauri" as const;

  invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    return invoke<T>(cmd, args);
  }

  channel<T>(): HostChannel<T> {
    // Tauri 的 Channel 自己实现了 toJSON（`__CHANNEL__:<id>`），放进 args
    // 就能被宿主认出来。这里原样返回，只收窄成接口类型。
    return new Channel<T>();
  }

  async listen<T>(event: string, cb: (payload: T) => void): Promise<() => void> {
    return listen<T>(event, (e) => cb(e.payload));
  }

  status(): LinkStatus {
    return "open";
  }

  onStatus(cb: (s: LinkStatus) => void): () => void {
    cb("open");
    return () => {};
  }

  onReconnect(): () => void {
    // IPC 不会断。
    return () => {};
  }

  setAppearance(appearance: HostAppearance): Promise<void> {
    // 参数名 `theme` 对应宿主 `set_appearance(theme: Appearance)`。
    return invoke("set_appearance", { theme: appearance });
  }
}
