/**
 * 右侧抽屉的统一标签栏（照 Codex）：浏览器页面、Git 改动、文件预览……
 * 每个功能是一个标签，共存不互斥，点着切换。以后要加新面板，
 * 加一种 kind + 一个渲染分支即可，标签栏本身不用动。
 *
 * 浏览器在这里不是一个标签，而是**一组**：宿主里的每个页面（CEF 标签）
 * 直接展开成顶层标签，和预览文件平级 —— 抽屉里只能有一条标签栏，
 * 浏览器面板再自带一排页面标签就是两排"标签 + 关闭 + 加号"叠着，
 * 分不清谁管谁。页面状态由 App 持有（见 useBrowserPanel）。
 *
 * 这里只有标签条的展示与交互；标签数组、激活项、抽屉开合都归 App 管
 * （工作台跟会话走，每个会话一份，和终端组、浏览器同一个语义）。
 */

import type { PanelState } from "../bridge";
import { basename } from "../pathDisplay";
import { BrowserIcon, DiffIcon, FileDocIcon } from "./icons";

/** 工作台标签。browser / changes 每会话至多一个，preview 每文件一个。
 *  browser 在标签栏上展开成一组页面标签，但在状态里始终是一项 ——
 *  页面的增删是宿主的事，工作台只关心"浏览器开没开"。 */
export type WorkbenchTab =
  | { kind: "browser" }
  | { kind: "changes" }
  | { kind: "preview"; path: string };

/** 工作台的全部状态。收成一个对象是为了跟会话整存整取。 */
export interface WorkbenchState {
  tabs: WorkbenchTab[];
  /** 激活标签的 id。抽屉收起时保留 —— 再展开回到原处。 */
  active: string | null;
  /** 抽屉展开着没有。收起不清标签（和旧版"收起面板标签保留"同款）。 */
  open: boolean;
}

export const EMPTY_WORKBENCH: WorkbenchState = { tabs: [], active: null, open: false };

/** 标签的稳定 id。preview 按路径区分，其余 kind 即 id（单例）。 */
export function tabId(t: WorkbenchTab): string {
  return t.kind === "preview" ? `preview:${t.path}` : t.kind;
}

/** 还没加载出标题（或停在空白页）的页面显示成这个。 */
const PAGE_PLACEHOLDER = "新标签页";

export function WorkbenchTabs({
  tabs,
  active,
  pages,
  onSelect,
  onClose,
  onSelectPage,
  onClosePage,
  onAdd,
  trailing,
}: {
  tabs: WorkbenchTab[];
  active: string | null;
  /** 浏览器的页面状态。没有 browser 标签时内容为空，不参与渲染。 */
  pages: PanelState;
  onSelect: (id: string) => void;
  onClose: (id: string) => void;
  /** 点了某个浏览器页面标签：激活浏览器并切到那一页。 */
  onSelectPage: (pageId: number) => void;
  /** 关某个浏览器页面。最后一页关掉时由 App 收掉整个浏览器标签。 */
  onClosePage: (pageId: number) => void;
  /** "+"按钮。菜单项由 App 提供（全局 ContextMenu），这里只报点击位置。 */
  onAdd: (e: React.MouseEvent) => void;
  /** 右端附加（窗口开关已钉在 .shell 上，这里一般不传）。 */
  trailing?: React.ReactNode;
}) {
  return (
    <div className="wb-tabs">
      {/* 标签单独一层可横滚：整条栏滚的话，右端的窗口开关会被一排标签
          推出视野 —— 而它们是"退出这个状态"的唯一出口。 */}
      <div className="wb-tabs-list">
        {tabs.map((t) => {
          if (t.kind === "browser") {
            // 浏览器启动要一秒左右。这段时间摆一个占位标签，形状从
            // 第一帧就是对的 —— 空着的话标签像是点丢了（原面板同款理由）。
            if (pages.tabs.length === 0) {
              return (
                <StripTab
                  key="browser"
                  active={active === "browser"}
                  icon={<BrowserIcon />}
                  title="浏览器"
                  tooltip="浏览器启动中…"
                  onSelect={() => onSelect("browser")}
                  onClose={() => onClose("browser")}
                />
              );
            }
            return pages.tabs.map((p) => (
              <StripTab
                key={`page:${p.id}`}
                // 页面标签的高亮 = 浏览器在前台 && 正是这一页。
                active={active === "browser" && p.id === pages.active}
                icon={<GlobeIcon />}
                title={p.title || PAGE_PLACEHOLDER}
                tooltip={p.url || PAGE_PLACEHOLDER}
                onSelect={() => onSelectPage(p.id)}
                onClose={() => onClosePage(p.id)}
              />
            ));
          }
          const id = tabId(t);
          return (
            <StripTab
              key={id}
              active={id === active}
              icon={t.kind === "changes" ? <DiffIcon /> : <FileDocIcon />}
              title={t.kind === "changes" ? "Git 改动" : basename(t.path)}
              tooltip={t.kind === "preview" ? t.path : "Git 改动"}
              onSelect={() => onSelect(id)}
              onClose={() => onClose(id)}
            />
          );
        })}
        {/* 一个标签都没有时不给"+"：那时下面的空状态本身就是添加菜单，
            两个入口并排只会让人犹豫点哪个。 */}
        {tabs.length > 0 ? (
          <button className="icon" onClick={onAdd} title="添加面板" aria-label="添加面板">
            <PlusIcon />
          </button>
        ) : null}
      </div>
      {/* 拖这块空白能挪窗口 —— 顶栏的空白处一直是这么用的。 */}
      <span className="wb-tabs-spacer" data-tauri-drag-region />
      {trailing}
    </div>
  );
}

