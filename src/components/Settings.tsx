import { type ReactElement, useCallback, useRef, useState } from "react";

import {
  type ConfigStatus,
  type UpdateInfo,
  revealInFinder,
} from "../bridge";
import { ConfirmDialog, type ConfirmRequest } from "./ConfirmDialog";
import { IS_MAC } from "./chrome";
import { useEscLayer } from "./Modal";
import { PaneHead } from "./settings/layout";
import {
  BookmarkIcon,
  GlobeIcon,
  HookIcon,
  InfoIcon,
  PackageIcon,
  PlugIcon,
  ProviderIcon,
  ShieldIcon,
  SkillIcon,
  TerminalIcon,
} from "./settings/navIcons";
import { AboutPane } from "./settings/AboutPane";
import { CommandsPane } from "./settings/CommandsPane";
import { HooksPane } from "./settings/HooksPane";
import { McpPane } from "./settings/McpPane";
import { PacksPane } from "./settings/PacksPane";
import { PermissionPane } from "./settings/PermissionPane";
import { PromptsPane } from "./settings/PromptsPane";
import { ProviderPane } from "./settings/ProviderPane";
import { SkillsPane } from "./settings/SkillsPane";
import { WebPane } from "./settings/WebPane";
import type { LeaveGuard } from "./settings/shared";

interface Props {
  status: ConfigStatus;
  onStatus: (s: ConfigStatus) => void;
  onClose: () => void;
  /** 当前会话的项目根。Skills 页用它列项目级技能；没有活跃会话时为 null。 */
  activeRoot?: string | null;
  appVersion: string;
  update: UpdateInfo | null;
  updateChecking: boolean;
  updateError: string | null;
  onCheckUpdate: () => void;
}

type Tab =
  | "provider"
  | "web"
  | "prompts"
  | "permission"
  | "mcp"
  | "packs"
  | "skills"
  | "commands"
  | "hooks"
  | "about";

interface TabDef {
  id: Tab;
  label: string;
  icon: () => ReactElement;
  /** 分区页头。标题和导航标签不必同字 —— 标签要短，标题可以说全。 */
  title: string;
  desc: string;
}

/**
 * 导航分区，按"改它会影响什么"分组。
 *
 * 十项平铺时每次切页都得从头读一遍标签；分成四组之后，找一项先定位组、
 * 再在两三项里挑，扫视快得多。
 */
const NAV: { group: string; tabs: TabDef[] }[] = [
  {
    group: "模型",
    tabs: [
      {
        id: "provider",
        label: "服务方",
        icon: ProviderIcon,
        title: "服务方",
        desc: "接入模型服务、保存 API key，并为每个模型设采样参数。",
      },
      {
        id: "web",
        label: "联网",
        icon: GlobeIcon,
        title: "联网",
        desc: "模型能不能抓网页、能不能搜索，以及正文用什么模型压缩。",
      },
      {
        id: "prompts",
        label: "提示词",
        icon: BookmarkIcon,
        title: "提示词",
        desc: "收藏常用的系统提示词，开会话时挑一条填进去，不用每次重打。",
      },
    ],
  },
  {
    group: "运行",
    tabs: [
      {
        id: "permission",
        label: "权限",
        icon: ShieldIcon,
        title: "权限与运行",
        desc: "新会话的默认权限、命令隔离，以及授权超时和单轮上限。",
      },
    ],
  },
  {
    group: "扩展",
    tabs: [
      {
        id: "mcp",
        label: "MCP",
        icon: PlugIcon,
        title: "MCP 服务器",
        desc: "接入外部工具服务器，连上之后模型直接就能调用。",
      },
      {
        id: "packs",
        label: "能力包",
        icon: PackageIcon,
        title: "能力包",
        desc: "可选下载的运行时。装上之后相关工具和技能自动注册。",
      },
      {
        id: "skills",
        label: "Skills",
        icon: SkillIcon,
        title: "Skills",
        desc: "写成 SKILL.md 的技能，模型按需加载。",
      },
      {
        id: "commands",
        label: "命令",
        icon: TerminalIcon,
        title: "斜杠命令",
        desc: "输入框敲 / 调用的提示词模板。模型看不到它们，不占上下文。",
      },
      {
        id: "hooks",
        label: "Hooks",
        icon: HookIcon,
        title: "Hooks",
        desc: "在固定检查点自动跑脚本，可以拦下工具调用或整轮回复。",
      },
    ],
  },
  {
    group: "应用",
    tabs: [
      {
        id: "about",
        label: "关于",
        icon: InfoIcon,
        title: "关于 Riot",
        desc: "版本、更新和配置文件位置。",
      },
    ],
  },
];

const ALL_TABS: TabDef[] = NAV.flatMap((g) => g.tabs);

/**
 * 设置整页。盖住主界面，不卸会话和终端 —— 回来还在。
 * 各分区的正文在 `settings/` 下一区一文件，这里管标签、离开拦截、保存回执。
 *
 * 所有修改都提交整个 [`AppConfig`] —— 宿主在保存前 resolve 一次，
 * 把"active 指向不存在的 provider"这类坏状态挡在写盘之前。
 */
