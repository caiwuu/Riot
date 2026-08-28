import { useEffect, useState } from "react";

import {
  type ConfigStatus,
  type HookInfo,
  hooksList,
  revealInFinder,
} from "../../bridge";
import { Card, CardBlock, Group, Row } from "./layout";

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
    <>
      <Group
        title="怎么写"
        desc={
          <>
            脚本从 stdin 收一行事件 JSON：<b>exit 2 拦下</b>这次调用（stderr 给模型看），exit 0
            的 stdout 作为补充上下文。事件有 PreToolUse / PostToolUse / Stop / UserPromptSubmit，
            matcher 支持工具名、<code>A|B</code> 和正则。
          </>
        }
      >
        <Card>
          <Row title="全局 hooks" desc={<code>{configDir}/hooks.json</code>}>
            <button onClick={() => void revealInFinder(configDir)}>打开目录</button>
          </Row>
          {activeRoot ? (
            <Row
              title="项目 hooks"
              desc={
                <>
                  <code>{activeRoot}/.riot/hooks.json</code>，和全局叠加。
                </>
              }
            >
              <button onClick={() => void revealInFinder(activeRoot)}>打开项目</button>
            </Row>
          ) : null}
        </Card>
      </Group>

      <Group
        title="已注册的 hooks"
        action={
          <button className="btn-compact" onClick={() => void refresh()}>
            刷新
          </button>
        }
      >
        {loadError ? (
          <div className="empty-state">
            <p className="form-error" style={{ margin: 0 }}>
              读取失败：{loadError}
            </p>
            <button onClick={() => void refresh()}>重试</button>
          </div>
        ) : hooks === null ? (
          <Card>
            <CardBlock>
              <p className="hint" style={{ margin: 0 }}>
                读取中…
              </p>
            </CardBlock>
          </Card>
        ) : hooks.length === 0 ? (
          <div className="empty-state">
            <p className="empty-title">还没有 hooks</p>
            <p className="hint">在上面的 hooks.json 里这样写：</p>
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
              <li
                key={`${h.event}-${h.command}-${i}`}
                className={h.error ? "skill-item bad" : "skill-item"}
              >
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
      </Group>
    </>
  );
}
