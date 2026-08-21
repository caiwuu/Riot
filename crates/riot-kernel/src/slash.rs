//! 斜杠命令：`/name args` 展开成一段 prompt 发出去。
//!
//! # 目录布局
//!
//! ```text
//! <配置目录>/riot/commands/**/*.md     全局命令
//! <项目根>/.riot/commands/**/*.md      项目命令，同名赢（和技能同一直觉）
//! ```
//!
//! 子目录变成命名空间：`commands/git/pr.md` → `/git:pr`。
//!
//! # 文件格式
//!
//! ```text
//! ---
//! description: 生成一个 PR 描述
//! argument-hint: [分支名]
//! ---
//! 请给分支 $1 写 PR 描述。完整参数：$ARGUMENTS
//! ```
//!
//! frontmatter 可省略（没有 `---` 开头就整个文件当正文，description
//! 取正文第一行）。正文是模板：`$ARGUMENTS` 换成整段参数原文，
//! `$1..$9` 按空白拆分取第 N 个（带引号的段落算一个）。模板里一个
//! 占位符都没有而用户又给了参数时，追加 `ARGUMENTS: <args>`（CC 同款，
//! 否则参数被静默扔掉）。
//!
//! 模板里可以写 `@路径`：展开后的 prompt 走的是普通发送那条路，消息级
//! 的 `@` 引用（见 [`crate::mentions`]）会照常把文件内容带上，这里不用
//! 也不该再实现一遍。
//!
//! `` !`cmd` `` 嵌入执行**不支持**：那是把"展开提示词"变成"执行任意
//! 命令"的口子，要做也得先过权限闸。
//!
//! 参数替换在宿主（[`expand`]），命令的选择和发送在前端。
//!
//! 豁免理由：宿主层，读的是用户自己的命令目录。

#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use serde::Serialize;

/// 单个模板的上限，同技能一个量级。
const MAX_BODY_CHARS: usize = 64 * 1024;

/// 给前端的命令清单条目。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlashCommand {
    /// 不带斜杠的名字（可能含命名空间，如 `git:pr`）。
    pub name: String,
    pub description: String,
    /// 补全菜单里的参数提示，如 `[分支名]`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    /// 模板正文。**不发给前端** —— 前端只需要清单和展开结果，
    /// 一份几十 KB 的模板过 IPC 是白花的钱。
    #[serde(skip)]
    pub body: String,
    /// `builtin` / `project` / `global` / `skill`。
    pub source: String,
    /// 用户敲 `/名字` 时，是否就地展开成提示词。
    ///
    /// 规则是「模型加载不了的才展开」：
    ///
    /// - 命令 → true。它本来就是提示词模板，没有别的加载路径。
    /// - 普通技能 → **false**。把名字发给模型，由它用 Skill 工具按需加载
    ///   正文 —— 渐进披露的意义就在这里，几 KB 正文不该塞进用户可见的消息。
    /// - 写了 `disable-model-invocation` 的技能 → true。模型的清单里没有它，
    ///   不展开的话它谁都跑不了。
    pub expand_inline: bool,
    /// 技能自己的层级：`builtin` / `global` / `project`。只有 `source == "skill"` 才有。
    ///
    /// 需要它是因为光说「技能」会在设置页造成两个页面**两个标签**：
    /// Skills 页把 `extend-riot` 标成「内置」，命令页标成「技能」，同一个东西
    /// 说两套话。带上层级之后两边的词汇是同一套（「内置技能」/「内置」）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_source: Option<String>,
    /// 技能目录，用来展开 `${SKILL_DIR}`。只有技能才有。
    #[serde(skip)]
    pub dir: Option<PathBuf>,
}

/// 全局命令目录。
pub fn global_dir() -> PathBuf {
    crate::config::config_path()
        .parent()
        .unwrap_or(Path::new("."))
        .join("commands")
}

/// 项目命令目录。
pub fn project_dir(root: &Path) -> PathBuf {
    root.join(".riot").join("commands")
}

/// 一个项目可用的全部命令：内置在前，然后项目级、全局（同名先到先得，
/// 所以项目级盖全局；自定义**不能**盖内置 —— 内置的行为要可预期）。
///
/// `project_root` 为 None（设置页里没有活跃会话）时只列内置 + 全局。
pub fn discover(project_root: Option<&Path>) -> Vec<SlashCommand> {
    let mut out = builtin();
    if let Some(root) = project_root {
        scan(&project_dir(root), "project", &mut out);
    }
    scan(&global_dir(), "global", &mut out);
    // 技能（含内置技能）排最后 = 优先级最低。没有活跃项目时也要走这条：
    // 内置技能不依赖项目，设置页里该看得到。
    add_skills(project_root, &mut out);
    out
}

