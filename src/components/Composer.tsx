/**
 * 输入区：contenteditable 编辑器（引用块/命令块）、附件（截图/文件）、
 * 斜杠菜单、@ 补全、模式与模型选择、排队面板。从 App.tsx 拆出。
 *
 * 纯文本解析在 `../lib/promptText`（与 Transcript 共用一份规则）；
 * contenteditable 的块机械在 `../lib/chipEditor`（与消息编辑框共用）；
 * 这里只放 Composer 组件本身。
 */

import { useEffect, useRef, useState } from "react";

import {
  clipboardPaths,
  compactSession,
  type ConfigStatus,
  decodePickFromComposer,
  hasActiveKey,
  type ImageInput,
  type PermissionMode,
  pickFiles,
  type ProviderConfig,
  readImage,
  searchFiles,
  setConfig as saveConfig,
  setPermissionMode,
  type SlashCommand,
  slashCommands,
  slashExpand,
  subscribeDragDrop,
} from "../bridge";
import { type QueuedItem, type WithdrawnPrompt } from "../hooks/useSession";
import {
  SLASH_QUERY_RE,
  SLASH_SUBMIT_RE,
  type Seg,
  promoteLeadingCmd,
  promptToSegs,
  segsText,
  segsToPrompt,
} from "../lib/promptText";
import {
  DEFAULT_COMPACT_THRESHOLD,
  WINDOW_PRESETS,
  compactThresholdForWindow,
  fmtTokens,
} from "../lib/contextWindow";
import { type ChipSeg, isChipSeg } from "../lib/chips";
import {
  caretToEnd,
  dropQueryAtCaret,
  handleChipKey,
  insertChipAtCaret,
  normalizePads,
  queryAtCaret,
  readEditor,
  writeEditor,
} from "../lib/chipEditor";
import { mergeSampling } from "../lib/sampling";
import { Chevron } from "./Chevron";
import { Chip, FileChip } from "./Chip";
import { ConfirmDialog, type ConfirmRequest } from "./ConfirmDialog";
import { ContextRing } from "./ContextRing";
import { ArrowUpIcon, PencilIcon, PlusIcon, StopIcon, TrashIcon } from "./icons";
import { ModeMenu, Picker, type PickerSection, modelLabel } from "./pickers";

const drafts = new Map<string, Seg[]>();

/**
 * 权限模式的 UI 缓存，理由同上，但它错了会出安全问题而不只是显示问题。
 *
 * Chat 按会话 id 重挂载，Composer 的本地 state 跟着一起丢。少了这层
 * 缓存，模式就退回全局默认值显示，而宿主那边还是用户选的那个 ——
 * 屏幕上写着「每次询问」，实际每一步都在静默放行。
 */
const modeCache = new Map<string, PermissionMode>();

/** 待发的一张图。`data` 是 base64，不含 `data:` 前缀。 */
interface Shot {
  id: string;
  name: string;
  mediaType: string;
  data: string;
}

/**
 * 每个会话待发的截图。和 drafts 同一个问题：Chat 按会话 id 重挂载，
 * 粘贴的图是组件 state，切走再切回就没了 —— 文字有 drafts 兜着，
 * 图同样是用户放进输入框的内容，不该丢。发送或删除后由同步 effect 清掉。
 */
const shotsCache = new Map<string, Shot[]>();

/**
 * 会话没了，它在输入框这边留下的东西也该走。
 *
 * 这三个 Map 都按会话 id 长期存活，正常收敛只发生在"内容被清空"那条
 * 路上（草稿删干净、图发出去）。而用户删掉一个还留着草稿和待发截图的
 * 会话时，那条路根本不会走 —— 截图是 base64，留下就是几兆内存挂到
 * 进程退出。由 App 的 dropSessionWorkbench 统一调用。
 */
export function forgetComposerSession(sessionId: string) {
  drafts.delete(sessionId);
  modeCache.delete(sessionId);
  shotsCache.delete(sessionId);
}

/**
 * 一条消息最多附几张图。
 *
 * 不是技术上限，是成本上限:每张图都要过一遍模型的视觉编码，五张已经能吃掉
 * 相当可观的一段上下文。真要看更多，分两条消息发更清楚。
 */
const MAX_SHOTS = 5;

/**
 * 缩到长边不超过这个值。
 *
 * 1568 是 Anthropic 文档给的"再大也不会更清楚"的门槛，两家的视觉编码都在
 * 这个量级上把图切成图块。粘一张 Retina 截图往往是 3000 多宽，缩一半之后
 * 体积掉到四分之一，而模型看到的信息一样多。
 */
const MAX_EDGE = 1568;

/** 认得出是图片的扩展名。拖进来的路径靠它分流。 */
const IMAGE_EXT = /\.(png|jpe?g|gif|webp)$/i;

/** 粘贴快捷键在界面上怎么写。 */
const PASTE_KEY = navigator.userAgent.includes("Mac") ? "⌘V" : "Ctrl+V";

