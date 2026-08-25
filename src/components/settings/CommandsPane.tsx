import { useEffect, useState } from "react";

import {
  type ConfigStatus,
  type SlashCommand,
  revealInFinder,
  slashCommands,
} from "../../bridge";
import { HintTip } from "../HintTip";

/** 技能在命令页的层级前缀。和 Skills 页用同一套词。 */
const SKILL_TIER: Record<string, string> = {
  builtin: "内置",
  pack: "能力包",
  global: "全局",
  project: "项目",
};

export function CommandsPane({ status, activeRoot }: { status: ConfigStatus; activeRoot: string | null }) {
  const [commands, setCommands] = useState<SlashCommand[] | null>(null);
  const [loadError, setLoadError] = useState("");
  const configDir = status.configPath.replace(/\/[^/]*$/, "");

  const refresh = async () => {
    setLoadError("");
    try {
      setCommands(await slashCommands(activeRoot));
    } catch (e) {
      setCommands(null);
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
          斜杠命令
          <HintTip>
            输入框敲 <code>/</code> 调用。<code>$ARGUMENTS</code> 换整段参数，
            <code>$1 $2</code> 换第 N 个。子目录是命名空间（<code>git/pr.md</code> →{" "}
            <code>/git:pr</code>）。
            <strong>模型看不到命令</strong>，只有你敲了才展开，不占上下文；技能的描述每轮都在，模型能自己决定用。优先级{" "}
            <strong>内置命令 &gt; 命令文件 &gt; 技能</strong>。
            {activeRoot ? (
              <>
                {" "}
                项目级放 <code>{activeRoot}/.riot/commands/</code>。
              </>
            ) : null}
          </HintTip>
        </h2>
        <button className="ghost" onClick={() => void refresh()}>
          刷新
        </button>
      </div>
      <div className="about-row">
        <code>{configDir}/commands/&lt;名字&gt;.md</code>
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
      ) : commands === null ? (
        <p className="hint">读取中…</p>
      ) : commands.length === 0 ? (
        <div className="empty-state">
          <p className="empty-title">还没有命令</p>
          <p className="hint">
            在上面的目录里放一个 <code>.md</code> 文件就有了：文件名是命令名，正文是敲
            <code>/</code> 后发出去的提示词。
          </p>
          <pre className="skill-example">{`# review.md
帮我审查当前改动，重点看：$ARGUMENTS`}</pre>
        </div>
      ) : (
        <ul className="skill-list">
          {commands.map((c) => (
            <li key={c.name} className="skill-item">
              <div className="skill-item-head">
                <span className="skill-name">/{c.name}</span>
                {c.argumentHint ? <code className="hook-matcher">{c.argumentHint}</code> : null}
                {/* 技能带上自己的层级 —— 只写「技能」的话，同一个
                    extend-riot 在 Skills 页是「内置」、这里是「技能」，
                    同一个东西两套说法。 */}
                <span className="skill-source">
                  {c.source === "builtin"
                    ? "内置"
                    : c.source === "skill"
                      ? `${SKILL_TIER[c.skillSource ?? ""] ?? ""}技能`
                      : c.source === "project"
                        ? "项目"
                        : "全局"}
                </span>
              </div>
              <p className="hint" style={{ margin: "2px 0 0" }}>
                {c.description}
              </p>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