/// 把技能也放进 `/` 菜单。
///
/// 技能和命令本来是两套东西：技能给模型按需加载，命令给用户敲。但用户
/// 侧的心智模型只有一个 —— 敲 `/` 看有什么可用。同一份 `SKILL.md` 既然
/// 已经写清了"什么时候、怎么做"，没有理由让用户为了自己调它再抄一份
/// `commands/verify.md`。
///
/// 排在最后 = 优先级最低。同名时命令赢：命令是专门为 `/` 写的，技能只是
/// 顺带可调用；反过来的话，用户写的命令会被一个同名技能悄悄顶掉。
fn add_skills(root: Option<&Path>, out: &mut Vec<SlashCommand>) {
    let found = crate::skills::discover_opt(root);
    let skills_global = crate::skills::global_dir();
    for card in found.cards {
        if out.iter().any(|c| c.name == card.name) {
            tracing::debug!(name = %card.name, "已有同名斜杠命令，技能不进菜单");
            continue;
        }
        // 模型加载不了的才就地展开，见 `expand_inline` 的说明。
        let expand_inline = found.slash_only.contains(&card.name);
        // 内置的先判：它的 dir 是空路径，落到前缀判断会被误认成 project。
        let skill_source = if found.builtin.contains(&card.name) {
            "builtin"
        } else if card.dir.starts_with(&skills_global) {
            "global"
        } else {
            "project"
        };
        out.push(SlashCommand {
            name: card.name,
            description: card.description,
            argument_hint: None,
            body: card.body,
            source: "skill".to_owned(),
            expand_inline,
            skill_source: Some(skill_source.to_owned()),
            dir: Some(card.dir),
        });
    }
}

/// 展开一条命令：`/name args` → 发给模型的 prompt。
///
/// `None` = 没这条命令，或它是内置命令（内置由前端按 name 执行，
/// 没有模板可展开）。
pub fn expand(project_root: Option<&Path>, name: &str, args: &str) -> Option<String> {
    let cmd = discover(project_root)
        .into_iter()
        .find(|c| c.name == name)?;
    if cmd.body.is_empty() {
        return None;
    }
    let mut text = substitute(&cmd.body, args);
    // 技能正文里可以写 ${SKILL_DIR}（模型调用那条路会替换它）。走 `/`
    // 这条路时也得替换，否则用户会在提示词里看到一个字面量占位符。
    if let Some(dir) = cmd.dir.as_deref() {
        text = text.replace("${SKILL_DIR}", &dir.display().to_string());
    }
    Some(text)
}

/// 参数替换（规则对齐 CC）：
/// - `$ARGUMENTS` → 整段参数原文
/// - `$1`..`$9` → 按空白拆分的第 N 个（`"带 空格"` 算一个，引号剥掉）
/// - 模板里一个占位符都没有、而用户给了参数 → 末尾追加 `ARGUMENTS: …`
///   （否则用户敲的参数被静默扔掉，看起来像命令不认参数）
fn substitute(body: &str, args: &str) -> String {
    let args = args.trim();
    let positional = split_args(args);

    let mut out = String::with_capacity(body.len() + args.len());
    let mut used_placeholder = false;
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        // $ARGUMENTS
        let rest: String = chars.clone().take("ARGUMENTS".len()).collect();
        if rest == "ARGUMENTS" {
            for _ in 0.."ARGUMENTS".len() {
                chars.next();
            }
            out.push_str(args);
            used_placeholder = true;
            continue;
        }
        // $1..$9
        match chars.peek().and_then(|d| d.to_digit(10)) {
            Some(n) if n >= 1 => {
                chars.next();
                if let Some(v) = positional.get((n - 1) as usize) {
                    out.push_str(v);
                }
                used_placeholder = true;
            }
            // 不是占位符（`$HOME`、`$(cmd)`）—— 原样留着，模板作者
            // 多半是想让 shell 或模型自己看到它。
            _ => out.push('$'),
        }
    }

    if !used_placeholder && !args.is_empty() {
        out.push_str("\n\nARGUMENTS: ");
        out.push_str(args);
    }
    out
}