/**
 * 抽屉开着但一个标签都没有时的空状态（Codex 同款）：把能添加的面板
 * 摆成一列大按钮，右端标快捷键。比一句"这里空空如也"有用 —— 空状态
 * 本身就是"添加面板"的菜单。只列能变成侧边标签的东西：终端停靠在
 * 底部、有自己的顶栏按钮，不在这里。
 */
export function WorkbenchEmpty({
  onChanges,
  onBrowser,
  onOpenFile,
}: {
  onChanges: () => void;
  onBrowser: () => void;
  onOpenFile: () => void;
}) {
  return (
    <div className="wb-empty">
      <button type="button" className="wb-empty-item" onClick={onChanges}>
        <span className="wb-empty-icon">
          <DiffIcon />
        </span>
        <span className="wb-empty-label">Git 改动</span>
        <kbd className="wb-empty-kbd">⌘⇧G</kbd>
      </button>
      <button type="button" className="wb-empty-item" onClick={onBrowser}>
        <span className="wb-empty-icon">
          <BrowserIcon />
        </span>
        <span className="wb-empty-label">浏览器</span>
        <kbd className="wb-empty-kbd">⌘T</kbd>
      </button>
      <button type="button" className="wb-empty-item" onClick={onOpenFile}>
        <span className="wb-empty-icon">
          <FileDocIcon />
        </span>
        <span className="wb-empty-label">打开文件…</span>
        <kbd className="wb-empty-kbd">⌘P</kbd>
      </button>
    </div>
  );
}

/** 一枚标签：图标 + 标题 + 悬停出现的关闭键。 */
function StripTab({
  active,
  icon,
  title,
  tooltip,
  onSelect,
  onClose,
}: {
  active: boolean;
  icon: React.ReactNode;
  title: string;
  tooltip: string;
  onSelect: () => void;
  onClose: () => void;
}) {
  const close = (e: React.SyntheticEvent) => {
    // 不冒泡给外层的"切到这个标签" —— 否则关掉的同时又切了过去。
    e.stopPropagation();
    onClose();
  };
  return (
    <button className={active ? "wb-tab active" : "wb-tab"} onClick={onSelect} title={tooltip}>
      <span className="wb-tab-icon">{icon}</span>
      <span className="wb-tab-title">{title}</span>
      {/* 关闭做成 span 而不是嵌套 button：button 套 button 是非法
          HTML，浏览器会把内层拆出去，点击行为不可预料。 */}
      <span
        className="wb-tab-close"
        role="button"
        tabIndex={0}
        aria-label={`关闭 ${title}`}
        onClick={close}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            close(e);
          }
        }}
      >
        <CloseIcon />
      </span>
    </button>
  );
}

function CloseIcon() {
  return (
    <svg width="10" height="10" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M4 4l8 8M12 4l-8 8"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
      />
    </svg>
  );
}

function PlusIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path d="M8 3v10M3 8h10" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
    </svg>
  );
}

/** 页面标签的地球（和原浏览器面板同款）。 */
function GlobeIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 16 16" fill="none" aria-hidden>
      <circle cx="8" cy="8" r="6" stroke="currentColor" strokeWidth="1.3" />
      <path
        d="M2 8h12M8 2c1.8 2 1.8 10 0 12M8 2C6.2 4 6.2 12 8 14"
        stroke="currentColor"
        strokeWidth="1.3"
      />
    </svg>
  );
}

