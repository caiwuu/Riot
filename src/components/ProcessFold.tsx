import { memo, useEffect, useRef, useState } from "react";

import type { Item } from "../hooks/useSession";
import { Chevron } from "./Chevron";
import { summarize, ToolCard } from "./ToolCard";

/** 能折进过程组的条目：思考和工具调用。 */
export type FoldItem = Extract<Item, { kind: "thinking" | "tool" }>;

/** 时间线的渲染块：普通条目原样一行，连续的过程条目折成一组。 */
export type Block =
  | { kind: "row"; item: Item }
  | { kind: "fold"; id: string; items: FoldItem[]; live: boolean };

/**
 * 不折叠的工具：编辑类卡片默认展开 diff / 内容（见 ToolCard），
 * 是用户要看的工作产物 —— 折进组里等于把成果藏起来。
 */
const KEEP_VISIBLE = new Set(["Edit", "Write", "MultiEdit", "NotebookEdit"]);

function foldable(it: Item): it is FoldItem {
  if (it.kind === "thinking") return true;
  if (it.kind !== "tool") return false;
  if (KEEP_VISIBLE.has(it.name)) return false;
  // 截图 / 读图不折：图的意义就是给人看（ToolCard 里默认展开的理由），
  // 折进组里用户不知道图已经在这，会转头让模型"把图贴出来"。
  // 运行中还没有图，带图工具落定的瞬间会被这条摘出组、以独立卡现身 ——
  // 那是内容的自然生长，不算跳动。
  if (it.resultImage || it.resultImagePath) return false;
  // 注意：运行中的工具**也折**。早先不折、单独成卡直播，跑完再收进
  // 组 —— 那一收一增的行数变化让生成期的底部随每个工具弹跳一次。
  // 直播职责移到了组头（见 ProcessGroup 的 live 形态），高度恒为一行。
  return true;
}

/**
 * 把时间线切成渲染块：连续 ≥2 条思考 / 工具折成一组。
 *
 * 长探索是"思考、Read、思考、Grep…"几十行连排，把回答挤出屏幕 ——
 * 折成一行摘要后，对话流里剩下的才是内容本身。单独一条不折：
 * 一行折成一行没省地方，反而多一次点击。
 *
 * `liveTail`（正在生成且没在流正文时为真）让尾部那段过程**即使只有
 * 一条也成组**，并标记 live：组从第一步就存在，之后每一步都是并入 ——
 * 组头恒为一行，工具完成只换行内文字，不再有"独立行收进组"的增删。
 * 单条的段落等轮到正文或轮次结束时自然还原成普通一行（组头和单行
 * 都是一行高，这次还原不跳）。
 */
export function groupBlocks(items: Item[], liveTail = false): Block[] {
  const blocks: Block[] = [];
  let run: FoldItem[] = [];
  const flush = (tail: boolean) => {
    const first = run[0];
    if (!first) return;
    const live = tail && liveTail;
    if (run.length >= 2 || live) blocks.push({ kind: "fold", id: first.id, items: run, live });
    else blocks.push({ kind: "row", item: first });
    run = [];
  };
  for (const it of items) {
    if (foldable(it)) {
      run.push(it);
    } else {
      flush(false);
      blocks.push({ kind: "row", item: it });
    }
  }
  flush(true);
  return blocks;
}

/**
 * 折叠头的一句话摘要：按类别数数，读过的文件点名。
 * 只写"14 步"没有信息量 —— 用户扫一眼要能知道这段过程干了什么。
 */