/// 按空白拆参数，`"..."` / `'...'` 里的空白不算分隔（引号剥掉）。
fn split_args(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut has = false;
    for c in args.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                has = true;
            }
            None if c.is_whitespace() => {
                if has || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            None => cur.push(c),
        }
    }
    if has || !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn builtin() -> Vec<SlashCommand> {
    vec![SlashCommand {
        name: "compact".into(),
        description: "把对话历史压缩成摘要，腾出上下文窗口".into(),
        argument_hint: None,
        body: String::new(),
        source: "builtin".into(),
        // 内置命令由前端按名字执行，没有模板可展开。
        expand_inline: false,
        skill_source: None,
        dir: None,
    }]
}

fn scan(dir: &Path, source: &str, out: &mut Vec<SlashCommand>) {
    let mut files = Vec::new();
    collect_md(dir, dir, &mut files);
    // 排序保证清单稳定（read_dir 顺序随机）。
    files.sort();
    for (rel, path) in files {
        let raw = match std::fs::read_to_string(&path) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "命令文件读不出来，跳过");
                continue;
            }
        };
        let name = rel.trim_end_matches(".md").replace(['/', '\\'], ":");
        if name.is_empty() {
            continue;
        }
        if out.iter().any(|c| c.name == name) {
            tracing::debug!(name = %name, "同名命令已存在（内置 > 项目 > 全局），跳过");
            continue;
        }
        match parse(&raw) {
            Ok((description, argument_hint, body)) => out.push(SlashCommand {
                name,
                description,
                argument_hint,
                body,
                source: source.into(),
                // 命令就是提示词模板，没有别的加载路径。
                expand_inline: true,
                skill_source: None,
                dir: None,
            }),
            Err(reason) => {
                tracing::warn!(path = %path.display(), reason = %reason, "命令解析失败，跳过");
            }
        }
    }
}

fn collect_md(base: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_md(base, &p, out);
        } else if p.extension().and_then(|e| e.to_str()) == Some("md")
            && let Ok(rel) = p.strip_prefix(base)
        {
            out.push((rel.to_string_lossy().into_owned(), p));
        }
    }
}

