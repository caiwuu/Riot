//! Skill 的发现与解析。
//!
//! # 目录布局
//!
//! ```text
//! <配置目录>/riot/skills/<名字>/SKILL.md     全局技能，所有项目可用
//! <项目根>/.riot/skills/<名字>/SKILL.md      项目技能，只在该项目的会话里
//! ```
//!
//! 同名时**项目级赢** —— 项目里的约定比全局偏好更具体（和 git 的
//! local > global 同一直觉）。
//!
//! # SKILL.md 格式
//!
//! ```text
//! ---
//! name: 发布流程
//! description: 发布新版本时用。跑测试、打 tag、更新 changelog。
//! ---
//! 正文（Markdown）。可用 $ARGUMENTS 和 ${SKILL_DIR} 占位符。
//! ```
//!
//! frontmatter 只认 `key: value` 单行形式。`description` 必填 ——
//! 它是模型决定"要不要加载"的唯一依据，没有它的技能等于不存在；
//! 缺了不静默跳过，作为"有问题的技能"报给设置页，用户看得见原因。
//!
//! # 为什么每轮扫描而不是缓存
//!
//! 技能是用户随手编辑的 Markdown。缓存的失效时机（文件监听？手动刷新？）
//! 比扫描本身贵得多 —— 目录里就几个小文件，扫一遍是微秒级的事。
//!
//! 豁免理由：宿主层，读的是用户自己的技能目录。

#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use riot_tools::tools::skill::SkillCard;
use serde::Serialize;

/// 单个技能正文的上限。超过的基本是往目录里塞错了东西（数据文件该放
/// 技能目录里让模型按需 Read，不该整个贴进 SKILL.md）。
const MAX_BODY_CHARS: usize = 64 * 1024;

/// 发现结果：能用的进工具，有问题的报给设置页。
#[derive(Default)]
pub struct Discovered {
    pub cards: Vec<SkillCard>,
    pub problems: Vec<Problem>,
}

pub struct Problem {
    pub path: PathBuf,
    pub reason: String,
}

/// 给设置页看的技能清单条目。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    /// SKILL.md 的完整路径。
    pub path: String,
    /// `global` 或 `project`。
    pub source: String,
    /// 解析失败的原因。None = 这个技能可用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 全局技能目录（`<配置目录>/riot/skills`）。
pub fn global_dir() -> PathBuf {
    crate::config::config_path()
        .parent()
        .unwrap_or(Path::new("."))
        .join("skills")
}

/// 项目技能目录。
pub fn project_dir(root: &Path) -> PathBuf {
    root.join(".riot").join("skills")
}

/// 扫描一个项目的可用技能：项目级在前（同名赢），全局在后。
pub fn discover(project_root: &Path) -> Discovered {
    discover_in(&project_dir(project_root), &global_dir())
}

fn discover_in(project: &Path, global: &Path) -> Discovered {
    let mut out = Discovered::default();
    scan_dir(project, &mut out);
    scan_dir(global, &mut out);
    out
}

fn scan_dir(dir: &Path, out: &mut Discovered) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return; // 目录不存在 = 没有技能，不是错误
    };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    // 排序让清单顺序稳定 —— 工具 prompt 进缓存前缀，read_dir 的随机顺序
    // 会让同一份技能集在两轮之间打碎 prompt cache。
    entries.sort();

    for entry in entries {
        let skill_md = entry.join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }
        let raw = match std::fs::read_to_string(&skill_md) {
            Ok(r) => r,
            Err(e) => {
                out.problems.push(Problem {
                    path: skill_md,
                    reason: format!("读不出来：{e}"),
                });
                continue;
            }
        };
        let fallback_name = entry
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unnamed")
            .to_owned();
        match parse_skill(&raw, &fallback_name, &entry) {
            Ok(card) => {
                // 同名先到先得：项目目录先扫，所以项目级赢。
                if out.cards.iter().any(|c| c.name == card.name) {
                    tracing::debug!(name = %card.name, "同名技能已存在（项目级优先），跳过");
                } else {
                    out.cards.push(card);
                }
            }
            Err(reason) => out.problems.push(Problem { path: skill_md, reason }),
        }
    }
}

/// 解析一个 SKILL.md。
fn parse_skill(raw: &str, fallback_name: &str, dir: &Path) -> Result<SkillCard, String> {
    let rest = raw
        .strip_prefix("---")
        .ok_or("缺 frontmatter：文件要以 --- 开头，里面至少写一行 description")?;
    let (front, body) = rest
        .split_once("\n---")
        .ok_or("frontmatter 没有结束的 ---")?;

    let mut name = None;
    let mut description = None;
    for line in front.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        // 只认单行标量。值两侧的引号剥掉 —— 用户从别处抄来的
        // frontmatter 常带引号。
        let value = value.trim().trim_matches('"').trim_matches('\'').to_owned();
        match key.trim() {
            "name" => name = Some(value),
            "description" => description = Some(value),
            _ => {} // allowed-tools / model 等字段先不支持，忽略而不是报错
        }
    }

    let description = description
        .filter(|d| !d.is_empty())
        .ok_or("缺 description —— 它是模型决定要不要加载的唯一依据")?;
    let name = name.filter(|n| !n.is_empty()).unwrap_or_else(|| fallback_name.to_owned());

    let mut body = body.trim_start_matches('\n').to_owned();
    if body.chars().count() > MAX_BODY_CHARS {
        body = body.chars().take(MAX_BODY_CHARS).collect();
        body.push_str("\n\n[正文超长已截断。数据文件应该放在技能目录里让模型按需读取，而不是全部写进 SKILL.md]");
    }
    if body.trim().is_empty() {
        return Err("正文是空的：frontmatter 之后要写这个技能的具体做法".into());
    }

    Ok(SkillCard {
        name,
        description,
        dir: dir.to_path_buf(),
        body,
    })
}