function foldSummary(items: FoldItem[]): string {
  const readNames: string[] = [];
  let reads = 0;
  let searches = 0;
  let cmds = 0;
  let tasks = 0;
  let web = 0;
  let browser = 0;
  let other = 0;
  let thinkChars = 0;

  for (const it of items) {
    if (it.kind === "thinking") {
      thinkChars += it.text.length;
      continue;
    }
    const input = it.input as Record<string, unknown>;
    const str = (k: string) => (typeof input?.[k] === "string" ? (input[k] as string) : "");
    switch (it.name) {
      case "Read": {
        reads++;
        const base = (str("path") || str("file_path")).split("/").pop() ?? "";
        if (base && !readNames.includes(base)) readNames.push(base);
        break;
      }
      case "Grep":
      case "Glob":
      case "LS":
        searches++;
        break;
      case "Bash":
        cmds++;
        break;
      case "Task":
        tasks++;
        break;
      case "WebSearch":
      case "WebFetch":
        web++;
        break;
      default:
        if (it.name.startsWith("Browser")) browser++;
        else other++;
    }
  }

  const parts: string[] = [];
  const first = readNames[0];
  if (reads > 0) {
    if (!first) parts.push(`读取 ${reads} 个文件`);
    else if (readNames.length === 1) parts.push(`读取 ${first}`);
    else parts.push(`读取 ${first} 等 ${readNames.length} 个文件`);
  }
  if (searches) parts.push(`搜索 ${searches} 次`);
  if (cmds) parts.push(`命令 ${cmds} 条`);
  if (tasks) parts.push(`子任务 ${tasks} 个`);
  if (web) parts.push(`联网 ${web} 次`);
  if (browser) parts.push(`浏览器 ${browser} 步`);
  if (other) parts.push(`其他 ${other} 步`);
  // 全是思考没有工具时，摘要落到思考本身。
  if (parts.length === 0) return `思考过程 · 共 ${thinkChars} 字`;
  return parts.join(" · ");
}

/**
 * 组头直播的"当前动作"：正在跑的工具优先（最新输出尾行 > 参数摘要），
 * 没有就落到正在流的思考。都没有（比如模型在生成下一个工具的参数）
 * 返回 null，组头回落到静态摘要 —— 转圈的图标还在，看得出没停。
 */
function liveActivity(
  items: FoldItem[],
  thinkingText?: string,
): { label: string; peek: string } | null {
  const oneLine = (s: string) => s.slice(-160).replace(/\s+/g, " ").trim();
  for (let i = items.length - 1; i >= 0; i--) {
    const it = items[i];
    if (it && it.kind === "tool" && it.status === "running") {
      const tail = it.output.length > 0 ? (it.output[it.output.length - 1] ?? "") : "";
      return { label: it.name, peek: oneLine(tail || summarize(it)) };
    }
  }
  if (thinkingText) return { label: "思考中…", peek: oneLine(thinkingText) };
  return null;
}

/**
 * 一段探索过程，折成一行摘要（学 Cursor 的 "Explored …"）。
 *
 * 生成期间（live）正在跑的工具、正在流的思考都并在组里，组头单行
 * 直播当前动作：转圈 + 工具名 + 最新输出（或思考尾巴）滚过。这段
 * 过程从头到尾只占一行，步骤完成只换行内文字 —— 早先"运行中独立
 * 成卡、完成收进组"的形态切换让底部随每个工具弹跳一次。落定后组头
 * 原地换成静态摘要。默认收着，点开还原成逐条卡片（正在跑的那张
 * 也在里面，实时输出照旧看得到）；组在长大时（key 是首条 id，稳定）
 * 展开状态不会丢。
 */
