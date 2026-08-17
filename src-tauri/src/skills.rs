//! Skill 的发现与解析。
//!
//! # 三层来源
//!
//! ```text
//! <项目根>/.riot/skills/<名字>/SKILL.md      项目技能，只在该项目的会话里
//! <配置目录>/skills/<名字>/SKILL.md          全局技能，用户自己写的
//! （编进二进制）                              内置技能，随应用分发
//! ```
//!
//! 同名时**越具体的赢**：项目 > 全局 > 内置（和 git 的 local > global
//! 同一直觉）。内置的排最后是刻意的 —— 用户想改内置技能的做法时，写一个
//! 同名的就能盖掉，不需要去找应用包里的文件，也不用等版本更新。
//!
//! # 内置技能为什么编进二进制
//!
//! `[约束]` 内置技能走 `include_str!`，**不走文件系统**。
//!
//! 理由和 Grep 用 ripgrep 的库而不是它的二进制是同一条：桌面应用不能假设
//! 运行时的文件布局。打包后的资源目录、开发时的 target 目录、用户手动挪过
//! 位置的 `.app`，三种情况路径都不一样，而任何一种解析失败的表现都是
//! 「内置技能莫名其妙消失了」—— 那种问题在用户机器上没法复现。
//!
//! 代价是加一个内置技能要改一行 Rust（见 [`BUILTIN_SKILLS`]）。换来的是
//! 「能装上就一定在」。
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

/// 随应用分发的技能。源文件在 `src-tauri/builtin/skills/<名字>/SKILL.md`。
///
/// 加一个：放好文件，然后在这里加一行。见模块文档「为什么编进二进制」。
///
/// `[约束]` 只放**在任何项目里都成立**的东西。这个清单会跟着应用装到每台
/// 机器上，往里塞 Riot 自己仓库的规矩（怎么跑 `cargo clippy -p riot-tools`、
/// `Tool` trait 怎么实现）等于给所有用户的所有项目发一份用不上的说明书。
/// 那类东西属于 Riot 仓库的 `.riot/skills/`。
const BUILTIN_SKILLS: &[(&str, &str)] = &[
    (
        "extend-riot",
        include_str!("../builtin/skills/extend-riot/SKILL.md"),
    ),
    ("commit", include_str!("../builtin/skills/commit/SKILL.md")),
    ("review", include_str!("../builtin/skills/review/SKILL.md")),
    ("verify", include_str!("../builtin/skills/verify/SKILL.md")),
    ("debug", include_str!("../builtin/skills/debug/SKILL.md")),
    (
        "simplify",
        include_str!("../builtin/skills/simplify/SKILL.md"),
    ),
    (
        "skillify",
        include_str!("../builtin/skills/skillify/SKILL.md"),
    ),
    (
        "split-to-prs",
        include_str!("../builtin/skills/split-to-prs/SKILL.md"),
    ),
];

/// 发现结果：能用的进工具，有问题的报给设置页。
#[derive(Default)]
pub struct Discovered {
    /// 全部可用技能。**包含**只给用户的那些 —— 它们要出现在 `/` 菜单里。
    pub cards: Vec<SkillCard>,
    pub problems: Vec<Problem>,
    /// frontmatter 写了 `disable-model-invocation: true` 的技能名。
    ///
    /// 这些只出现在 `/` 菜单里，不进 Skill 工具的清单。用途是"我想有个
    /// 快捷入口，但不希望模型自己去调它" —— 比如一个会改动很多文件的流程。
    pub slash_only: std::collections::HashSet<String>,
    /// 最终生效的这一份是内置技能（不是用户写的）。
    ///
    /// 记「最终生效的」而不是「存在同名内置」：用户写了同名的就以他为准，
    /// 这时这个名字不该再被标成内置 —— 设置页要显示的是**实际在用的那份**
    /// 来自哪里。
    pub builtin: std::collections::HashSet<String>,
}

impl Discovered {
    /// 模型能主动调的那些。
    ///
    /// `[约束]` Skill 工具只该拿到这一份。拿全量的话 `disable-model-invocation`
    /// 就成了一个骗人的开关 —— 它写在 frontmatter 里，用户会以为生效了。
    pub fn model_cards(&self) -> Vec<SkillCard> {
        self.cards
            .iter()
            .filter(|c| !self.slash_only.contains(&c.name))
            .cloned()
            .collect()
    }
}

#[derive(Clone)]
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
    /// SKILL.md 的完整路径。内置技能没有路径（编进了二进制），给空串。
    pub path: String,
    /// `builtin` / `global` / `project`。
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

