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
  RemoteIcon,
  ShieldIcon,
  SkillIcon,
  SlidersIcon,
  TerminalIcon,
} from "./settings/navIcons";
import { type MessageKey, useT } from "../i18n";
import { AboutPane } from "./settings/AboutPane";
import { CommandsPane } from "./settings/CommandsPane";
import { GeneralPane } from "./settings/GeneralPane";
import { HooksPane } from "./settings/HooksPane";
import { McpPane } from "./settings/McpPane";
import { PacksPane } from "./settings/PacksPane";
import { PermissionPane } from "./settings/PermissionPane";
import { PromptsPane } from "./settings/PromptsPane";
import { ProviderPane } from "./settings/ProviderPane";
import { RemotePane } from "./settings/RemotePane";
import { SkillsPane } from "./settings/SkillsPane";
import { WebPane } from "./settings/WebPane";
import type { LeaveGuard } from "./settings/shared";

interface Props {
  status: ConfigStatus;
  onStatus: (s: ConfigStatus) => void;
  onClose: () => void;
  /** 当前会话的项目根。Skills 页用它列项目级技能；没有活跃会话时为 null。 */
  activeRoot?: string | null;
  /** 跟主界面侧栏同一宽度，打开设置时左列不用跳一截。 */
  navWidth: number;
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
  | "general"
  | "remote"
  | "about";

interface TabDef {
  id: Tab;
  icon: () => ReactElement;
}

/**
 * 导航分区，按"改它会影响什么"分组。
 *
 * 十项平铺时每次切页都得从头读一遍标签；分成四组之后，找一项先定位组、
 * 再在两三项里挑，扫视快得多。
 *
 * 标签、页头标题和说明都在词典里，键由 id 派生：`settings.tab.<id>` 是
 * 导航标签，`.title` / `.desc` 是分区页头 —— 标签要短，标题可以说全。
 */
const NAV: { group: MessageKey; tabs: TabDef[] }[] = [
  {
    group: "settings.group.model",
    tabs: [
      { id: "provider", icon: ProviderIcon },
      { id: "web", icon: GlobeIcon },
      { id: "prompts", icon: BookmarkIcon },
    ],
  },
  {
    group: "settings.group.run",
    tabs: [{ id: "permission", icon: ShieldIcon }],
  },
  {
    group: "settings.group.ext",
    tabs: [
      { id: "mcp", icon: PlugIcon },
      { id: "packs", icon: PackageIcon },
      { id: "skills", icon: SkillIcon },
      { id: "commands", icon: TerminalIcon },
      { id: "hooks", icon: HookIcon },
    ],
  },
  {
    group: "settings.group.app",
    tabs: [
      { id: "general", icon: SlidersIcon },
      { id: "remote", icon: RemoteIcon },
      { id: "about", icon: InfoIcon },
    ],
  },
];

const tabLabel = (id: Tab): MessageKey => `settings.tab.${id}`;
const tabTitle = (id: Tab): MessageKey => `settings.tab.${id}.title`;
const tabDesc = (id: Tab): MessageKey => `settings.tab.${id}.desc`;

/**
 * 设置整页。盖住主界面，不卸会话和终端 —— 回来还在。
 * 布局跟主界面同一套左右分栏：左列通顶（返回在侧栏顶上），右列是正文。
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
  navWidth,
}: Props) {
  const { t } = useT();
  const [tab, setTab] = useState<Tab>("provider");
  const [confirm, setConfirm] = useState<ConfirmRequest | null>(null);

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
      <div className="settings-page" role="dialog" aria-modal="true" aria-label={t("settings.title")}>
        <aside className="settings-side" style={{ width: navWidth }}>
          <div
            className={IS_MAC ? "settings-head pad-lights" : "settings-head"}
            data-tauri-drag-region
          >
            <button className="settings-back" onClick={requestClose} title={t("settings.backTitle")}>
              <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
                <path
                  d="M10 3L5 8l5 5"
                  stroke="currentColor"
                  strokeWidth="1.6"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                />
              </svg>
              {t("settings.back")}
            </button>
          </div>
          <nav className="settings-nav" role="tablist" aria-label={t("settings.navLabel")}>
            {NAV.map((g) => (
              <div className="settings-nav-group" key={g.group}>
                <span className="settings-nav-label">{t(g.group)}</span>
                {g.tabs.map((def) => (
                  <button
                    key={def.id}
                    role="tab"
                    aria-selected={tab === def.id}
                    className={tab === def.id ? "settings-tab active" : "settings-tab"}
                    onClick={() => guarded(() => setTab(def.id))}
                  >
                    <span className="settings-tab-icon">
                      <def.icon />
                    </span>
                    {t(tabLabel(def.id))}
                  </button>
                ))}
              </div>
            ))}
          </nav>
        </aside>

        <div className="settings-body">
          <div className="settings-body-top" data-tauri-drag-region />
          <div className="settings-scroll">
            <div className="settings-inner">
              <PaneHead title={t(tabTitle(tab))} desc={t(tabDesc(tab))} />
              {/* 配置读不懂被回落成默认值时，用户看到的是"我配的东西全没了"。
                  不在这儿说一句，他不会知道旁边躺着一份完好的备份。放在
                  标签页外面：无论他点开哪一页都得看见。 */}
              {status.configBackup ? (
                <div className="recovered-note">
                  <p className="empty-title">{t("settings.configBroken.title")}</p>
                  <p className="hint">{t("settings.configBroken.hint")}</p>
                  <code className="path">{status.configBackup}</code>
                  <button onClick={() => void revealInFinder(status.configBackup ?? "")}>
                    {t("common.revealInFinder")}
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
              {tab === "general" ? <GeneralPane /> : null}
              {tab === "remote" ? (
                <RemotePane
                  status={status}
                  onStatus={onStatus}
                  onSaved={flashSaved}
                  askConfirm={setConfirm}
                />
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
            {t("common.saved")}
          </span>
        ) : null}
      </div>
      {confirm ? <ConfirmDialog c={confirm} onClose={() => setConfirm(null)} /> : null}
    </>
  );
}
