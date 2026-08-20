import { memo, useEffect, useRef, useState } from "react";

import type { Item } from "../hooks/useSession";
import { Chevron } from "./Chevron";
import { ToolCard } from "./ToolCard";

/** 能折进过程组的条目：思考和工具调用。 */
export type FoldItem = Extract<Item, { kind: "thinking" | "tool" }>;

/** 时间线的渲染块：普通条目原样一行，连续的过程条目折成一组。 */
export type Block =
  | { kind: "row"; item: Item }
  | { kind: "fold"; id: string; items: FoldItem[] };

/**
 * 不折叠的工具：编辑类卡片默认展开 diff / 内容（见 ToolCard），
 * 是用户要看的工作产物 —— 折进组里等于把成果藏起来。
 */
const KEEP_VISIBLE = new Set(["Edit", "Write", "MultiEdit", "NotebookEdit"]);

function foldable(it: Item): it is FoldItem {
  if (it.kind === "thinking") return true;
  if (it.kind !== "tool") return false;
  if (KEEP_VISIBLE.has(it.name)) return false;
  // 还在跑的工具不折：它是"现在正在发生的事"，得留在屏幕上直播
  // （转圈、实时输出都在这张卡上）。跑完落定，下一次分组自然并进
  // 相邻的组 —— 生成过程中的折叠就是这么逐步发生的。
  if (it.status === "running") return false;
  // 截图 / 读图不折：图的意义就是给人看（ToolCard 里默认展开的理由），
  // 折进组里用户不知道图已经在这，会转头让模型"把图贴出来"。
  if (it.resultImage || it.resultImagePath) return false;
  return true;
}

/**
 * 把时间线切成渲染块：连续 ≥2 条思考 / 工具折成一组。
 *
 * 长探索是"思考、Read、思考、Grep…"几十行连排，把回答挤出屏幕 ——
 * 折成一行摘要后，对话流里剩下的才是内容本身。单独一条不折：
 * 一行折成一行没省地方，反而多一次点击。
 */
export function groupBlocks(items: Item[]): Block[] {
  const blocks: Block[] = [];
  let run: FoldItem[] = [];
  const flush = () => {
    const first = run[0];
    if (!first) return;
    if (run.length >= 2) blocks.push({ kind: "fold", id: first.id, items: run });
    else blocks.push({ kind: "row", item: first });
    run = [];
  };
  for (const it of items) {
    if (foldable(it)) {
      run.push(it);
    } else {
      flush();
      blocks.push({ kind: "row", item: it });
    }
  }
  flush();
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
 * 一段已完成的探索过程，折成一行摘要（学 Cursor 的 "Explored …"）。
 *
 * 生成过程中同样生效：已落定的步骤随做随折，正在跑的那一步被
 * foldable 排除、单独成行直播 —— 屏幕上任何时刻只有"一行过去 +
 * 一行现在"。默认收着，点开还原成原来的逐条卡片；组在长大时
 * （key 是首条 id，稳定）展开状态不会丢。
 */
export const ProcessGroup = memo(
  function ProcessGroup({ items }: { items: FoldItem[] }) {
    const [open, setOpen] = useState(false);
    const summary = foldSummary(items);
    // 失败必须透出来 —— 折叠不能把"中间有一步炸了"也一起藏掉。
    const fails = items.filter((it) => it.kind === "tool" && it.status === "error").length;

    return (
      <div className="fold-block">
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
          <span className="fold-label" title={summary}>
            {summary}
          </span>
          <span className="fold-count">{items.length} 步</span>
          {fails > 0 ? <span className="fold-fail">{fails} 项失败</span> : null}
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
          </div>
        ) : null}
      </div>
    );
  },
  // items 每次分组都是新数组，浅比较必然失败；逐元素比引用 ——
  // useSession 更新条目走的是整条替换，引用相等就是没变。
  (a, b) => a.items.length === b.items.length && a.items.every((it, i) => it === b.items[i]),
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

  useEffect(() => {
    if (!live || !open) return;
    const el = bodyRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [text, live, open]);

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
        onClick={() => setOpen(!open)}
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
        <div className="think-body" ref={bodyRef}>
          {text}
        </div>
      ) : null}
    </div>
  );
}
