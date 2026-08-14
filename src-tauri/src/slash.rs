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
    /// `builtin` / `global` / `project`。
    pub source: String,
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
    out
}

/// 展开一条命令：`/name args` → 发给模型的 prompt。
///
/// `None` = 没这条命令，或它是内置命令（内置由前端按 name 执行，
/// 没有模板可展开）。
pub fn expand(project_root: Option<&Path>, name: &str, args: &str) -> Option<String> {
    let cmd = discover(project_root).into_iter().find(|c| c.name == name)?;
    (!cmd.body.is_empty()).then(|| substitute(&cmd.body, args))
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
            }),
            Err(reason) => {
                tracing::warn!(path = %path.display(), reason = %reason, "命令解析失败，跳过");
            }
        }
    }
}

fn collect_md(base: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
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
            let Some((key, value)) = line.split_once(':') else { continue };
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
    let description = description.filter(|d| !d.is_empty()).unwrap_or_else(|| {
        body.lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(60)
            .collect()
    });
    Ok((description, argument_hint.filter(|h| !h.is_empty()), body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 解析带_frontmatter_的命令() {
        let (d, h, b) = parse("---\ndescription: 写 PR\nargument-hint: [分支]\n---\n给 $1 写 PR").expect("解析");
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

    #[test]
    fn 参数展开_arguments_与位置参数() {
        assert_eq!(substitute("给 $1 写 PR：$ARGUMENTS", "main 加了缓存"), "给 main 写 PR：main 加了缓存");
        assert_eq!(substitute("第二个是 $2", "a b c"), "第二个是 b");
        assert_eq!(substitute("缺参数：[$3]", "a"), "缺参数：[]", "没给的位置参数留空");
    }

    #[test]
    fn 参数展开_引号算一个() {
        assert_eq!(
            substitute("[$1] [$2]", r#""hello world" second"#),
            "[hello world] [second]"
        );
        assert_eq!(split_args(""), Vec::<String>::new());
        assert_eq!(split_args(r#"a "" b"#), vec!["a", "", "b"], "空引号是一个空参数");
    }

    #[test]
    fn 无占位符时把参数追加到末尾() {
        // 否则用户敲的参数被静默扔掉，看起来像"这个命令不认参数"。
        assert_eq!(substitute("跑测试", "只跑单测"), "跑测试\n\nARGUMENTS: 只跑单测");
        assert_eq!(substitute("跑测试", ""), "跑测试", "没参数就不加尾巴");
        assert_eq!(substitute("用 $ARGUMENTS", ""), "用 ", "有占位符就不追加");
    }

    #[test]
    fn 非占位符的美元号原样保留() {
        assert_eq!(substitute("echo $HOME 和 $(date)", ""), "echo $HOME 和 $(date)");
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
        let compact = out.iter().find(|c| c.name == "compact").expect("有 compact");
        assert_eq!(compact.source, "builtin", "内置命令不能被自定义盖掉");
    }
}
