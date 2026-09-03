//! 文件改动列表:会话改动条和 Git 抽屉共用的渲染。
//!
//! 两个视图的数据形状相同(FileChange),差别只在来源和空态文案 ——
//! 逐文件的行、hunk、复制路径这些交互只写一份,两边不会慢慢长歪。
//!
//! 行是无边框的紧凑列表(Cursor 同款):图标认文件类型、颜色认状态,
//! 一屏能扫二十个文件。每行一个框的话,十个文件就把面板占满了。

import { useContext, useLayoutEffect, useRef, useState } from "react";

import type { FileChange } from "../bridge";
import { joinRoot, looksAbsPath } from "../pathDisplay";
import { Chevron } from "./Chevron";
import { FileIcon } from "./FileIcon";
import { openFilePreview } from "./FilePreview";
import { ProjectRootContext } from "./Markdown";

const STATUS_LABEL: Record<FileChange["status"], string> = {
  created: "新增",
  modified: "修改",
  deleted: "删除",
  renamed: "重命名",
};

/** 状态字母(git 惯例)。"修改"是常态,不标 —— 只让例外跳出来。 */
const STATUS_MARK: Partial<Record<FileChange["status"], string>> = {
  created: "A",
  deleted: "D",
  renamed: "R",
};

export function FileChangeList({ changes }: { changes: FileChange[] }) {
  // 手风琴:同时只展开一个文件。review 是一份一份看的,开着上一份
  // 去点下一份,旧 diff 只会把新 diff 顶出视野,还得手动收。
  const [expanded, setExpanded] = useState<string | null>(null);
  /** 刚复制过路径的那一行。给"复制路径"按钮一个"已复制"的确认拍。 */
  const [copied, setCopied] = useState<string | null>(null);
  const copiedTimer = useRef<number | undefined>(undefined);
  /** 改动路径是相对项目根的，预览要拼成绝对的。 */
  const root = useContext(ProjectRootContext);

  const copyPath = (path: string) => {
    void navigator.clipboard.writeText(path).then(() => {
      setCopied(path);
      window.clearTimeout(copiedTimer.current);
      copiedTimer.current = window.setTimeout(() => setCopied(null), 1500);
    });
  };

  const toggle = (path: string) => setExpanded((cur) => (cur === path ? null : path));

  return (
    <>
      {changes.map((c) => {
        const open = expanded === c.path;
        const mark = STATUS_MARK[c.status];
        const { dir, name } = splitDisplayPath(c.path);
        return (
          <div className="change" key={c.path}>
            <button
              className="change-head"
              onClick={() => toggle(c.path)}
              type="button"
              aria-expanded={open}
              title={fullTitle(c)}
            >
              <FileIcon path={c.path} />
              <ChangePath dir={dir} name={name} deleted={c.status === "deleted"} />
              <span className="change-stat">
                <span className="add">+{c.added}</span>{" "}
                <span className="del">−{c.removed}</span>
              </span>
              {mark ? (
                <span className={`change-mark ${c.status}`} title={STATUS_LABEL[c.status]}>
                  {mark}
                </span>
              ) : null}
              <span className="change-head-grow" />
              {/* 操作钮平时藏着：扫列表时只要图标、路径、行数。
                  预览、复制在前，展开在最后 —— 点整行也能展开，箭头只是明示。 */}
              {c.status !== "deleted" ? (
                <span
                  className="change-copy"
                  role="button"
                  tabIndex={0}
                  title="预览文件"
                  onClick={(e) => {
                    e.stopPropagation();
                    openFilePreview(looksAbsPath(c.path) ? c.path : joinRoot(root, c.path));
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      e.stopPropagation();
                      openFilePreview(looksAbsPath(c.path) ? c.path : joinRoot(root, c.path));
                    }
                  }}
                >
                  <EyeIcon />
                </span>
              ) : null}
              <span
                className={copied === c.path ? "change-copy done" : "change-copy"}
                role="button"
                tabIndex={0}
                title="复制完整路径"
                onClick={(e) => {
                  e.stopPropagation();
                  copyPath(c.path);
                }}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    e.stopPropagation();
                    copyPath(c.path);
                  }
                }}
              >
                {copied === c.path ? <CheckIcon /> : <CopyIcon />}
              </span>
              <Chevron open={open} />
            </button>

            {open ? <ChangeDetail c={c} /> : null}
          </div>
        );
      })}
    </>
  );
}