/// 解析一个命令文件：frontmatter 可选。
fn parse(raw: &str) -> Result<(String, Option<String>, String), String> {
    let (front, body) = match raw.strip_prefix("---") {
        Some(rest) => match rest.split_once("\n---") {
            Some((f, b)) => (Some(f), b),
            None => return Err("frontmatter 没有结束的 ---".into()),
        },
        None => (None, raw),
    };

    let mut description = None;
    let mut argument_hint = None;
    if let Some(front) = front {
        for line in front.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim().trim_matches('"').trim_matches('\'').to_owned();
            match key.trim() {
                "description" => description = Some(value),
                "argument-hint" => argument_hint = Some(value),
                _ => {} // allowed-tools / model 等先不支持，忽略
            }
        }
    }

    let mut body = body.trim_start_matches('\n').trim_end().to_owned();
    if body.trim().is_empty() {
        return Err("正文是空的".into());
    }
    if body.chars().count() > MAX_BODY_CHARS {
        body = body.chars().take(MAX_BODY_CHARS).collect();
        body.push_str("\n\n[模板超长已截断]");
    }

    // 没写 description 就拿正文第一行凑 —— 补全菜单里一行说明必须有。
    let description = description
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| body.lines().next().unwrap_or("").chars().take(60).collect());
    Ok((description, argument_hint.filter(|h| !h.is_empty()), body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 解析带_frontmatter_的命令() {
        let (d, h, b) = parse("---\ndescription: 写 PR\nargument-hint: [分支]\n---\n给 $1 写 PR")
            .expect("解析");
        assert_eq!(d, "写 PR");
        assert_eq!(h.as_deref(), Some("[分支]"));
        assert_eq!(b, "给 $1 写 PR");
    }

    #[test]
    fn 没有_frontmatter_也认_描述取首行() {
        let (d, h, b) = parse("跑全部测试并总结失败原因\n细节…").expect("解析");
        assert_eq!(d, "跑全部测试并总结失败原因");
        assert!(h.is_none());
        assert!(b.starts_with("跑全部测试"));
    }

    #[test]
    fn 空正文报错() {
        assert!(parse("---\ndescription: x\n---\n\n").is_err());
    }

    /// 技能要出现在 `/` 菜单里，而且 `${SKILL_DIR}` 要被替换掉。
    ///
    /// 不替换的话用户在提示词里看到的是一个字面量占位符 —— 模型收到它只会
    /// 困惑，而这个 bug 在模型调用那条路上不存在（那边替换了），所以很容易
    /// 只修一半。
    #[test]
    fn 技能能当斜杠命令调用() {
        let t = tempfile::tempdir().expect("临时目录");
        let root = t.path();
        let dir = root.join(".riot/skills/verify");
        std::fs::create_dir_all(&dir).expect("建目录");
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: verify\ndescription: 跑验证\n---\n按 ${SKILL_DIR} 里的清单跑，参数：$ARGUMENTS\n",
        )
        .expect("写技能");

        let list = discover(Some(root));
        let found = list
            .iter()
            .find(|c| c.name == "verify")
            .expect("技能该在 / 菜单里");
        assert_eq!(found.source, "skill", "来源要标出来，用户得知道这是技能");

        let text = expand(Some(root), "verify", "只跑 clippy").expect("该展开");
        assert!(text.contains("只跑 clippy"), "$ARGUMENTS 要替换：{text}");
        assert!(
            !text.contains("${SKILL_DIR}"),
            "占位符不能漏到提示词里：{text}"
        );
        assert!(text.contains("verify"), "SKILL_DIR 该指向技能目录：{text}");
    }

    /// 「模型加载不了的才就地展开」这条规则。
    ///
    /// 两头都得对：普通技能展开了，就把几 KB 正文塞进了用户可见的消息，
    /// 渐进披露白做；而 `disable-model-invocation` 的技能不展开，就成了一个
    /// 谁都跑不了的死技能 —— 模型的清单里没有它，用户敲了也只是把名字发出去。
    #[test]
    fn 只有模型加载不了的技能才就地展开() {
        let t = tempfile::tempdir().expect("临时目录");
        let root = t.path();
        let base = root.join(".riot/skills");

        for (name, front) in [
            ("normal", "---\nname: normal\ndescription: d\n---\n正文\n"),
            (
                "handsoff",
                "---\nname: handsoff\ndescription: d\ndisable-model-invocation: true\n---\n正文\n",
            ),
        ] {
            let dir = base.join(name);
            std::fs::create_dir_all(&dir).expect("建目录");
            std::fs::write(dir.join("SKILL.md"), front).expect("写技能");
        }

        let list = discover(Some(root));
        let normal = list.iter().find(|c| c.name == "normal").expect("有");
        let handsoff = list.iter().find(|c| c.name == "handsoff").expect("有");

        assert!(
            !normal.expand_inline,
            "普通技能不该就地展开 —— 该由模型用 Skill 工具按需加载"
        );
        assert!(
            handsoff.expand_inline,
            "模型调不了的技能必须就地展开，否则谁都跑不了它"
        );
    }

    /// 同名时命令赢。反过来的话，用户亲手写的命令会被一个同名技能悄悄顶掉。
    #[test]
    fn 同名时斜杠命令压过技能() {
        let t = tempfile::tempdir().expect("临时目录");
        let root = t.path();

        let sk = root.join(".riot/skills/dup");
        std::fs::create_dir_all(&sk).expect("建目录");
        std::fs::write(
            sk.join("SKILL.md"),
            "---\nname: dup\ndescription: 技能版\n---\n技能正文\n",
        )
        .expect("写技能");

        let cmd = root.join(".riot/commands");
        std::fs::create_dir_all(&cmd).expect("建目录");
        std::fs::write(
            cmd.join("dup.md"),
            "---\ndescription: 命令版\n---\n命令正文\n",
        )
        .expect("写命令");

        let list = discover(Some(root));
        let hits: Vec<&SlashCommand> = list.iter().filter(|c| c.name == "dup").collect();
        assert_eq!(hits.len(), 1, "不该出现两条同名");
        assert_eq!(hits[0].description, "命令版", "命令必须赢");
    }

    /// 内置技能要出现在 `/` 菜单里，而且不依赖有没有活跃项目。
    ///
    /// 内置技能和内置命令走的是同一条管道：写一个内置技能就等于多了一条
    /// `/名字`。所以「内置命令」不需要另一套机制。
    #[test]
    fn 内置技能也是斜杠命令() {
        let list = discover(None);
        assert!(
            list.iter().any(|c| c.source == "skill"),
            "没有活跃项目时也该列出内置技能，实际：{:?}",
            list.iter()
                .map(|c| (&c.name, &c.source))
                .collect::<Vec<_>>()
        );
    }

    /// 技能在 `/` 菜单里要带上自己的层级。
    ///
    /// 只报 `source: "skill"` 的话，同一个 extend-riot 在 Skills 页显示
    /// 「内置」、在命令页显示「技能」—— 同一个东西两套说法，用户会问
    /// 「显示技能什么意思」（真发生过）。两边得用同一套词。
    #[test]
    fn 技能在命令清单里带着自己的层级() {
        let t = tempfile::tempdir().expect("临时目录");
        let root = t.path();
        let dir = root.join(".riot/skills/mine");
        std::fs::create_dir_all(&dir).expect("建目录");
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: mine\ndescription: d\n---\n正文\n",
        )
        .expect("写技能");

        let list = discover(Some(root));

        let mine = list
            .iter()
            .find(|c| c.name == "mine")
            .expect("项目技能该在");
        assert_eq!(mine.source, "skill");
        assert_eq!(mine.skill_source.as_deref(), Some("project"));

        // 内置技能同理，而它的 dir 是空路径 —— 不特判会被误认成 project。
        let builtin_skill = list
            .iter()
            .find(|c| c.source == "skill" && c.skill_source.as_deref() == Some("builtin"))
            .expect("内置技能该带 builtin 层级");
        assert!(!builtin_skill.name.is_empty());

        // 命令文件和内置命令没有这个字段 —— 它们不是技能。
        let compact = list
            .iter()
            .find(|c| c.name == "compact")
            .expect("内置命令该在");
        assert_eq!(compact.source, "builtin");
        assert!(compact.skill_source.is_none());
    }

    /// 内置命令不能被顶掉 —— 它们的行为要可预期。
    #[test]
    fn 技能顶不掉内置命令() {
        let t = tempfile::tempdir().expect("临时目录");
        let root = t.path();
        let sk = root.join(".riot/skills/compact");
        std::fs::create_dir_all(&sk).expect("建目录");
        std::fs::write(
            sk.join("SKILL.md"),
            "---\nname: compact\ndescription: 冒名\n---\n正文\n",
        )
        .expect("写技能");

        let list = discover(Some(root));
        let hits: Vec<&SlashCommand> = list.iter().filter(|c| c.name == "compact").collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source, "builtin", "内置必须赢");
    }

    #[test]
    fn 参数展开_arguments_与位置参数() {
        assert_eq!(
            substitute("给 $1 写 PR：$ARGUMENTS", "main 加了缓存"),
            "给 main 写 PR：main 加了缓存"
        );
        assert_eq!(substitute("第二个是 $2", "a b c"), "第二个是 b");
        assert_eq!(
            substitute("缺参数：[$3]", "a"),
            "缺参数：[]",
            "没给的位置参数留空"
        );
    }

    #[test]
    fn 参数展开_引号算一个() {
        assert_eq!(
            substitute("[$1] [$2]", r#""hello world" second"#),
            "[hello world] [second]"
        );
        assert_eq!(split_args(""), Vec::<String>::new());
        assert_eq!(
            split_args(r#"a "" b"#),
            vec!["a", "", "b"],
            "空引号是一个空参数"
        );
    }

    #[test]
    fn 无占位符时把参数追加到末尾() {
        // 否则用户敲的参数被静默扔掉，看起来像"这个命令不认参数"。
        assert_eq!(
            substitute("跑测试", "只跑单测"),
            "跑测试\n\nARGUMENTS: 只跑单测"
        );
        assert_eq!(substitute("跑测试", ""), "跑测试", "没参数就不加尾巴");
        assert_eq!(substitute("用 $ARGUMENTS", ""), "用 ", "有占位符就不追加");
    }

    #[test]
    fn 非占位符的美元号原样保留() {
        assert_eq!(
            substitute("echo $HOME 和 $(date)", ""),
            "echo $HOME 和 $(date)"
        );
    }

    #[test]
    fn 发现_命名空间_与优先级() {
        let t = tempfile::tempdir().expect("目录");
        let project = t.path().join("proj/.riot/commands");
        std::fs::create_dir_all(project.join("git")).expect("目录");
        std::fs::write(project.join("git/pr.md"), "项目级 PR 模板").expect("写");
        std::fs::write(project.join("deploy.md"), "项目级部署").expect("写");
        // 自定义不能盖内置。
        std::fs::write(project.join("compact.md"), "假装是压缩").expect("写");

        let mut out = builtin();
        scan(&project, "project", &mut out);

        let names: Vec<&str> = out.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"git:pr"), "子目录变命名空间：{names:?}");
        assert!(names.contains(&"deploy"));
        let compact = out
            .iter()
            .find(|c| c.name == "compact")
            .expect("有 compact");
        assert_eq!(compact.source, "builtin", "内置命令不能被自定义盖掉");
    }
}
