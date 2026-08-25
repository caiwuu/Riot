import { useEffect, useState } from "react";

import {
  type ConfigStatus,
  type HookInfo,
  hooksList,
  revealInFinder,
} from "../../bridge";
import { HintTip } from "../HintTip";

const HOOK_EVENT_HINT: Record<string, string> = {
  PreToolUse: "工具执行前。exit 2 = 拦下这次调用",
  PostToolUse: "工具执行后。反馈给模型（格式检查、lint）",
  Stop: "模型想收尾时。exit 2 = 不许停，带理由再跑一轮",
  UserPromptSubmit: "消息发出前。exit 2 = 拦下这条消息",
};

export function HooksPane({ status, activeRoot }: { status: ConfigStatus; activeRoot: string | null }) {
  const [hooks, setHooks] = useState<HookInfo[] | null>(null);
  const [loadError, setLoadError] = useState("");
  const configDir = status.configPath.replace(/\/[^/]*$/, "");

  const refresh = async () => {
    setLoadError("");
    try {
      setHooks(await hooksList(activeRoot));
    } catch (e) {
      setHooks(null);
      setLoadError(String(e));
    }
  };
  useEffect(() => {
    void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeRoot]);

  return (
    <section>
      <div className="skills-head">
        <h2>
          Hooks
          <HintTip>
            固定检查点跑脚本：stdin 收一行事件 JSON，<b>exit 2 拦下</b>（stderr
            给模型看），exit 0 的 stdout 作补充上下文。事件：PreToolUse / PostToolUse / Stop /
            UserPromptSubmit。matcher 支持工具名、<code>A|B</code>、正则。
            {activeRoot ? (
              <>
                {" "}
                项目级 <code>{activeRoot}/.riot/hooks.json</code> 和全局叠加。
              </>
            ) : null}
          </HintTip>
        </h2>
        <button className="ghost" onClick={() => void refresh()}>
          刷新
        </button>
      </div>
      <div className="about-row">
        <code>{configDir}/hooks.json</code>
        <button className="ghost" onClick={() => void revealInFinder(configDir)}>
          打开目录
        </button>
      </div>
      {loadError ? (
        <div className="empty-state">
          <p className="form-error" style={{ margin: 0 }}>
            读取失败：{loadError}
          </p>
          <button onClick={() => void refresh()}>重试</button>
        </div>
      ) : hooks === null ? (
        <p className="hint">读取中…</p>
      ) : hooks.length === 0 ? (
        <div className="empty-state">
          <p className="empty-title">还没有 hooks</p>
          <pre className="skill-example">{`{
  "PreToolUse": [
    { "matcher": "Bash",
      "hooks": [{ "type": "command", "command": "./scripts/check-cmd.sh" }] }
  ],
  "Stop": [
    { "hooks": [{ "type": "command", "command": "cargo test -q" }] }
  ]
}`}</pre>
        </div>
      ) : (
        <ul className="skill-list">
          {hooks.map((h, i) => (
            <li key={`${h.event}-${h.command}-${i}`} className={h.error ? "skill-item bad" : "skill-item"}>
              <div className="skill-item-head">
                <span className="skill-name">{h.error ? "配置有问题" : h.event}</span>
                {h.matcher ? <code className="hook-matcher">{h.matcher}</code> : null}
                <span className="skill-source">{h.source === "project" ? "项目" : "全局"}</span>
              </div>
              <p className={h.error ? "form-error" : "hint"} style={{ margin: "2px 0 0" }}>
                {h.error ? `${h.command}：${h.error}` : h.command}
              </p>
              {!h.error ? (
                <p className="hint" style={{ margin: "2px 0 0" }}>
                  {HOOK_EVENT_HINT[h.event] ?? ""}（超时 {h.timeoutSecs}s）
                </p>
              ) : null}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
