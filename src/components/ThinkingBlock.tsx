import { useEffect, useRef, useState } from "react";

import { Chevron } from "./Chevron";
import { Markdown } from "./Markdown";
import { SmoothFold } from "./SmoothFold";

/**
 * 手动展开过的思考块，键是正文前缀。
 *
 * `[约束]` 展开状态不能只存在组件里。直播中的那段思考和它落定后的
 * 条目是**两个不同的 React 实例** —— 直播的挂在列表尾部（见
 * ProcessGroup 和 Transcript 的 thinkingText 分支），落定的进 items.map，
 * key 也对不上。`open` 作为组件内 state 会随重挂载归零：用户正读到
 * 一半，轮次一结束整块就被收起来。
 *
 * 用正文前缀而不是条目 id 当身份：落定前根本没有 id（内核那边的
 * `msg_x-k` 到不了直播这一侧），而思考文本只追加不改写，同一段思考
 * 在落定前后前缀完全一致。
 */
const expandedThinks = new Map<string, true>();
/** 认人够用就行，键留太长白占内存。 */
const thinkKey = (text: string) => text.slice(0, 80);
/** 展开过的块不清会一直攒着。上限之外按最早展开的先丢。 */
const EXPANDED_MAX = 200;

function rememberThink(key: string, open: boolean) {
  if (!open) {
    expandedThinks.delete(key);
    return;
  }
  expandedThinks.set(key, true);
  if (expandedThinks.size > EXPANDED_MAX) {
    const oldest = expandedThinks.keys().next();
    if (!oldest.done) expandedThinks.delete(oldest.value);
  }
}

/**
 * 思考过程：默认折叠（过程不是结论，铺开会把回答挤走），但用户
 * 展开过就一直开着 —— 包括轮次结束、这块从直播实例换成落定条目。
 *
 * 正在流而**没有**展开的那条在标题右侧滚过最新文字，既看得出没卡住，
 * 又不占高度。展开之后正文完整铺开、不限高也不套内层滚动条。
 *
 * 正文走和回答同一套 markdown：模型思考时照样写列表、代码块、`标记`，
 * 摊成纯文本就是满屏的星号和井号，比渲染过的更难读。收起时 SmoothFold
 * 不挂载孩子，历史里的思考不会白白 parse 一遍。
 *
 * 独立成文件：过程组（ProcessFold）和 Task 卡片里的子时间线（ToolCard）
 * 都要用它，而过程组本身依赖 ToolCard —— 放在任一边都成环。
 */
export function ThinkingBlock({ text, live }: { text: string; live?: boolean }) {
  const [open, setOpen] = useState(() => expandedThinks.has(thinkKey(text)));

  // 直播期间正文在长，短思考的前缀会跟着变（长到 80 字后才定）。
  // 展开状态得跟着搬家，否则落定时按最终文本去查，扑空。
  const keyRef = useRef(thinkKey(text));
  useEffect(() => {
    const next = thinkKey(text);
    const prev = keyRef.current;
    if (next === prev) return;
    keyRef.current = next;
    if (expandedThinks.delete(prev)) expandedThinks.set(next, true);
  }, [text]);

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
          const next = !open;
          setOpen(next);
          rememberThink(keyRef.current, next);
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
      <SmoothFold open={open}>
        <div className="think-body">
          <Markdown text={text} breaks />
        </div>
      </SmoothFold>
    </div>
  );
}