/// 扫描一个项目的可用技能：项目级 > 全局 > 内置（同名先到先得）。
pub fn discover(project_root: &Path) -> Discovered {
    discover_in(&project_dir(project_root), &global_dir())
}

/// 没有活跃项目时（设置页）也要能列：全局 + 内置。
///
/// 内置技能不依赖项目，少了它们设置页会显示"还没有技能"，而它们明明装着。
pub fn discover_opt(project_root: Option<&Path>) -> Discovered {
    match project_root {
        Some(root) => discover(root),
        None => discover_in(Path::new("/nonexistent-no-project"), &global_dir()),
    }
}

fn discover_in(project: &Path, global: &Path) -> Discovered {
    let mut out = discover_dirs(project, global);
    // 内置**最后**扫：同名先到先得，所以用户写的那份赢。
    scan_builtin(&mut out);
    out
}

/// 只扫两个目录，不含内置。
///
/// 单独留一个入口是给测试用的：目录扫描的规则（同名优先级、顺序稳定、
/// 解析失败进 problems）和内置那一层无关，混在一起测的话每加一个内置技能
/// 都要去改一堆断言里的数字。
fn discover_dirs(project: &Path, global: &Path) -> Discovered {
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
            Ok((card, slash_only)) => {
                // 同名先到先得：项目目录先扫，所以项目级赢。
                if out.cards.iter().any(|c| c.name == card.name) {
                    tracing::debug!(name = %card.name, "同名技能已存在（项目级优先），跳过");
                } else {
                    if slash_only {
                        out.slash_only.insert(card.name.clone());
                    }
                    out.cards.push(card);
                }
            }
            Err(reason) => out.problems.push(Problem { path: skill_md, reason }),
        }
    }
}

/// 扫内置技能（编进二进制的那些）。
///
/// 解析失败进 `problems` 而不是 panic：一个写坏的内置技能不该让应用起不来。
/// 真正防它的是 `内置技能都能解析` 那个用例 —— 在 CI 里拦住，不在用户机器上。
fn scan_builtin(out: &mut Discovered) {
    for (name, raw) in BUILTIN_SKILLS {
        // 内置技能没有目录：它不在文件系统上，也就不可能有同目录的数据文件。
        // 因此 `${SKILL_DIR}` 对它没有意义（有用例守着这一点）。
        match parse_skill(raw, name, Path::new("")) {
            Ok((card, slash_only)) => {
                if out.cards.iter().any(|c| c.name == card.name) {
                    tracing::debug!(name = %card.name, "用户写了同名技能，内置的那份让位");
                } else {
                    if slash_only {
                        out.slash_only.insert(card.name.clone());
                    }
                    out.builtin.insert(card.name.clone());
                    out.cards.push(card);
                }
            }
            Err(reason) => out.problems.push(Problem {
                path: PathBuf::from(format!("<内置技能 {name}>")),
                reason,
            }),
        }
    }
}

/// 解析一个 SKILL.md。返回 `(卡片, 是否只给用户调)`。
fn parse_skill(raw: &str, fallback_name: &str, dir: &Path) -> Result<(SkillCard, bool), String> {
    let rest = raw
        .strip_prefix("---")
        .ok_or("缺 frontmatter：文件要以 --- 开头，里面至少写一行 description")?;
    let (front, body) = rest
        .split_once("\n---")
        .ok_or("frontmatter 没有结束的 ---")?;

    let mut name = None;
    let mut description = None;
    let mut slash_only = false;
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
            // 只给用户 `/` 调，不进 Skill 工具。
            "disable-model-invocation" => slash_only = value.eq_ignore_ascii_case("true"),
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

    Ok((
        SkillCard {
            name,
            description,
            dir: dir.to_path_buf(),
            body,
        },
        slash_only,
    ))
}