export const ProcessGroup = memo(
  function ProcessGroup({
    items,
    live = false,
    thinkingText,
  }: {
    items: FoldItem[];
    /** 尾部直播组：这段过程还在进行，组头显示当前动作。 */
    live?: boolean;
    /** 正在流式输出、还没落成条目的思考。直播在组头，展开时排在组尾。 */
    thinkingText?: string;
  }) {
    const [open, setOpen] = useState(false);
    const summary = foldSummary(items);
    // 失败数常驻头部但**弱化** —— 探索里的中途失败（grep 没命中、
    // 试错命令）大多被后续步骤自愈，不值得一块刺眼的红。真要紧的
    // 是"以失败收尾"：最后一步炸了，多半意味着模型接下来要认输，
    // 这种才升警示色。逐条的红色"失败"在展开层，要追查点开就有。
    const fails = items.filter((it) => it.kind === "tool" && it.status === "error").length;
    const last = items[items.length - 1];
    const endedFail = !live && last?.kind === "tool" && last.status === "error";
    const act = live ? liveActivity(items, thinkingText) : null;

    return (
      <div className={live ? "fold-block live" : "fold-block"}>
        <button
          type="button"
          className="fold-head"
          // 同 think-head：点击只为开合，不拿焦点 —— WKWebView 会把
          // focused button 滚进视野，正好滚离底部。
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => setOpen(!open)}
          aria-expanded={open}
        >
          <Chevron open={open} />
          {live ? (
            <span className="fold-live-icon" aria-hidden>
              ◐
            </span>
          ) : null}
          {act ? (
            <>
              <span className="fold-live-label">{act.label}</span>
              {act.peek ? (
                <span className="fold-peek" aria-hidden>
                  <span className="fold-peek-text">{act.peek}</span>
                </span>
              ) : null}
            </>
          ) : (
            <span className="fold-label" title={summary}>
              {summary}
            </span>
          )}
          <span className="fold-count">{items.length} 步</span>
          {fails > 0 ? (
            <span className={endedFail ? "fold-fail fold-fail-final" : "fold-fail"}>
              {fails} 项失败
            </span>
          ) : null}
        </button>
        {open ? (
          <div className="fold-body">
            {items.map((it) =>
              it.kind === "thinking" ? (
                <ThinkingBlock key={it.id} text={it.text} />
              ) : (
                <ToolCard key={it.id} tool={it} />
              ),
            )}
            {thinkingText ? <ThinkingBlock text={thinkingText} live /> : null}
          </div>
        ) : null}
      </div>
    );
  },
  // items 每次分组都是新数组，浅比较必然失败；逐元素比引用 ——
  // useSession 更新条目走的是整条替换，引用相等就是没变。
  (a, b) =>
    a.live === b.live &&
    a.thinkingText === b.thinkingText &&
    a.items.length === b.items.length &&
    a.items.every((it, i) => it === b.items[i]),
);

/**
 * 思考过程：始终默认折叠（过程不是结论，铺开会把回答挤走）。
 *
 * 正在流的那条不展开正文，而是在标题右侧滚过最新的思考文字 ——
 * 既能看出"没卡住"，收尾落定时高度又几乎不变。早先直播时整块展开，
 * 收尾一折叠底部内容瞬间矮掉几百像素，贴底跟随会被这次跳变打断。
 */
export function ThinkingBlock({ text, live }: { text: string; live?: boolean }) {
  const [open, setOpen] = useState(false);
  const bodyRef = useRef<HTMLDivElement>(null);
  // 展开的直播正文是否贴底跟随。同主消息流的规则:向上滚立即交出
  // 控制权，自己滚回底部才恢复 —— 早先无条件贴底，每个字都把用户
  // 拽回最下面，流式期间根本翻不上去。
  const bodyStick = useRef(true);
  const bodyTop = useRef(0);

  useEffect(() => {
    if (!live || !open || !bodyStick.current) return;
    const el = bodyRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [text, live, open]);

  const onBodyScroll = () => {
    const el = bodyRef.current;
    if (!el) return;
    const top = el.scrollTop;
    const delta = top - bodyTop.current;
    bodyTop.current = top;
    const gap = el.scrollHeight - top - el.clientHeight;
    // 程序化贴底只发生在 stick 为真时且方向向下，不会误触解锁，
    // 所以这里不需要主消息流那样的 pinning 挡板。
    if (delta < 0 && gap > 1) bodyStick.current = false;
    else if (delta > 0 && gap < 16) bodyStick.current = true;
  };

  // 最近一段文字压成一行当预览。换行换成空格 —— 预览框只有一行高。
  const peek = live && !open ? text.slice(-160).replace(/\s+/g, " ").trim() : "";

  return (
    <div className={live ? "think-block live" : "think-block"}>
      <button
        type="button"
        className="think-head"
        // 点标题只为开合，不要把焦点吃过去 —— WKWebView 对 focused
        // button 会默认滚进视野，正好滚到这条思考、离开底部。
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => {
          // 重新展开时从底部（最新内容）看起，跟随也一并恢复。
          bodyStick.current = true;
          setOpen(!open);
        }}
      >
        <Chevron open={open} />
        <span className="think-label">{live ? "思考中…" : "思考过程"}</span>
        <span className="think-chars">{text.length} 字</span>
        {peek ? (
          <span className="think-peek" aria-hidden>
            <span className="think-peek-text">{peek}</span>
          </span>
        ) : null}
      </button>
      {open ? (
        <div className="think-body" ref={bodyRef} onScroll={onBodyScroll}>
          {text}
        </div>
      ) : null}
    </div>
  );
}