/** 看着像一条绝对路径吗。三种写法:`/a/b`、`file://…`、`C:\a\b` 或 UNC。 */
function looksAbsolute(line: string): boolean {
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
function hasAttachment(dt: DataTransfer | null): boolean {
  if (!dt) return false;
  if (dt.files.length > 0 || dt.types.includes("Files")) return true;
  const lines = dt
    .getData("text/plain")
    .split("\n")
    .filter((l) => l.trim());
  return lines.length > 0 && lines.every(looksAbsolute);
}

/** 把 webview 的 `File` 读成待发的图。 */
async function toShot(file: File): Promise<Shot> {
  const buf = await file.arrayBuffer();
  return {
    id: `${file.name}-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`,
    name: file.name || "粘贴的图片",
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
async function shrink(shot: Shot): Promise<Shot> {
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
function bytesToBase64(bytes: Uint8Array): string {
  const CHUNK = 0x8000;
  let binary = "";
  for (let i = 0; i < bytes.length; i += CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(binary);
}

/**
 * 排队面板：模型跑动中发的插话停在这里（Cursor 同款交互），当前任务
 * **完全跑完**才自动发出、变成对话气泡 —— 中途不插队。想立刻处理就点
 * ↑（停止当前轮，优先发这条）；也可以撤回编辑、删除。
 */
export function QueuePanel({
  queued,
  onEdit,
  onSendNow,
  onDelete,
}: {
  queued: QueuedItem[];
  onEdit: (id: string) => void;
  onSendNow: (id: string) => void;
  onDelete: (id: string) => void;
}) {
  const [open, setOpen] = useState(true);
  return (
    <div className="queue-panel">
      <button type="button" className="queue-head" onClick={() => setOpen((v) => !v)}>
        <Chevron open={open} />
        {queued.length} 条排队
      </button>
      {open
        ? queued.map((q) => (
            <div className="queue-row" key={q.id}>
              <span className="queue-ring" aria-hidden />
              <span className="queue-text" title={q.text}>
                {q.text || "（仅图片）"}
              </span>
              {q.images.length > 0 ? (
                <span className="queue-imgs">{q.images.length} 图</span>
              ) : null}
              {q.refs.length > 0 ? (
                <span className="queue-imgs" title={q.refs.join("\n")}>
                  {q.refs.length} 文件
                </span>
              ) : null}
              <span className="queue-actions">
                <button
                  type="button"
                  title="编辑（放回输入框）"
                  aria-label="编辑"
                  onClick={() => onEdit(q.id)}
                >
                  <PencilIcon />
                </button>
                <button
                  type="button"
                  title="立即发送（停止当前轮，优先处理这条）"
                  aria-label="立即发送"
                  onClick={() => onSendNow(q.id)}
                >
                  <ArrowUpIcon />
                </button>
                <button type="button" title="删除" aria-label="删除" onClick={() => onDelete(q.id)}>
                  <TrashIcon />
                </button>
              </span>
            </div>
          ))
        : null}
    </div>
  );
}

export function Composer({
  sessionId,
  workspace,
  workspaceMissing,
  onMissingWorkspace,
  busy,
  config,
  onConfig,
  initialMode,
  hostMode,
  tokens,
  queued,
  onQueueDelete,
  onQueueEdit,
  onQueueSendNow,
  onSend,
  onStop,
  withdrawn,
  onWithdrawnRestored,
  onOpenSettings,
  insertText,
  onInserted,
  armed = true,
}: {
  sessionId: string;
  /** 会话的项目根。斜杠命令要按它找项目级 commands/。 */
  workspace: string;
  workspaceMissing?: boolean;
  onMissingWorkspace?: () => void;
  busy: boolean;
  config: ConfigStatus;
  onConfig: (s: ConfigStatus) => void;
  /** 宿主侧这个会话的当前模式，不是全局默认值。 */
  initialMode: PermissionMode;
  /** 宿主主动切的模式（批准计划）。null = 没发生过。 */
  hostMode: PermissionMode | null;
  tokens: { input: number; output: number; context: number };
  /** 排队面板：跑轮中发的、还没注入对话的插话。 */
  queued: QueuedItem[];
  onQueueDelete: (id: string) => void;
  onQueueEdit: (
    id: string,
  ) => Promise<{ text: string; images: ImageInput[]; refs: string[] } | null>;
  onQueueSendNow: (id: string) => void;
  /** 返回 false = 没发出去（hook 拦了、模型没配好），输入要放回输入框。 */
  onSend: (t: string, images: ImageInput[], refs: string[]) => Promise<boolean>;
  onStop: () => void;
  /** 被撤回的提问（模型没开口就停了）。放回输入框，然后 `onWithdrawnRestored`。 */
  withdrawn: WithdrawnPrompt | null;
  onWithdrawnRestored: () => void;
  onOpenSettings: () => void;
  /** 外部要塞进来的一段文字（终端选中的输出）。null = 没有。 */
  insertText?: string | null;
  onInserted?: () => void;
  /** 前台才接全局拖放 / 粘贴。隐藏的保活实例不能跟前台抢。 */
  armed?: boolean;
}) {
  // 编辑区是**非受控**的：内容住在 DOM 里，这些 state 只是它的投影。
  // 受控写法（每次输入都回写 innerHTML）会在每一次按键后重置光标，
  // 中文输入法更是直接不能用。
  const [draft, setDraftRaw] = useState(() => segsText(drafts.get(sessionId) ?? []));
  const [mode, setMode] = useState<PermissionMode>(
    () => modeCache.get(sessionId) ?? initialMode,
  );

  // 宿主主动切换（批准计划时用户选的执行档）→ 界面跟上。
  // 回写 setPermissionMode 是为了把模式落进会话索引（幂等 —— 宿主
  // 内存里已经是这个值了，这一步只补持久化）。
  useEffect(() => {
    if (!hostMode) return;
    setMode(hostMode);
    modeCache.set(sessionId, hostMode);
    void setPermissionMode(sessionId, hostMode).catch(() => {});
  }, [hostMode, sessionId]);
  const [modeConfirm, setModeConfirm] = useState<ConfirmRequest | null>(null);
  /** 这个会话可用的斜杠命令 + 技能。每次挂载拉一次（用户加了 .md 切一下会话就有）。 */
  const [commands, setCommands] = useState<SlashCommand[]>([]);
  /** 补全菜单里高亮到第几条。 */
  const [slashPick, setSlashPick] = useState(0);
  /** `@` 引用的候选文件。 */
  const [fileHits, setFileHits] = useState<string[]>([]);
  const [filePick, setFilePick] = useState(0);
  /** 光标前那个没敲完的 `@查询`。undefined = 不在引用语境里。 */
  const [mentionQuery, setMentionQuery] = useState<string | undefined>(undefined);
  /** 斜杠命令的执行反馈（压缩中、展开失败）。 */
  const [slashNote, setSlashNote] = useState("");
  /** 待发的图。发出去就清空。挂载时从模块级缓存恢复（见 shotsCache）。 */
  const [shots, setShots] = useState<Shot[]>(() => shotsCache.get(sessionId) ?? []);

  // 写通到模块级缓存。挂在 effect 而不是每个 setShots 调用点：
  // 调用点有五六处（粘贴、拖放、删除、发送、失败回滚），漏一处
  // 就是一个静默丢图的洞。
  useEffect(() => {
    if (shots.length) shotsCache.set(sessionId, shots);
    else shotsCache.delete(sessionId);
  }, [sessionId, shots]);
  /**
   * 编辑区里的块（按出现顺序）。发出去就清空。
   *
   * `@wechat.html` 是给解析器看的写法，让用户对着它编辑（删一半、光标
   * 插在中间）只会把引用弄坏。块是一个整体：点 ✕ 或退格整个删掉。
   *
   * `[约束]` 三种块合在一个 state 里，"输入框算不算空"才只有一个判据。
   * 拆成 refs/elems/cmdName 三份时这个判断在五处各写一遍，加第四种块漏
   * 一处就是"只放了一个块的输入框被当成空的" —— 占位符不让开、发送键
   * 点不动，而用户明明看见自己放了东西进去。
   */
  const [chips, setChips] = useState<ChipSeg[]>([]);
  /** 拖/选进来失败的那一条。附件是"扔进去就走"的操作，不报的话用户以为成了。 */
  const [dropError, setDropError] = useState("");
  const [dragging, setDragging] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  // 中文 IME：确认候选/上屏英文时，keydown(Enter) 常在 compositionend 之后到达，
  // 此时 nativeEvent.isComposing 已是 false，会被误当成发送。用 ref 盖住这一拍。
  const imeRef = useRef(false);

  /** 引用块挑出来的路径。发送时当附件递给宿主。 */
  const refs = chips.flatMap((s) => (s.kind === "ref" ? [s.value] : []));
  /** 已经落成色块的那条命令/技能名。 */
  const cmdName = chips.find((s) => s.kind === "cmd")?.value ?? null;
  /** 编辑区里有东西吗。占位提示看它 —— 图在编辑区外面，不算。 */
  const hasInput = draft.trim().length > 0 || chips.length > 0;
  /** 能发出去吗。只附了图也是一条消息（"看这个截图"就是这么发的）。 */
  const canSend = hasInput || shots.length > 0;

  const cfg = config.config;
  const hasKey = hasActiveKey(config);
  const activeProvider =
    cfg.providers.find((p) => p.id === cfg.activeProvider) ?? cfg.providers[0] ?? null;

  // 内联切换：直接改激活的 provider/model 并回写配置。和设置页共用
  // 同一条 setConfig 通道，宿主 resolve 一次挡住坏状态。切 provider 时
  // 若当前模型不属于新家，跳到新家的第一个模型。
  const switchProvider = (p: ProviderConfig) => {
    if (p.id === cfg.activeProvider) return;
    const model = p.models.some((m) => m.id === cfg.activeModel)
      ? cfg.activeModel
      : (p.models[0]?.id ?? "");
    void saveConfig({ ...cfg, activeProvider: p.id, activeModel: model })
      .then(onConfig)
      .catch(() => {});
  };
  const switchModel = (m: string) => {
    if (m === cfg.activeModel && activeProvider?.id === cfg.activeProvider) return;
    // 菜单里列的是 activeProvider（含 providers[0] 兜底）的模型，所以
    // provider 要一起写。只写 activeModel 的话，active 为空时会留下
    // 「模型有值、provider 是空 id」的配置 —— keyStatus 按空 id 查不到，
    // 表现为 key 已保存、横幅却说没配。
    void saveConfig({
      ...cfg,
      activeProvider: activeProvider?.id ?? cfg.activeProvider,
      activeModel: m,
    })
      .then(onConfig)
      .catch(() => {});
  };

  // 当前模型的上下文窗口。改它改的是这个模型的压缩时机，写回 ModelConfig
  // 持久化 —— 窗口是模型的固有属性，不是这次对话的临时偏好，下次选中它
  // 还该是这个值。
  const activeModelCfg = activeProvider?.models.find((m) => m.id === cfg.activeModel);
  const switchWindow = (raw: string) => {
    if (!activeProvider || !activeModelCfg) return;
    const next = raw ? Number(raw) : undefined;
    if (next === activeModelCfg.contextWindow) return;
    const models = activeProvider.models.map((m) => {
      if (m.id !== activeModelCfg.id) return m;
      // 选「跟随设置」要把键**删掉**而不是写 undefined：配置会被序列化
      // 发给宿主，显式的 null 在那边是"填了个空窗口"，不是"没填"。
      const { contextWindow: _cleared, ...rest } = m;
      return next === undefined ? rest : { ...rest, contextWindow: next };
    });
    void saveConfig({
      ...cfg,
      providers: cfg.providers.map((p) => (p.id === activeProvider.id ? { ...p, models } : p)),
    })
      .then(onConfig)
      .catch(() => {});
  };

  // 这一轮实际会在哪儿触发压缩。和宿主侧同一条规则：填了窗口按窗口推，
  // 没填用设置里的全局值。占用环拿它当分母 —— 环满就是"下一轮要压了"。
  const compactAt = activeModelCfg?.contextWindow
    ? compactThresholdForWindow(
        activeModelCfg.contextWindow,
        // 模型上选了「模型默认」就是不发上限，别再让服务方那层漏下来。
        mergeSampling(activeModelCfg.sampling ?? {}, activeProvider?.sampling ?? {})
          .maxOutputTokens ?? undefined,
      )
    : (cfg.compactThresholdTokens ?? DEFAULT_COMPACT_THRESHOLD);

  const windowSection: PickerSection | undefined = activeModelCfg
    ? {
        title: "上下文窗口",
        items: [
          {
            id: "",
            label: "跟随设置",
            active: !activeModelCfg.contextWindow,
            note: fmtTokens(cfg.compactThresholdTokens ?? DEFAULT_COMPACT_THRESHOLD),
          },
          // 在设置里填过的非档位值（比如 256000）也要列出来，否则用户打开
          // 菜单会看到一项都没亮 —— 像是那个设置没生效。
          ...[
            ...new Set([
              ...WINDOW_PRESETS,
              ...(activeModelCfg.contextWindow ? [activeModelCfg.contextWindow] : []),
            ]),
          ]
            .sort((a, b) => a - b)
            .map((w) => ({
              id: String(w),
              label: fmtTokens(w),
              active: activeModelCfg.contextWindow === w,
            })),
        ],
        onPick: switchWindow,
      }
    : undefined;

  // 技能也在这份清单里 —— 宿主那边把命令和技能并成了一条发现管道
  // （`slash::discover`）。这里曾经自己拉一次 skillsList 再合并，那是
  // 两个真相：优先级规则（内置 > 命令 > 技能）在两处各写一遍，改一边
  // 就会不一致。
  useEffect(() => {
    let alive = true;
    void slashCommands(workspace)
      .then((cmds) => {
        if (alive) setCommands(cmds);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [workspace]);

  /** 把编辑区当前的内容读进 state（每次输入、每次光标移动后调）。 */
  const sync = () => {
    const el = ref.current;
    if (!el) return;
    // 守卫字符先归位：原生删除/剪切可能吃掉守卫或留下孤儿。IME 组字中
    // 不动 DOM —— normalize 合并文本节点会打断组字。
    if (!imeRef.current) normalizePads(el);
    let segs = readEditor(el);
    const known = new Set(commands.map((c) => c.name));
    const promoted = promoteLeadingCmd(segs, known);
    if (promoted) {
      writeEditor(el, promoted);
      caretToEnd(el);
      segs = promoted;
    }
    const text = segsText(segs);
    const chipSegs = segs.filter(isChipSeg);
    setDraftRaw(text);
    setChips(chipSegs);
    setMentionQuery(queryAtCaret(el));
    // 删光内容后浏览器常留一个 `<br>`，读出来是个 "\n"。当成有内容的话，
    // 占位提示不再出现、草稿缓存里也会存下一堆看不见的空行。
    if (text.trim() || chipSegs.length) drafts.set(sessionId, segs);
    else drafts.delete(sessionId);
  };

  /** 程序化改写编辑区内容（清空、回滚、撤回排队项）。 */
  const setContent = (segs: Seg[]) => {
    const el = ref.current;
    if (!el) return;
    writeEditor(el, segs);
    caretToEnd(el);
    sync();
  };

  /**
   * 换掉文字、留下已有的块。
   *
   * `[约束]` 只在"整条文字都要被替换"时用（Esc 清掉半截 `/xxx`）。
   * 别拿它做追加 —— 块会被重排到前面去，用户会看到自己刚插在句中的
   * 引用莫名其妙跳到了句首。要在光标处加东西用 `insertChipAtCaret`。
   */
  const replaceText = (v: string) => {
    const el = ref.current;
    if (!el) return;
    // 留下所有块。按 kind 逐个点名的写法每加一种块就漏一次 —— 元素块
    // 就这么被吞过：取件之后按 Esc 收起半截 `/xxx`，绿块跟着没了。
    const keep = readEditor(el).filter(isChipSeg);
    setContent(v ? [{ kind: "text", value: v }, ...keep] : keep);
  };

  // 终端选中的那段输出：追加到现有草稿后面，不是替换。
  //
  // 包在代码围栏里 —— 报错栈里的尖括号和缩进不这么处理会被 markdown
  // 吃掉一半。追加完把焦点放回输入框，用户接着就能在前面补一句
  // "这个报错怎么回事"，那才是他按下那个键的目的。
  const insertedRef = useRef(onInserted);
  insertedRef.current = onInserted;
  useEffect(() => {
    if (!insertText) return;
    const el = ref.current;
    if (!el) return;
    const cur = readEditor(el);
    // 浏览器取件走的是这条通道，但它不是一段要围栏的文本，而是一个元素 ——
    // 渲染成色块，接在现有内容后面。
    const pick = decodePickFromComposer(insertText);
    if (pick) {
      setContent([...cur, { kind: "elem", value: pick.selector, label: pick.description }]);
    } else {
      // 终端选中那类:整段包进代码围栏（报错栈里的尖括号/缩进不这么处理
      // 会被 markdown 吃掉）。
      const prefix = segsText(cur).trim() ? "\n\n" : "";
      setContent([...cur, { kind: "text", value: `${prefix}\`\`\`\n${insertText}\n\`\`\`\n` }]);
    }
    el.focus();
    caretToEnd(el);
    insertedRef.current?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [insertText]);

  // 切会话：编辑区是非受控的，组件复用时内容不会自己跟着换。
  // 顺带把焦点放进去 —— contenteditable 不吃 autoFocus（React 只对
  // 表单元素生效），少了这一步切完会话得先点一下才能打字。
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    writeEditor(el, drafts.get(sessionId) ?? []);
    if (armed) {
      el.focus();
      caretToEnd(el);
    }
    sync();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  // 保活的会话切回来：编辑区没重挂，得自己把焦点要回来。
  useEffect(() => {
    if (!armed) return;
    ref.current?.focus();
  }, [armed]);

  // 补全菜单只在"还没敲空格"时出：`/co` 出菜单，`/compact 参数` 不出 ——
  // 后者用户已经选定命令在写参数了，菜单只会挡住视线。
  const slashQuery = cmdName ? undefined : SLASH_QUERY_RE.exec(draft)?.[1];
  const matches =
    slashQuery === undefined
      ? []
      : commands
          .filter((c) => c.name.toLowerCase().includes(slashQuery.toLowerCase()))
          // 前缀匹配排在包含匹配前面（敲 `co` 时 `compact` 该在最上面）
          .sort((a, b) => {
            const q = slashQuery.toLowerCase();
            const ap = a.name.toLowerCase().startsWith(q) ? 0 : 1;
            const bp = b.name.toLowerCase().startsWith(q) ? 0 : 1;
            return ap - bp || a.name.localeCompare(b.name);
          })
          .slice(0, 8);
  const pick = Math.min(slashPick, Math.max(matches.length - 1, 0));

  /** 选中一条命令/技能：收成色块，光标贴在块后面直接写参数。 */
  const chooseSlash = (c: SlashCommand) => {
    const el = ref.current;
    if (!el) return;
    // 旧命令块被这一条顶掉，其余的块（文件、页面元素）都留着。
    const keep = readEditor(el).filter((s) => isChipSeg(s) && s.kind !== "cmd");
    setContent([{ kind: "cmd", value: c.name }, ...keep]);
    setSlashPick(0);
    el.focus();
  };

  // `@` 文件引用：认的是**光标处**那个没敲完的 token（由 sync 算出来），
  // 所以在句子中间插引用也能用。
  useEffect(() => {
    if (mentionQuery === undefined) {
      setFileHits([]);
      return;
    }
    // 防抖：每敲一个字都问一次宿主，大仓库上菜单会跳。
    let alive = true;
    const t = setTimeout(() => {
      void searchFiles(sessionId, mentionQuery)
        .then((r) => alive && setFileHits(r))
        .catch(() => {});
    }, 60);
    return () => {
      alive = false;
      clearTimeout(t);
    };
  }, [mentionQuery, sessionId]);

  const fileMatches = mentionQuery === undefined ? [] : fileHits;
  const fpick = Math.min(filePick, Math.max(fileMatches.length - 1, 0));

  /** 选中一个文件：把光标处的 `@查询` 换成一个块，就地插在句子里。 */
  const chooseFile = (p: string) => {
    const el = ref.current;
    if (!el) return;
    el.focus();
    insertChipAtCaret(el, { kind: "ref", value: p });
    setFilePick(0);
    sync();
  };

  const submit = () => {
    const text = draft.trim();
    // 只附了图/只挂了块、什么都没打也算一条消息 —— "看这个截图"、
    // "看看这个文件"都是这么发的（见 canSend）。
    // busy 不拦：模型干活时发的消息进排队面板，内核在安全点注入。
    if (!canSend || !hasKey || !cfg.activeModel) return;

    // 斜杠命令：内置的当场执行，能展开的展开成 prompt 再走正常发送。
    //
    // 普通技能**不**当命令跑（`expandInline` 为假）—— 只把名字发给模型，
    // 由它用 Skill 工具按需加载正文。展开了就等于把几 KB 正文塞进用户可见
    // 的消息，渐进披露白做。写了 disable-model-invocation 的技能例外：
    // 模型的清单里没有它，不展开谁都跑不了。判据由宿主给，见 slash.rs。
    //
    // 认不出的 `/xxx` 原样发出去 —— 用户可能真想跟模型说这个词。
    const sentSegsNow = ref.current ? readEditor(ref.current) : [];
    const cmdSeg = sentSegsNow.find((s) => s.kind === "cmd");
    const cmd = cmdSeg
      ? commands.find((c) => c.name === cmdSeg.value)
      : (() => {
          const slash = SLASH_SUBMIT_RE.exec(text);
          return slash ? commands.find((c) => c.name === slash[1]) : undefined;
        })();
    if (cmd && (cmd.source === "builtin" || cmd.expandInline)) {
      const args = cmdSeg
        ? sentSegsNow
            .filter((s) => s.kind === "text")
            .map((s) => s.value)
            .join("")
            .trim()
        : (SLASH_SUBMIT_RE.exec(text)?.[2] ?? "");
      // 命令块之外的块整个交给 runSlash：失败时要原样放回，成功时元素块
      // 还得接进正文（展开结果里没有它们的位置）。
      const sentChips = sentSegsNow.filter((s) => isChipSeg(s) && s.kind !== "cmd");
      setContent([]);
      setShots([]);
      void runSlash(cmd, args, sentChips);
      return;
    }

    // 乐观清空，被拒了再放回来。清空是为了让"发出去了"这件事立刻可见；
    // 而拒绝路径上宿主既没收下消息、界面也撤掉了气泡 —— 不放回的话，
    // 用户刚打的那段字在两头都不存在了。
    const sent = shots;
    const sentSegs = ref.current ? readEditor(ref.current) : [];
    const sentRefs = refs;
    // 发出去的是**带标记**的文本：块在原位留下 `@路径`（见 segsToPrompt）。
    const prompt = segsToPrompt(sentSegs).trim();
    setContent([]);
    setShots([]);
    void onSend(
      prompt,
      sent.map(({ mediaType, data }) => ({ mediaType, data })),
      sentRefs,
    ).then((ok) => {
      if (ok) return;
      // 连块带字整段放回去（等待期间新打的接在后面）。
      const cur = ref.current ? readEditor(ref.current) : [];
      setContent([...sentSegs, ...cur]);
      setShots((prev) => [...sent, ...prev]);
    });
  };

  /**
   * 执行一条斜杠命令。
   *
   * 自定义命令展开成 prompt 后**当普通消息发出去** —— 模型看到的和
   * 对话流里显示的是同一段文字。藏起原文只会让"模型为什么这么答"
   * 变得无从追溯（切回会话时更是只剩展开结果）。
   */
  const runSlash = async (cmd: SlashCommand, args: string, sentChips: ChipSeg[] = []) => {
    if (cmd.source === "builtin") {
      if (cmd.name === "compact") {
        // 进行中的提示在对话流里（「正在压缩上下文…」），这里不再横幅重复一遍。
        try {
          await compactSession(sessionId);
        } catch (e) {
          setSlashNote(String(e));
        }
      }
      return;
    }
    // 失败时把 `/命令 参数` 和块原样放回去：展开出来的 prompt 是
    // 派生物，用户手里那行才是他打的东西。
    const restore = () => {
      const cur = ref.current ? readEditor(ref.current) : [];
      const back = sentChips.filter(
        (c) => !cur.some((s) => s.kind === c.kind && s.value === c.value),
      );
      setContent([
        { kind: "cmd", value: cmd.name },
        { kind: "text", value: args ? ` ${args}` : "" },
        ...back,
        ...cur,
      ]);
    };
    const sentRefs = sentChips.flatMap((s) => (s.kind === "ref" ? [s.value] : []));
    try {
      const prompt = await slashExpand(sessionId, cmd.name, args);
      if (!prompt) {
        setSlashNote(`/${cmd.name} 展开失败：命令可能刚被删掉`);
        restore();
        return;
      }
      // 元素块接在展开结果后面。文件引用有 refs 附件那条通道，元素块没有 ——
      // 不写进正文的话，用户明明把绿块留在了输入框里，模型却看不见它。
      const elems = sentChips.filter((s) => s.kind === "elem");
      const body = elems.length
        ? `${prompt}\n\n${elems.map((e) => segsToPrompt([e])).join("\n")}`
        : prompt;
      if (!(await onSend(body, [], sentRefs))) restore();
    } catch (e) {
      setSlashNote(String(e));
      restore();
    }
  };

  /**
   * 把一条已经离开输入框的消息放回来（撤回的提问、撤回来改的排队插话）。
   * 原有草稿接在它后面，谁都不丢。
   */
  const putBack = (
    input: { text: string; images: ImageInput[]; refs: string[] },
    imageLabel: string,
  ) => {
    const cur = ref.current ? readEditor(ref.current) : [];
    const held = cur.flatMap((s) => (s.kind === "ref" ? [s.value] : []));
    const gap: Seg[] = segsText(cur).trim() ? [{ kind: "text", value: "\n" }] : [];
    setContent([...promptToSegs(input.text, input.refs, held), ...gap, ...cur]);
    if (input.images.length > 0) {
      setShots((prev) => [
        ...prev,
        ...input.images.map((img, i) => ({
          id: `back-${Date.now()}-${i}`,
          name: `${imageLabel} ${i + 1}`,
          mediaType: img.mediaType,
          data: img.data,
        })),
      ]);
    }
    ref.current?.focus();
  };

  /** 把一条排队插话撤回输入框改。 */
  const editQueued = async (id: string) => {
    const input = await onQueueEdit(id);
    if (!input) return;
    putBack(input, "排队图片");
  };

  // 撤回的提问回到输入框：模型一个字都没给出就被停了，那句话从没被
  // 回答过 —— 用户按停止的意思是"我重说一遍"，而不是"扔掉我刚打的字"。
  //
  // `[约束]` 按 id 记账防重入。撤回往往正好把会话清空，输入框那一刻
  // 从对话区挪回首屏 —— 那是一次**重挂载**，StrictMode 会把挂载时的
  // effect 跑两遍，不挡的话用户看到自己那句话被放回来两份。
  const restoredRef = useRef(onWithdrawnRestored);
  restoredRef.current = onWithdrawnRestored;
  const restoredId = useRef<string | null>(null);
  useEffect(() => {
    if (!withdrawn || restoredId.current === withdrawn.id) return;
    restoredId.current = withdrawn.id;
    putBack(withdrawn, "撤回图片");
    restoredRef.current();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [withdrawn]);

  /**
   * 收下一批图。
   *
   * 在前端先缩一遍:粘一张 Retina 截图动辄五六 MB，原样发过去要么撞服务方的
   * 单图上限，要么白烧一大截上下文 —— 而模型看布局用不到那个分辨率。
   */
  const addShots = async (items: { data: string; mediaType: string; name: string }[]) => {
    const scaled = await Promise.all(
      items.map((it) =>
        shrink({
          id: `${it.name}-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`,
          ...it,
        }),
      ),
    );
    setShots((prev) => {
      const merged = [...prev, ...scaled];
      // 超上限要说出来 —— 静默丢掉的话，用户以为十张全发出去了。
      if (merged.length > MAX_SHOTS) {
        setDropError(`一条消息最多 ${MAX_SHOTS} 张图，已忽略多出的 ${merged.length - MAX_SHOTS} 张。`);
      }
      return merged.slice(0, MAX_SHOTS);
    });
  };

  /** webview 给的 `File`（剪贴板里只有像素的那种）:图片收下，其它的说清为什么不收。 */
  const takeFiles = async (files: File[]) => {
    const images = files.filter((f) => f.type.startsWith("image/"));
    const rest = files.filter((f) => !f.type.startsWith("image/"));

    if (images.length) {
      const read = await Promise.all(images.map(toShot));
      await addShots(read);
    }
    if (rest.length) {
      // 走到这里说明系统没给出路径（`File` 对象自己是没有的）。非图片文件
      // 只能靠路径进对话 —— 引用块认的就是路径。
      setDropError(
        `${rest[0]?.name ?? "这个文件"} 不是图片，而系统没给出它的路径。` +
          `请用左下角的「+」选择，或者在输入框里打 @ 找它。`,
      );
    }
  };

  /** 拖进来或从对话框选的路径:图片读成内容，其它的变成引用块。 */
  const takePaths = async (paths: string[]) => {
    const images = paths.filter((p) => IMAGE_EXT.test(p));
    const files = paths.filter((p) => !IMAGE_EXT.test(p));

    if (images.length) {
      const read = await Promise.all(
        images.map((p) => readImage(p).catch((e: unknown) => String(e))),
      );
      const ok = read.filter((r): r is Awaited<ReturnType<typeof readImage>> =>
        typeof r !== "string",
      );
      await addShots(ok);
      const failed = read.filter((r): r is string => typeof r === "string");
      if (failed.length) setDropError(failed[0] ?? "");
    }

    // 非图片文件走和 `@` 一样的引用块：都是"用户点名了这个文件"，
    // 没道理一个变成块、另一个变成一串裸路径。项目内的收成相对路径，
    // 块上只显示文件名，长路径不会把输入框撑变形。
    if (files.length) {
      const el = ref.current;
      if (el) {
        el.focus();
        // 光标未必在输入框里 —— 拖放和粘贴发生时焦点常在别处，甚至正
        // 选着对话流里的一段文字。不校正的话块会插到那段选区上去。
        const sel = window.getSelection();
        const inside =
          sel && sel.rangeCount > 0 && el.contains(sel.getRangeAt(0).startContainer);
        if (!inside) caretToEnd(el);
        for (const p of files) {
          // 两种分隔符都认:Windows 上拖进来的是 `C:\proj\a.md`。
          const inWs = p.startsWith(`${workspace}/`) || p.startsWith(`${workspace}\\`);
          insertChipAtCaret(el, { kind: "ref", value: inWs ? p.slice(workspace.length + 1) : p });
        }
        sync();
      }
    }
  };

  /**
   * 收下剪贴板里的附件。返回是否真的收下了。
   *
   * 先问宿主要磁盘路径:拿得到就和拖放走同一条路，非图片文件也能变成
   * 引用块。拿不到（剪贴板里只有像素的截图、或非 macOS）再退回 webview
   * 给的 `File`。两样都没有就还给调用方按文字处理。
   */
  const pasteFiles = async (files: File[]): Promise<boolean> => {
    const paths = await clipboardPaths().catch(() => []);
    if (paths.length) {
      await takePaths(paths);
      return true;
    }
    if (files.length) {
      await takeFiles(files);
      return true;
    }
    return false;
  };

  // 拖到窗口任何地方都算数。只认输入框那一小条的话用户得先瞄准，而窗口
  // 大半面积是对话流 —— 拖偏了什么都不会发生，还以为这个功能没做。
  //
  // 处理函数放 ref 里:订阅只在挂载时建一次，而 takePaths 每次渲染都是
  // 新的闭包，直接进依赖数组会让拖放订阅跟着输入框的每一次输入重建。
  const dropRef = useRef(takePaths);
  dropRef.current = takePaths;
  useEffect(() => {
    if (!armed) {
      setDragging(false);
      return;
    }
    return subscribeDragDrop((e) => {
      if (e.kind === "leave") {
        setDragging(false);
        return;
      }
      if (e.kind === "enter") {
        // 没有路径的拖拽（拖一段文字、拖网页里的图）不亮落点提示 ——
        // 亮了却接不住是更糟的反馈。
        setDragging(e.paths.length > 0);
        return;
      }
      if (e.kind !== "drop") return;
      setDragging(false);
      if (e.paths.length) {
        void dropRef.current(e.paths);
      } else {
        setDropError(
          "拖进来的东西在磁盘上没有对应文件（多半是从网页里直接拖的图）。" +
            `复制它，再回到这里 ${PASTE_KEY}。`,
        );
      }
    });
  }, [armed]);

  // 焦点不在输入框时 ⌘V 也算数 —— 在 Finder 里复制完文件回到窗口，第一
  // 反应是直接粘，不会先去点一下输入框。
  //
  // 别处的可编辑元素（终端、设置里的输入框）不抢:那是人家的内容。
  const pasteRef = useRef(pasteFiles);
  pasteRef.current = pasteFiles;
  useEffect(() => {
    if (!armed) return;
    const onPaste = (e: ClipboardEvent) => {
      const t = e.target;
      if (t instanceof Element && t.closest("input, textarea, [contenteditable='true']")) return;
      if (!hasAttachment(e.clipboardData)) return;
      e.preventDefault();
      void pasteRef.current(Array.from(e.clipboardData?.files ?? []));
    };
    document.addEventListener("paste", onPaste);
    return () => document.removeEventListener("paste", onPaste);
  }, [armed]);

  const applyMode = (m: PermissionMode) => {
    const prev = mode;
    setMode(m);
    modeCache.set(sessionId, m);
    // 失败必须回滚到宿主的真实值。这里显示的是"它会不会问我"，
    // 显示成放行而实际在问只是啰嗦，反过来则是用户以为有人把关。
    setPermissionMode(sessionId, m).catch(() => {
      setMode(prev);
      modeCache.set(sessionId, prev);
    });
  };

  const changeMode = (m: PermissionMode) => {
    // 无人值守关掉的是最后一层保护，不能一次点击就生效。
    if (m === "unattended" && mode !== "unattended") {
      setModeConfirm({
        title: "切到无人值守？",
        body: "这个会话之后不会再有任何权限弹窗，包括危险操作。",
        confirmLabel: "确认切换",
        action: () => applyMode(m),
      });
      return;
    }
    applyMode(m);
  };

  return (
    <div className="composer-wrap">
      {/* 落点提示铺满整个窗口 —— 因为落点确实是整个窗口，提示只圈住输入框
          会让人以为必须拖到那一条上。 */}
      {dragging ? (
        <div className="drop-veil" aria-hidden>
          <div className="drop-veil-card">松手，加进输入框</div>
        </div>
      ) : null}

      {workspaceMissing ? (
        <button className="key-banner" onClick={onMissingWorkspace}>
          项目目录已经不在磁盘上。点这里移除或另选目录。
        </button>
      ) : null}

      {/* 三种"还不能发消息"要分开说。都写成"还没有 API key"的话，
          一个服务方都没有的新用户会去找那个根本不存在的 key 输入框。 */}
      {cfg.providers.length === 0 ? (
        <button className="key-banner" onClick={onOpenSettings}>
          还没有配置服务方，点这里添加
        </button>
      ) : !hasKey ? (
        <button className="key-banner" onClick={onOpenSettings}>
          {activeProvider?.name ?? "当前服务方"}还没有 API key，点这里配置
        </button>
      ) : !cfg.activeModel ? (
        <button className="key-banner" onClick={onOpenSettings}>
          {activeProvider?.name ?? "当前服务方"}还没有选中模型，点这里配置
        </button>
      ) : null}

      {dropError ? (
        <button className="key-banner" onClick={() => setDropError("")} title="点击关闭">
          {dropError}
        </button>
      ) : null}

      {slashNote ? (
        <button className="key-banner" onClick={() => setSlashNote("")} title="点击关闭">
          {slashNote}
        </button>
      ) : null}

      {queued.length > 0 ? <QueuePanel queued={queued} onEdit={(id) => void editQueued(id)} onSendNow={onQueueSendNow} onDelete={onQueueDelete} /> : null}

      {matches.length > 0 ? (
        <div className="slash-menu">
          {matches.map((c, i) => (
            <button
              type="button"
              key={c.name}
              className={i === pick ? "slash-item active" : "slash-item"}
              // mousedown 而不是 click：click 之前 textarea 先失焦，
              // 焦点一跑菜单就关了，点击落空。
              onMouseDown={(e) => {
                e.preventDefault();
                chooseSlash(c);
              }}
              onMouseEnter={() => setSlashPick(i)}
            >
              <Chip seg={{ kind: "cmd", value: c.name }} />
              {c.argumentHint ? <span className="slash-hint">{c.argumentHint}</span> : null}
              <span className="slash-desc">{c.description}</span>
              {c.source !== "builtin" ? (
                <span className="slash-src">
                  {c.source === "skill" ? "技能" : c.source === "project" ? "项目" : "全局"}
                </span>
              ) : null}
            </button>
          ))}
        </div>
      ) : null}

      {fileMatches.length > 0 && matches.length === 0 ? (
        <div className="slash-menu">
          {fileMatches.map((p, i) => (
            <button
              type="button"
              key={p}
              className={i === fpick ? "slash-item active" : "slash-item"}
              onMouseDown={(e) => {
                e.preventDefault();
                chooseFile(p);
              }}
              onMouseEnter={() => setFilePick(i)}
            >
              {/* 文件名在前、目录在后：一屏候选里先扫到的是名字。 */}
              <FileChip path={p} />
              <span className="slash-desc">{p}</span>
            </button>
          ))}
        </div>
      ) : null}

      <form
        className={dragging ? "composer dragging" : "composer"}
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        {shots.length ? (
          <div className="attachments">
            {shots.map((s) => (
              <div className="attachment" key={s.id} title={s.name}>
                <img src={`data:${s.mediaType};base64,${s.data}`} alt={s.name} />
                <button
                  type="button"
                  className="attachment-remove"
                  onClick={() => setShots((prev) => prev.filter((x) => x.id !== s.id))}
                  aria-label="移除"
                >
                  ✕
                </button>
              </div>
            ))}
          </div>
        ) : null}

        {/* 引用块住在编辑区里、和文字同一行，所以这里没有单独的块列表。 */}
        <div
          ref={ref}
          className={hasInput ? "composer-input" : "composer-input empty"}
          contentEditable
          suppressContentEditableWarning
          role="textbox"
          aria-multiline="true"
          data-placeholder={
            busy ? "它正在做事…此刻发送会排队，当前任务完成后自动发出" : "描述一个任务，或问点什么"
          }
          onInput={sync}
          // 光标挪动也要重算 `@查询` —— 用户可能把光标移回句子中间的
          // 一个半截 @ 上继续挑文件。
          onKeyUp={sync}
          onMouseUp={sync}
          onBlur={sync}
          // 粘贴板里的图和文件直接收下。这是"看这个截图"最常用的发法 ——
          // 截完图 ⌘V 就完事，不用先存盘再选文件；在 Finder 里复制的文件
          // 同理，粘进来就是一个引用块。
          onPaste={(e) => {
            const files = Array.from(e.clipboardData.files);
            const text = e.clipboardData.getData("text/plain");
            // 富文本粘贴一律降级成纯文本：contenteditable 默认会把网页的
            // 样式、图片、甚至整个表格结构原样塞进来。
            e.preventDefault();
            if (hasAttachment(e.clipboardData)) {
              // 问宿主要路径是一次 IPC，所以只在"看着像附件"时才走这条 ——
              // 每敲一次 ⌘V 都异步一下，粘长文本时会看见一帧空白。
              void pasteFiles(files).then((took) => {
                // 只是一段以 / 开头的普通文字（比如一行 shell 命令），
                // 按纯文本粘贴，别把它吃掉。
                if (took) return;
                ref.current?.focus();
                document.execCommand("insertText", false, text);
                sync();
              });
              return;
            }
            document.execCommand("insertText", false, text);
            sync();
          }}
          onCompositionStart={() => {
            imeRef.current = true;
          }}
          onCompositionEnd={() => {
            // compositionend 与确认用的 Enter 可能跨到下一个宏任务，
            // microtask 不够，用 setTimeout(0) 盖住这一拍。
            setTimeout(() => {
              imeRef.current = false;
            }, 0);
            sync();
          }}
          onKeyDown={(e) => {
            // 色块当原子：退格一次整块删掉，方向键整块跳过。
            // 交给浏览器的话，WebKit 会先把光标塞进块里（或先选中再删）。
            if (
              ref.current &&
              !e.nativeEvent.isComposing &&
              !imeRef.current &&
              handleChipKey(e, ref.current)
            ) {
              e.preventDefault();
              sync();
              return;
            }

            // 补全菜单开着时，方向键和 Tab/Enter 归它用。两个菜单不会
            // 同时开：`/` 要求整条草稿就是命令，`@` 认的是末尾那一段。
            const menu = matches.length > 0 ? "slash" : fileMatches.length > 0 ? "file" : null;
            if (menu && !e.nativeEvent.isComposing && !imeRef.current) {
              const len = menu === "slash" ? matches.length : fileMatches.length;
              const move = menu === "slash" ? setSlashPick : setFilePick;
              if (e.key === "ArrowDown") {
                e.preventDefault();
                move((p) => (p + 1) % len);
                return;
              }
              if (e.key === "ArrowUp") {
                e.preventDefault();
                move((p) => (p - 1 + len) % len);
                return;
              }
              if (e.key === "Escape") {
                e.preventDefault();
                // 斜杠菜单：整条草稿就是那个命令，清掉即可。
                // 文件菜单：正文还在，只把光标前那段 @ 抹掉收起菜单。
                if (menu === "slash") {
                  replaceText("");
                } else {
                  dropQueryAtCaret();
                  sync();
                }
                return;
              }
              if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey && e.keyCode !== 229)) {
                e.preventDefault();
                if (menu === "slash") {
                  const c = matches[pick];
                  if (c) chooseSlash(c);
                } else {
                  const f = fileMatches[fpick];
                  if (f) chooseFile(f);
                }
                return;
              }
            }
            // 空输入时 Esc 中断当前轮 —— 想停不必去够那个停止按钮。
            // 有草稿时 Esc 留给"清空/退出引用"这类局部撤销，不误伤。
            if (e.key === "Escape" && busy && !draft.trim() && !e.nativeEvent.isComposing) {
              e.preventDefault();
              onStop();
              return;
            }
            // 敲空格且整段正好是一条已知命令：收成色块，别留下 `/compact ` 纯文字。
            if (e.key === " " && !e.nativeEvent.isComposing && !imeRef.current) {
              const typed = SLASH_QUERY_RE.exec(draft)?.[1];
              const exact = typed ? commands.find((c) => c.name === typed) : undefined;
              if (exact) {
                e.preventDefault();
                chooseSlash(exact);
                return;
              }
            }
            // 229 = IME 处理中的占位 keyCode，部分 WebView 上比 isComposing 更准
            if (
              e.key === "Enter" &&
              !e.shiftKey &&
              !e.nativeEvent.isComposing &&
              e.keyCode !== 229 &&
              !imeRef.current
            ) {
              e.preventDefault();
              submit();
            }
          }}
        />

        <div className="composer-bar">
          <div className="composer-tools">
            <button
              type="button"
              className="composer-icon"
              onClick={() => void pickFiles().then(takePaths).catch(() => {})}
              title="附加图片或文件"
              aria-label="附加图片或文件"
            >
              <PlusIcon />
            </button>
            <ModeMenu mode={mode} onChange={changeMode} />
            {/* 窄列藏起来：三个 pill 并排是挤的源头，换服务方/模型去设置里也能做。 */}
            <div className="composer-picks">
              <Picker
                title="切换服务方"
                label={activeProvider?.name ?? "选择服务方"}
                items={cfg.providers.map((p) => ({
                  id: p.id,
                  label: p.name,
                  active: p.id === cfg.activeProvider,
                  ...(config.keyStatus[p.id] ? {} : { note: "未配置 key", warn: true }),
                }))}
                onPick={(id) => {
                  const p = cfg.providers.find((x) => x.id === id);
                  if (p) switchProvider(p);
                }}
              />
              <Picker
                title="切换模型"
                label={modelLabel(activeProvider, cfg.activeModel) || "选择模型"}
                items={(activeProvider?.models ?? []).map((m) => ({
                  id: m.id,
                  // 有显示名就用它。菜单里那一列越短越好读，模型 ID 常常很长。
                  label: m.name?.trim() || m.id,
                  active: m.id === cfg.activeModel,
                  ...(m.vision ? { vision: true } : {}),
                  ...(m.contextWindow ? { note: fmtTokens(m.contextWindow) } : {}),
                }))}
                {...(windowSection ? { section: windowSection } : {})}
                emptyHint="这个服务方还没有模型"
                onEmpty={onOpenSettings}
                onPick={switchModel}
              />
            </div>
          </div>
          <div className="composer-actions">
            {tokens.input + tokens.output > 0 ? (
              <ContextRing
                used={tokens.context}
                threshold={compactAt}
                totals={{ input: tokens.input, output: tokens.output }}
                {...(activeModelCfg?.contextWindow
                  ? { window: activeModelCfg.contextWindow }
                  : {})}
              />
            ) : null}
            {/* 停止常驻：只要在忙就显示，不再被"打了字"的发送按钮顶掉 ——
                想中止不必先清空输入。有草稿时它和发送并排，各司其职。 */}
            {busy ? (
              <button type="button" className="send stop" onClick={onStop} title="停止 (Esc)" aria-label="停止">
                <StopIcon />
              </button>
            ) : null}
            {!busy || canSend ? (
              <button
                type="submit"
                className="send"
                disabled={!canSend || !hasKey || !cfg.activeModel}
                title={
                  busy ? "排队发送（当前任务完成后自动发出）" : cfg.activeModel ? "发送" : "先选择一个模型"
                }
                aria-label={busy ? "排队发送" : cfg.activeModel ? "发送" : "先选择一个模型"}
              >
                <ArrowUpIcon />
              </button>
            ) : null}
          </div>
        </div>
      </form>
      {modeConfirm ? (
        <ConfirmDialog c={modeConfirm} onClose={() => setModeConfirm(null)} />
      ) : null}
    </div>
  );
}

