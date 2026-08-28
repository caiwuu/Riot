import { useEffect, useState } from "react";

import {
  type ConfigStatus,
  type SkillInfo,
  revealInFinder,
  skillsList,
} from "../../bridge";
import { Card, CardBlock, Group, Row } from "./layout";

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
      <Group
        title="怎么写"
        desc={
          <>
            写成 <code>SKILL.md</code>，模型按需加载：名字和描述每轮都在上下文里，正文用到才读，
            所以描述要写「什么时候用」。优先级 <strong>项目 &gt; 全局 &gt; 内置</strong>
            ，同名即可盖掉内置。只想自己敲 <code>/</code> 调用、不想占上下文的，去「命令」页。
          </>
        }
      >
        <Card>
          <Row title="全局技能目录" desc={<code>{globalDir}/&lt;名字&gt;/SKILL.md</code>}>
            <button onClick={() => void revealInFinder(globalDir)}>打开目录</button>
          </Row>
          {activeRoot ? (
            <Row
              title="项目技能目录"
              desc={<code>{activeRoot}/.riot/skills/&lt;名字&gt;/SKILL.md</code>}
            >
              <button onClick={() => void revealInFinder(activeRoot)}>打开项目</button>
            </Row>
          ) : null}
        </Card>
      </Group>

      <Group
        title="已发现的技能"
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
        ) : skills === null ? (
          <Card>
            <CardBlock>
              <p className="hint" style={{ margin: 0 }}>
                读取中…
              </p>
            </CardBlock>
          </Card>
        ) : skills.length === 0 ? (
          <div className="empty-state">
            <p className="empty-title">还没有技能</p>
            <p className="hint">在上面的目录里建一个文件夹，放进这样一个 SKILL.md：</p>
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
      </Group>
    </>
  );
}