/// 设置页的技能清单：可用的和有问题的都列出来。
pub fn list(project_root: Option<&Path>) -> Vec<SkillInfo> {
    let global = global_dir();
    let d = match project_root {
        Some(root) => discover(root),
        None => discover_in(Path::new("/nonexistent-no-project"), &global),
    };

    let mut out: Vec<SkillInfo> = d
        .cards
        .into_iter()
        .map(|c| {
            let source = if c.dir.starts_with(&global) { "global" } else { "project" };
            SkillInfo {
                name: c.name,
                description: c.description,
                path: c.dir.join("SKILL.md").display().to_string(),
                source: source.into(),
                error: None,
            }
        })
        .collect();
    out.extend(d.problems.into_iter().map(|p| SkillInfo {
        name: p
            .path
            .parent()
            .and_then(|d| d.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_owned(),
        description: String::new(),
        source: if p.path.starts_with(&global) { "global" } else { "project" }.into(),
        path: p.path.display().to_string(),
        error: Some(p.reason),
    }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(base: &Path, name: &str, content: &str) {
        let dir = base.join(name);
        std::fs::create_dir_all(&dir).expect("建技能目录");
        std::fs::write(dir.join("SKILL.md"), content).expect("写 SKILL.md");
    }

    fn dirs() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let t = tempfile::tempdir().expect("临时目录");
        let project = t.path().join("proj/.riot/skills");
        let global = t.path().join("global/skills");
        std::fs::create_dir_all(&project).expect("建目录");
        std::fs::create_dir_all(&global).expect("建目录");
        (t, project, global)
    }

    #[test]
    fn 解析_frontmatter_和正文() {
        let (_t, project, global) = dirs();
        write_skill(
            &global,
            "release",
            "---\nname: 发布\ndescription: \"发布新版本时用\"\n---\n第一步：跑测试。\n",
        );

        let d = discover_in(&project, &global);
        assert!(d.problems.is_empty(), "{:?}", d.problems.first().map(|p| &p.reason));
        assert_eq!(d.cards.len(), 1);
        assert_eq!(d.cards[0].name, "发布");
        assert_eq!(d.cards[0].description, "发布新版本时用", "引号要剥掉");
        assert!(d.cards[0].body.contains("第一步"));
    }

    #[test]
    fn 缺_description_报问题而不是静默消失() {
        // 静默跳过的话，用户写了技能却看不到它，也不知道为什么 ——
        // 设置页要能显示原因。
        let (_t, project, global) = dirs();
        write_skill(&global, "bad", "---\nname: x\n---\n正文\n");

        let d = discover_in(&project, &global);
        assert!(d.cards.is_empty());
        assert_eq!(d.problems.len(), 1);
        assert!(d.problems[0].reason.contains("description"), "{}", d.problems[0].reason);
    }

    #[test]
    fn 名字缺省用目录名() {
        let (_t, project, global) = dirs();
        write_skill(&global, "deploy", "---\ndescription: d\n---\n正文\n");
        let d = discover_in(&project, &global);
        assert_eq!(d.cards[0].name, "deploy");
    }

    #[test]
    fn 项目级覆盖同名全局技能() {
        // 项目里的约定比全局偏好更具体。反过来的话，用户在项目里
        // 定制的流程会被全局那份悄悄顶掉。
        let (_t, project, global) = dirs();
        write_skill(&global, "release", "---\nname: 发布\ndescription: 全局\n---\n全局做法\n");
        write_skill(&project, "release", "---\nname: 发布\ndescription: 项目\n---\n项目做法\n");

        let d = discover_in(&project, &global);
        assert_eq!(d.cards.len(), 1);
        assert_eq!(d.cards[0].description, "项目", "项目级必须赢");
    }

    #[test]
    fn 没有技能目录不是错误() {
        let t = tempfile::tempdir().expect("临时目录");
        let d = discover_in(&t.path().join("nope1"), &t.path().join("nope2"));
        assert!(d.cards.is_empty());
        assert!(d.problems.is_empty());
    }

    #[test]
    fn 没有_frontmatter_的文件报问题() {
        let (_t, project, global) = dirs();
        write_skill(&global, "plain", "就是一段普通 markdown\n");
        let d = discover_in(&project, &global);
        assert!(d.cards.is_empty());
        assert!(d.problems[0].reason.contains("---"), "{}", d.problems[0].reason);
    }

    #[test]
    fn 清单顺序稳定() {
        // 工具 prompt 进缓存前缀。read_dir 顺序随机的话，同一份技能集
        // 两轮之间会打碎 prompt cache。
        let (_t, project, global) = dirs();
        write_skill(&global, "bbb", "---\ndescription: b\n---\nx\n");
        write_skill(&global, "aaa", "---\ndescription: a\n---\nx\n");
        let names: Vec<String> = discover_in(&project, &global)
            .cards
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, vec!["aaa", "bbb"]);
    }
}
