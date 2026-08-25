import { useCallback, useRef, useState } from "react";

import {
  type ConfigStatus,
  type UpdateInfo,
  revealInFinder,
} from "../bridge";
import { ConfirmDialog, type ConfirmRequest } from "./ConfirmDialog";
import { Modal } from "./Modal";
import { AboutPane } from "./settings/AboutPane";
import { CommandsPane } from "./settings/CommandsPane";
import { HooksPane } from "./settings/HooksPane";
import { McpPane } from "./settings/McpPane";
import { PacksPane } from "./settings/PacksPane";
import { PermissionPane } from "./settings/PermissionPane";
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
  | "permission"
  | "mcp"
  | "packs"
  | "skills"
  | "commands"
  | "hooks"
  | "about";

const TABS: { id: Tab; label: string }[] = [
  { id: "provider", label: "Provider" },
  { id: "web", label: "联网" },
  { id: "permission", label: "权限" },
  { id: "mcp", label: "MCP" },
  { id: "packs", label: "能力包" },
  { id: "skills", label: "Skills" },
  { id: "commands", label: "命令" },
  { id: "hooks", label: "Hooks" },
  { id: "about", label: "关于" },
];

/**
 * 设置弹层，左侧分区导航。各分区的正文在 `settings/` 下一区一文件，
 * 这里只管三件事：标签切换、离开拦截（见 [`LeaveGuard`]）、保存回执。
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
    // 关窗前把焦点从输入框上拿走，让"失焦提交"的字段先落地 ——
    // 不做的话，正在编辑的 baseUrl/系统提示词随组件卸载无声蒸发。
    (document.activeElement as HTMLElement | null)?.blur?.();
    guarded(onClose);
  }, [guarded, onClose]);

  return (
    <>
      <Modal className="settings" label="设置" onClose={requestClose}>
          <div className="settings-head">
            <span className="settings-head-title">设置</span>
            <button className="settings-close" onClick={requestClose} title="关闭 (Esc)" aria-label="关闭">
              <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
                <path d="M4 4l8 8M12 4l-8 8" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
              </svg>
            </button>
          </div>

          <div className="settings-main">
            <div className="settings-nav" role="tablist" aria-label="设置分区">
              {TABS.map((t) => (
                <button
                  key={t.id}
                  role="tab"
                  aria-selected={tab === t.id}
                  className={tab === t.id ? "settings-tab active" : "settings-tab"}
                  onClick={() => guarded(() => setTab(t.id))}
                >
                  {t.label}
                </button>
              ))}
            </div>

            <div className="settings-body">
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
          {/* 低调的保存回执：各 Pane 的失焦提交原本全程静默，成功与否只能猜。 */}
          {savedTick > 0 ? (
            <span key={savedTick} className="save-flash" role="status">
              已保存 ✓
            </span>
          ) : null}
      </Modal>
      {confirm ? <ConfirmDialog c={confirm} onClose={() => setConfirm(null)} /> : null}
    </>
  );
}
