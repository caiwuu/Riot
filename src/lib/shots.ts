/**
 * 待发/待编的图片附件。底部输入框和消息内联编辑共用同一套 Composer：
 * 缩图、读 File、认剪贴板附件，这些不该各写一份。
 */

import type { ImageInput } from "../bridge";
import { t } from "../i18n";

/** 待发的一张图。`data` 是 base64，不含 `data:` 前缀。 */
export interface Shot {
  id: string;
  name: string;
  mediaType: string;
  data: string;
}

/**
 * 一条消息最多附几张图。
 *
 * 不是技术上限，是成本上限:每张图都要过一遍模型的视觉编码，五张已经能吃掉
 * 相当可观的一段上下文。真要看更多，分两条消息发更清楚。
 */
export const MAX_SHOTS = 5;

/**
 * 缩到长边不超过这个值。
 *
 * 1568 是 Anthropic 文档给的"再大也不会更清楚"的门槛，两家的视觉编码都在
 * 这个量级上把图切成图块。粘一张 Retina 截图往往是 3000 多宽，缩一半之后
 * 体积掉到四分之一，而模型看到的信息一样多。
 */
const MAX_EDGE = 1568;

/** 认得出是图片的扩展名。拖进来的路径靠它分流。 */
export const IMAGE_EXT = /\.(png|jpe?g|gif|webp)$/i;

/** 粘贴快捷键在界面上怎么写。 */
export const PASTE_KEY = navigator.userAgent.includes("Mac") ? "⌘V" : "Ctrl+V";

/** 看着像一条绝对路径吗。三种写法:`/a/b`、`file://…`、`C:\a\b` 或 UNC。 */
export function looksAbsolute(line: string): boolean {
  return (
    line.startsWith("/") ||
    line.startsWith("file://") ||
    line.startsWith("\\\\") ||
    /^[A-Za-z]:[\\/]/.test(line)
  );
}

/**
 * 这次粘贴带的是附件吗（图、或在文件管理器里复制的文件）。
 *
 * 三条判据满足一条就算:
 * - `files` 有东西 —— 截图这种剪贴板里躺着像素的；
 * - types 里有 `Files` —— webview 认出了文件；
 * - 文字整段都是绝对路径 —— 在访达里 ⌘C 一个文件，WebKit 只把**路径当
 *   文字**递过来，前两条都是空的。真正的路径要再问一次系统粘贴板
 *   （见 `clipboardPaths`），这里只负责决定"值不值得问"。
 *
 * 宁可问多了:一行以 `/` 开头的普通文字（shell 命令、注释）会白问一次
 * IPC，然后按文本粘贴，用户看不出区别。
 */
export function hasAttachment(dt: DataTransfer | null): boolean {
  if (!dt) return false;
  if (dt.files.length > 0 || dt.types.includes("Files")) return true;
  const lines = dt
    .getData("text/plain")
    .split("\n")
    .filter((l) => l.trim());
  return lines.length > 0 && lines.every(looksAbsolute);
}

/** 把 webview 的 `File` 读成待发的图。 */
export async function toShot(file: File): Promise<Shot> {
  const buf = await file.arrayBuffer();
  return {
    id: `${file.name}-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`,
    name: file.name || t("composer.attach.pastedImage"),
    mediaType: file.type || "image/png",
    data: bytesToBase64(new Uint8Array(buf)),
  };
}

/**
 * 长边超了就缩，并统一转成 JPEG。
 *
 * 原图是 PNG 的截图尤其值得转:同样内容 JPEG 往往只有三分之一大，而模型
 * 判断的是布局和颜色，不是无损像素。
 *
 * 缩不动（canvas 用不了、图解不开）时原样返回 —— 有图比没图好。
 */
export async function shrink(shot: Shot): Promise<Shot> {
  try {
    const img = new Image();
    img.src = `data:${shot.mediaType};base64,${shot.data}`;
    await img.decode();
    const edge = Math.max(img.naturalWidth, img.naturalHeight);
    if (edge <= MAX_EDGE) return shot;

    const scale = MAX_EDGE / edge;
    const canvas = document.createElement("canvas");
    canvas.width = Math.round(img.naturalWidth * scale);
    canvas.height = Math.round(img.naturalHeight * scale);
    const ctx = canvas.getContext("2d");
    if (!ctx) return shot;
    ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
    const url = canvas.toDataURL("image/jpeg", 0.85);
    const data = url.slice(url.indexOf(",") + 1);
    return { ...shot, mediaType: "image/jpeg", data };
  } catch {
    return shot;
  }
}

/**
 * 字节转 base64。
 *
 * 分块喂给 `String.fromCharCode`:一次展开几 MB 的数组会超过参数个数上限，
 * 表现是 `RangeError: too many arguments`，而那个报错完全不像"图太大"。
 */
export function bytesToBase64(bytes: Uint8Array): string {
  const CHUNK = 0x8000;
  let binary = "";
  for (let i = 0; i < bytes.length; i += CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(binary);
}

/** 气泡回显的 data URL → 编辑框里的待发图。 */
export function shotsFromDataUrls(urls: string[]): Shot[] {
  return urls.map((src, i) => {
    const m = /^data:([^;]+);base64,([\s\S]+)$/.exec(src);
    return {
      id: `existing-${i}`,
      name: t("composer.attach.pastedImage"),
      mediaType: m?.[1] ?? "image/png",
      data: m?.[2] ?? "",
    };
  });
}

export function shotToInput(s: Shot): ImageInput {
  return { mediaType: s.mediaType, data: s.data };
}

export function shotDataUrl(s: Shot): string {
  return `data:${s.mediaType};base64,${s.data}`;
}

/** 收下一批图，超上限时截掉并告诉调用方多了几张。 */
export async function mergeShots(
  prev: Shot[],
  items: { data: string; mediaType: string; name: string }[],
): Promise<{ shots: Shot[]; extra: number }> {
  const scaled = await Promise.all(
    items.map((it) =>
      shrink({
        id: `${it.name}-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`,
        ...it,
      }),
    ),
  );
  const merged = [...prev, ...scaled];
  return {
    shots: merged.slice(0, MAX_SHOTS),
    extra: Math.max(0, merged.length - MAX_SHOTS),
  };
}