function ChangeDetail({ c }: { c: FileChange }) {
  // 没有逐行差异的三种情况要说清是哪种 —— 全都显示成空白的话,
  // 用户分不清"没改内容"和"比不出来"。
  if (c.binary) {
    return <div className="change-note">二进制文件，没有逐行差异。</div>;
  }
  if (c.hunks.length === 0) {
    if (c.status === "renamed") {
      return (
        <div className="change-note">
          文件已重命名，内容未变更。
          {c.renamedFrom ? (
            <span className="change-note-from">旧路径：{c.renamedFrom}</span>
          ) : null}
        </div>
      );
    }
    return (
      <div className="change-note">
        {c.truncated ? "这个文件没能比出逐行差异，请直接看文件本身。" : "内容没有变化。"}
      </div>
    );
  }
  return (
    <div className="change-diff">
      {c.status === "renamed" && c.renamedFrom ? (
        <div className="change-note">旧路径：{c.renamedFrom}</div>
      ) : null}
      {c.hunks.map((h, i) => (
        <div className="hunk" key={i}>
          <div className="hunk-head">{h.header}</div>
          {h.lines.map((l, j) => (
            <div className={`hunk-line ${l.kind}`} key={j}>
              <span className="hunk-sign" aria-hidden>
                {l.kind === "add" ? "+" : l.kind === "del" ? "−" : " "}
              </span>
              <span className="hunk-text">{l.text || "\u00a0"}</span>
            </div>
          ))}
        </div>
      ))}
      {c.truncated ? (
        <div className="hunk-more">改动太大，只显示了前面一截。完整内容请看文件本身。</div>
      ) : null}
    </div>
  );
}

/** 悬停提示:状态 + 全路径;重命名带上旧路径,一眼看全"从哪来到哪去"。 */
function fullTitle(c: FileChange): string {
  if (c.status === "renamed" && c.renamedFrom) {
    return `${STATUS_LABEL[c.status]}：${c.renamedFrom} → ${c.path}`;
  }
  return `${STATUS_LABEL[c.status]}：${c.path}`;
}

/**
 * 行宽稳定多久之后才重新判断目录显不显示。
 *
 * `[约束]` 不能跟着 ResizeObserver 每帧算。判断必须先摘掉 `.tight`
 * 再 `offsetWidth` 强制一次同步布局才量得到可用宽度 —— `.change-path`
 * 是 `flex: 0 1 auto` 的内容宽盒子，收窄之后量到的是文件名宽度而不是
 * 行里还剩多少地方，不摘就再也长不回来。一次改动列表几十行，每行每帧
 * 一次强制布局，侧栏开合动画和拖分隔线全被拖住。
 *
 * 行宽只在连续变形（开合动画、拖分隔线、拉窗口）时变，而目录显不显示
 * 是纯装饰 —— 停下来之后再定，没人看得出晚了这一拍。
 */
const REFIT_QUIET_MS = 80;

/**
 * 够宽就目录+文件名；行被挤窄时整段目录拿掉，只留文件名。
 * 半截 `src/fea…` 既认不出目录、又占着文件名的位置，不如干脆不画。
 */
function ChangePath({
  dir,
  name,
  deleted,
}: {
  dir: string;
  name: string;
  deleted: boolean;
}) {
  const ref = useRef<HTMLSpanElement>(null);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || !dir) return;
    const row = el.closest(".change-head");
    const fit = () => {
      el.classList.remove("tight");
      void el.offsetWidth;
      const dirEl = el.querySelector(".change-dir");
      const nameEl = el.querySelector(".change-name");
      const need = (dirEl?.scrollWidth ?? 0) + (nameEl?.scrollWidth ?? 0);
      if (need > el.clientWidth + 1) el.classList.add("tight");
    };
    let timer = 0;
    const ro = new ResizeObserver(() => {
      window.clearTimeout(timer);
      timer = window.setTimeout(fit, REFIT_QUIET_MS);
    });
    ro.observe(row ?? el);
    // 首次同步定一次：挂载那一拍走防抖的话，目录会先出现再被抽掉。
    fit();
    return () => {
      window.clearTimeout(timer);
      ro.disconnect();
    };
  }, [dir, name]);

  return (
    <span ref={ref} className={deleted ? "change-path deleted" : "change-path"}>
      {dir ? <span className="change-dir">{dir}</span> : null}
      <span className="change-name">{name}</span>
    </span>
  );
}

function CopyIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden>
      <rect x="5.5" y="5.5" width="8" height="8" rx="1.4" stroke="currentColor" strokeWidth="1.3" />
      <path
        d="M10.5 5.5V4.2A1.7 1.7 0 0 0 8.8 2.5H4.2A1.7 1.7 0 0 0 2.5 4.2v4.6A1.7 1.7 0 0 0 4.2 10.5H5.5"
        stroke="currentColor"
        strokeWidth="1.3"
      />
    </svg>
  );
}

function CheckIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M3.5 8.2L6.6 11.2 12.5 4.8"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function EyeIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M1.8 8s2.3-4.2 6.2-4.2S14.2 8 14.2 8s-2.3 4.2-6.2 4.2S1.8 8 1.8 8z"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinejoin="round"
      />
      <circle cx="8" cy="8" r="1.9" stroke="currentColor" strokeWidth="1.3" />
    </svg>
  );
}

function splitDisplayPath(path: string): { dir: string; name: string } {
  const slash = path.lastIndexOf("/");
  if (slash < 0) return { dir: "", name: path };
  return { dir: path.slice(0, slash + 1), name: path.slice(slash + 1) };
}
