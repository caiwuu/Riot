//! 文件改动列表:会话改动条和 Git 抽屉共用的渲染。
//!
//! 两个视图的数据形状相同(FileChange),差别只在来源和空态文案 ——
//! 逐文件的行、hunk、复制路径这些交互只写一份,两边不会慢慢长歪。
//!
//! 行是无边框的紧凑列表(Cursor 同款):图标认文件类型、颜色认状态,
//! 一屏能扫二十个文件。每行一个框的话,十个文件就把面板占满了。

import { useLayoutEffect, useRef, useState } from "react";

import type { FileChange } from "../bridge";
import { SETI_BY_EXT, SETI_BY_NAME, SETI_DEFAULT, type SetiIcon } from "../lib/fileIcons";
import { Chevron } from "./Chevron";

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
                  复制在前、展开在最后 —— 点整行也能展开，箭头只是明示。 */}
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

/* ── 文件类型图标 ─────────────────────────────
   用的是 Cursor / VS Code 内置的 seti 图标字体(拷进 assets/fonts,
   映射生成在 lib/fileIcons.ts)—— 用户在编辑器里天天看的就是这套,
   不用重新学一遍"哪个颜色是哪种文件"。 */

/** 文件名 → 图标。先按完整文件名(package.json、dockerfile…),
 *  再按后缀从长到短(x.blade.php 先试 blade.php 再试 php)。 */
function iconFor(path: string): SetiIcon {
  const name = path.slice(path.lastIndexOf("/") + 1).toLowerCase();
  const byName = SETI_BY_NAME[name];
  if (byName) return byName;
  const parts = name.split(".");
  for (let i = 1; i < parts.length; i++) {
    const icon = SETI_BY_EXT[parts.slice(i).join(".")];
    if (icon) return icon;
  }
  return SETI_DEFAULT;
}

function FileIcon({ path }: { path: string }) {
  const icon = iconFor(path);
  return (
    <span className="file-icon" style={{ color: icon.color }} aria-hidden>
      {icon.ch}
    </span>
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
    const ro = new ResizeObserver(fit);
    ro.observe(row ?? el);
    fit();
    return () => ro.disconnect();
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

function splitDisplayPath(path: string): { dir: string; name: string } {
  const slash = path.lastIndexOf("/");
  if (slash < 0) return { dir: "", name: path };
  return { dir: path.slice(0, slash + 1), name: path.slice(slash + 1) };
}
