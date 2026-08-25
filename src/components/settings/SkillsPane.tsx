import { useEffect, useState } from "react";

import {
  type ConfigStatus,
  type SkillInfo,
  revealInFinder,
  skillsList,
} from "../../bridge";
import { HintTip } from "../HintTip";

/**
 * 技能清单（只读）。技能就是磁盘上的 SKILL.md，编辑器比表单好用 ——
 * 这页只负责"有哪些、哪个坏了、目录在哪"。
 */
export function SkillsPane({ status, activeRoot }: { status: ConfigStatus; activeRoot: string | null }) {
  const [skills, setSkills] = useState<SkillInfo[] | null>(null);
  const [loadError, setLoadError] = useState("");
  const configDir = status.configPath.replace(/\/[^/]*$/, "");
  const globalDir = `${configDir}/skills`;

  const refresh = async () => {
    setLoadError("");
    try {
      setSkills(await skillsList(activeRoot));
    } catch (e) {
      // 读失败不能装成"还没有技能"：空状态会引导用户去建目录，而不是去修权限
      setSkills(null);
      setLoadError(String(e));
    }
  };
  useEffect(() => {
    void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeRoot]);

  return (
    <>
      <section>
        <h2>
          Skills
          <HintTip>
            写成 <code>SKILL.md</code>，模型按需加载（清单进上下文，正文用到才读）。优先级{" "}
            <strong>项目 &gt; 全局 &gt; 内置</strong>，同名即可盖掉内置。
            名字和描述每轮都在上下文里，所以描述写「什么时候用」。只想自己敲{" "}
            <code>/</code>、不想占上下文的，去「命令」页。
            {activeRoot ? (
              <>
                {" "}
                项目级放 <code>{activeRoot}/.riot/skills/</code>。
              </>
            ) : null}
          </HintTip>
        </h2>
        <div className="about-row">
          <code>{globalDir}/&lt;名字&gt;/SKILL.md</code>
          <button className="ghost" onClick={() => void revealInFinder(globalDir)}>
            打开目录
          </button>
        </div>
      </section>

      <section>
        <div className="skills-head">
          <h2>已发现的技能</h2>
          <button className="ghost" onClick={() => void refresh()}>
            刷新
          </button>
        </div>
        {loadError ? (
          <div className="empty-state">
            <p className="form-error" style={{ margin: 0 }}>
              读取失败：{loadError}
            </p>
            <button onClick={() => void refresh()}>重试</button>
          </div>
        ) : skills === null ? (
          <p className="hint">读取中…</p>
        ) : skills.length === 0 ? (
          <div className="empty-state">
            <p className="empty-title">还没有技能</p>
            <p className="hint">
              示例格式：
            </p>
            <pre className="skill-example">{`---
name: 发布流程
description: 发布新版本时用。跑测试、打 tag、更新 changelog。
---
1. 跑 cargo test --workspace
2. ……`}</pre>
          </div>
        ) : (
          <ul className="skill-list">
            {skills.map((s) => (
              <li
                key={s.path || `builtin-${s.name}`}
                className={s.error ? "skill-item bad" : "skill-item"}
              >
                <div className="skill-item-head">
                  <span className="skill-name">{s.name}</span>
                  <span className="skill-source">
                    {s.source === "builtin"
                      ? "内置"
                      : s.source === "project"
                        ? "项目"
                        : s.source === "pack"
                          ? "能力包"
                          : "全局"}
                  </span>
                </div>
                <p className={s.error ? "form-error" : "hint"} style={{ margin: "2px 0 0" }}>
                  {s.error ?? s.description}
                </p>
              </li>
            ))}
          </ul>
        )}
      </section>
    </>
  );
}