/// 设置页的技能清单：可用的和有问题的都列出来。
pub fn list(project_root: Option<&Path>) -> Vec<SkillInfo> {
    let global = global_dir();
    let d = discover_opt(project_root);

    let mut out: Vec<SkillInfo> = d
        .cards
        .iter()
        .map(|c| {
            // 内置的先判：它的 dir 是空路径，落到下面的前缀判断会被误认成
            // "project"（空路径不以全局目录开头）。
            let builtin = d.builtin.contains(&c.name);
            let source = if builtin {
                "builtin"
            } else if c.dir.starts_with(&global) {
                "global"
            } else {
                "project"
            };
            SkillInfo {
                name: c.name.clone(),
                description: c.description.clone(),
                // 内置技能不在文件系统上，没有路径可给。
                path: if builtin {
                    String::new()
                } else {
                    c.dir.join("SKILL.md").display().to_string()
                },
                source: source.into(),
                error: None,
            }
        })
        .collect();
    out.extend(d.problems.iter().cloned().map(|p| SkillInfo {
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

        let d = discover_dirs(&project, &global);
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

        let d = discover_dirs(&project, &global);
        assert!(d.cards.is_empty());
        assert_eq!(d.problems.len(), 1);
        assert!(d.problems[0].reason.contains("description"), "{}", d.problems[0].reason);
    }

    #[test]
    fn 名字缺省用目录名() {
        let (_t, project, global) = dirs();
        write_skill(&global, "deploy", "---\ndescription: d\n---\n正文\n");
        let d = discover_dirs(&project, &global);
        assert_eq!(d.cards[0].name, "deploy");
    }

    #[test]
    fn 项目级覆盖同名全局技能() {
        // 项目里的约定比全局偏好更具体。反过来的话，用户在项目里
        // 定制的流程会被全局那份悄悄顶掉。
        let (_t, project, global) = dirs();
        write_skill(&global, "release", "---\nname: 发布\ndescription: 全局\n---\n全局做法\n");
        write_skill(&project, "release", "---\nname: 发布\ndescription: 项目\n---\n项目做法\n");

        let d = discover_dirs(&project, &global);
        assert_eq!(d.cards.len(), 1);
        assert_eq!(d.cards[0].description, "项目", "项目级必须赢");
    }

    #[test]
    fn 没有技能目录不是错误() {
        let t = tempfile::tempdir().expect("临时目录");
        let d = discover_dirs(&t.path().join("nope1"), &t.path().join("nope2"));
        assert!(d.cards.is_empty());
        assert!(d.problems.is_empty());
    }

    #[test]
    fn 没有_frontmatter_的文件报问题() {
        let (_t, project, global) = dirs();
        write_skill(&global, "plain", "就是一段普通 markdown\n");
        let d = discover_dirs(&project, &global);
        assert!(d.cards.is_empty());
        assert!(d.problems[0].reason.contains("---"), "{}", d.problems[0].reason);
    }

    /// 内置技能必须全部能解析。
    ///
    /// 这是 `BUILTIN_SKILLS` 存在的前提：它们编进了二进制，装到每台机器上，
    /// 而一个 frontmatter 写坏的内置技能不会让任何东西编译失败 —— 只会让那条
    /// 流程在**所有用户**那里静默地不存在。要在 CI 拦住，不能等用户发现。
    #[test]
    fn 内置技能都能解析() {
        let mut out = Discovered::default();
        scan_builtin(&mut out);

        assert!(
            out.problems.is_empty(),
            "内置技能解析失败：{:?}",
            out.problems.iter().map(|p| (&p.path, &p.reason)).collect::<Vec<_>>()
        );
        assert_eq!(out.cards.len(), BUILTIN_SKILLS.len(), "清单里的每一条都该出来");
        assert_eq!(out.builtin.len(), BUILTIN_SKILLS.len(), "都该被标成内置");

        for c in &out.cards {
            // 清单进工具 prompt，单条描述硬顶 250 字符（见 skill.rs）。
            assert!(
                c.description.chars().count() <= 250,
                "内置技能「{}」的 description 有 {} 字，会被截断",
                c.name,
                c.description.chars().count()
            );
            assert!(!c.body.trim().is_empty(), "内置技能「{}」正文是空的", c.name);
        }

        // 刻意**不**断言正文里没有 `${SKILL_DIR}`：内置技能没有目录，但
        // 「扩展 Riot」那个正是在讲解这个占位符，文本里出现它是对的。
        // 真正要守的是"没有目录时不做替换"，那是 skill.rs 的职责，
        // 用例在 `riot-tools` 那边（搜 `没有目录的技能不替换占位符`）。
    }

    /// 用户写的同名技能要盖掉内置的。
    ///
    /// 这是内置技能排最后扫的全部意义：想改内置流程的做法时，写一个同名的
    /// 就行，不用去翻应用包里的文件，也不用等版本更新。
    #[test]
    fn 用户的同名技能盖掉内置的() {
        let name = BUILTIN_SKILLS[0].0;
        let (_t, project, global) = dirs();
        write_skill(
            &project,
            name,
            &format!("---\nname: {name}\ndescription: 我自己的版本\n---\n我的做法\n"),
        );

        let d = discover_in(&project, &global);
        let hit: Vec<&SkillCard> = d.cards.iter().filter(|c| c.name == name).collect();
        assert_eq!(hit.len(), 1, "不该出现两条同名");
        assert_eq!(hit[0].description, "我自己的版本", "用户的那份必须赢");
        assert!(
            !d.builtin.contains(name),
            "被盖掉之后不该再标成内置 —— 设置页要显示实际在用的那份来自哪里"
        );
    }

    /// 没有活跃项目时（设置页刚打开、还没建会话）也要列出内置技能。
    ///
    /// 少了这条，设置页会显示"还没有技能"，而它们明明装着。
    #[test]
    fn 没有项目也能列出内置技能() {
        let list = list(None);
        assert!(
            list.iter().any(|s| s.source == "builtin"),
            "没有项目时该列出内置技能，实际：{:?}",
            list.iter().map(|s| (&s.name, &s.source)).collect::<Vec<_>>()
        );
        assert!(
            list.iter().filter(|s| s.source == "builtin").all(|s| s.path.is_empty()),
            "内置技能不在文件系统上，不该给出路径"
        );
    }

    /// 仓库自带的技能必须全部能解析。
    ///
    /// 它们是"机制之外的内容"—— 一个 frontmatter 写坏的 SKILL.md 不会让
    /// 任何东西编译失败，只会让那条流程**静默地不存在**，而模型照旧在没有
    /// 它的情况下干活，谁都不会注意到。
    #[test]
    fn 仓库自带的技能都能解析() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("src-tauri 的上一级是仓库根");
        let d = discover_dirs(&project_dir(repo), Path::new("/nonexistent-no-global"));

        assert!(
            d.problems.is_empty(),
            "自带技能解析失败：{:?}",
            d.problems.iter().map(|p| (&p.path, &p.reason)).collect::<Vec<_>>()
        );
        assert!(!d.cards.is_empty(), "仓库该自带技能，一个都没扫到说明目录没跟着走");
        for c in &d.cards {
            // 清单进工具 prompt，单条描述硬顶 250 字符（见 skill.rs）。
            // 超了会被截断，模型就是在残句上做"要不要加载"的判断。
            assert!(
                c.description.chars().count() <= 250,
                "技能「{}」的 description 有 {} 字，会被截断",
                c.name,
                c.description.chars().count()
            );
        }
    }

    /// `disable-model-invocation` 必须真的把技能从模型那侧摘掉。
    ///
    /// 只解析不生效是最坏的一种：用户在 frontmatter 里写了它，设置页也
    /// 列着这个技能，而模型照样能调 —— 没有任何迹象表明开关没生效。
    #[test]
    fn 只给用户调的技能不进模型清单() {
        let (_t, project, global) = dirs();
        write_skill(
            &global,
            "danger",
            "---\nname: danger\ndescription: d\ndisable-model-invocation: true\n---\n正文\n",
        );
        write_skill(&global, "normal", "---\nname: normal\ndescription: d\n---\n正文\n");

        let d = discover_dirs(&project, &global);
        assert!(d.problems.is_empty(), "{:?}", d.problems.first().map(|p| &p.reason));
        assert_eq!(d.cards.len(), 2, "两个都该被发现（`/` 菜单要用全量）");

        let model = d.model_cards();
        let names: Vec<&str> = model.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["normal"], "写了开关的不该进模型清单");
    }

    #[test]
    fn 开关只认_true() {
        let (_t, project, global) = dirs();
        // 写成别的值按"没关"算 —— fail-open 的方向在这里是对的：
        // 拼错一个值不该让技能从模型那侧静默消失。
        write_skill(
            &global,
            "s",
            "---\nname: s\ndescription: d\ndisable-model-invocation: no\n---\n正文\n",
        );
        let d = discover_dirs(&project, &global);
        assert_eq!(d.model_cards().len(), 1, "「no」不是 true，该照常给模型");
        assert!(d.slash_only.is_empty());
    }

    #[test]
    fn 清单顺序稳定() {
        // 工具 prompt 进缓存前缀。read_dir 顺序随机的话，同一份技能集
        // 两轮之间会打碎 prompt cache。
        let (_t, project, global) = dirs();
        write_skill(&global, "bbb", "---\ndescription: b\n---\nx\n");
        write_skill(&global, "aaa", "---\ndescription: a\n---\nx\n");
        let names: Vec<String> = discover_dirs(&project, &global)
            .cards
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, vec!["aaa", "bbb"]);
    }
}