export function Settings({
  status,
  onStatus,
  onClose,
  activeRoot,
  appVersion,
  update,
  updateChecking,
  updateError,
  onCheckUpdate,
}: Props) {
  const [tab, setTab] = useState<Tab>("provider");
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);
  const current = ALL_TABS.find((t) => t.id === tab);

  /** 「已保存 ✓」瞬时提示。计数器当 key：连续保存也能重启淡出动画。 */
  const [savedTick, setSavedTick] = useState(0);
  const flashSaved = useCallback(() => setSavedTick((t) => t + 1), []);

  /** 当前分区注册的离开拦截。ref 而不是 state：它只在离开的瞬间被读一次。 */
  const leaveGuard = useRef<LeaveGuard | null>(null);
  const registerLeaveGuard = useCallback((g: LeaveGuard | null) => {
    leaveGuard.current = g;
  }, []);

  /** 关闭 / 切分区都从这儿走：有未保存的内容就先问，没有就直接做。 */
  const guarded = useCallback(
    (proceed: () => void) => {
      const ask = leaveGuard.current?.();
      if (ask) {
        setConfirm({
          ...ask,
          action: () => {
            leaveGuard.current = null;
            proceed();
          },
        });
      } else {
        proceed();
      }
    },
    [],
  );
  const requestClose = useCallback(() => {
    // 离开前把焦点从输入框上拿走，让"失焦提交"的字段先落地 ——
    // 不做的话，正在编辑的 baseUrl 随组件卸载无声蒸发。
    (document.activeElement as HTMLElement | null)?.blur?.();
    guarded(onClose);
  }, [guarded, onClose]);
  useEscLayer(requestClose);

  return (
    <>
      <div className="settings-page" role="dialog" aria-modal="true" aria-label="设置">
        <div
          className={IS_MAC ? "settings-head pad-traffic" : "settings-head"}
          data-tauri-drag-region
        >
          <button className="settings-back" onClick={requestClose} title="返回应用 (Esc)">
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
              <path
                d="M10 3L5 8l5 5"
                stroke="currentColor"
                strokeWidth="1.6"
                strokeLinecap="round"
                strokeLinejoin="round"
              />
            </svg>
            返回应用
          </button>
        </div>

        <div className="settings-main">
          <nav className="settings-nav" role="tablist" aria-label="设置分区">
            {NAV.map((g) => (
              <div className="settings-nav-group" key={g.group}>
                <span className="settings-nav-label">{g.group}</span>
                {g.tabs.map((t) => (
                  <button
                    key={t.id}
                    role="tab"
                    aria-selected={tab === t.id}
                    className={tab === t.id ? "settings-tab active" : "settings-tab"}
                    onClick={() => guarded(() => setTab(t.id))}
                  >
                    <span className="settings-tab-icon">
                      <t.icon />
                    </span>
                    {t.label}
                  </button>
                ))}
              </div>
            ))}
          </nav>

          <div className="settings-body">
            <div className="settings-inner">
              {current ? <PaneHead title={current.title} desc={current.desc} /> : null}
              {/* 配置读不懂被回落成默认值时，用户看到的是"我配的东西全没了"。
                  不在这儿说一句，他不会知道旁边躺着一份完好的备份。放在
                  标签页外面：无论他点开哪一页都得看见。 */}
              {status.configBackup ? (
                <div className="recovered-note">
                  <p className="empty-title">配置文件损坏</p>
                  <p className="hint">已用默认设置启动，原文件备份在：</p>
                  <code className="path">{status.configBackup}</code>
                  <button onClick={() => void revealInFinder(status.configBackup ?? "")}>
                    在访达中显示
                  </button>
                </div>
              ) : null}
              {tab === "provider" ? (
                <ProviderPane
                  status={status}
                  onStatus={onStatus}
                  askConfirm={setConfirm}
                  onSaved={flashSaved}
                />
              ) : null}
              {tab === "web" ? (
                <WebPane status={status} onStatus={onStatus} onSaved={flashSaved} />
              ) : null}
              {tab === "prompts" ? (
                <PromptsPane
                  status={status}
                  onStatus={onStatus}
                  askConfirm={setConfirm}
                  onSaved={flashSaved}
                />
              ) : null}
              {tab === "permission" ? (
                <PermissionPane
                  status={status}
                  onStatus={onStatus}
                  askConfirm={setConfirm}
                  onSaved={flashSaved}
                />
              ) : null}
              {tab === "mcp" ? (
                <McpPane
                  status={status}
                  onStatus={onStatus}
                  askConfirm={setConfirm}
                  registerLeaveGuard={registerLeaveGuard}
                  onSaved={flashSaved}
                />
              ) : null}
              {tab === "packs" ? <PacksPane askConfirm={setConfirm} /> : null}
              {tab === "skills" ? (
                <SkillsPane status={status} activeRoot={activeRoot ?? null} />
              ) : null}
              {tab === "commands" ? (
                <CommandsPane status={status} activeRoot={activeRoot ?? null} />
              ) : null}
              {tab === "hooks" ? (
                <HooksPane status={status} activeRoot={activeRoot ?? null} />
              ) : null}
              {tab === "about" ? (
                <AboutPane
                  status={status}
                  version={appVersion}
                  update={update}
                  checking={updateChecking}
                  error={updateError}
                  onCheck={onCheckUpdate}
                />
              ) : null}
            </div>
          </div>
        </div>
        {/* 低调的保存回执：各 Pane 的失焦提交原本全程静默，成功与否只能猜。 */}
        {savedTick > 0 ? (
          <span key={savedTick} className="save-flash" role="status">
            已保存 ✓
          </span>
        ) : null}
      </div>
      {confirm ? <ConfirmDialog c={confirm} onClose={() => setConfirm(null)} /> : null}
    </>
  );
}
