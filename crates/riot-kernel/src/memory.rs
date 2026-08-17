//! 记忆文件：AGENTS.md 的发现与展开。
//!
//! # 层级（对照 Claude Code 的 claudemd.ts，做了两处刻意的差异）
//!
//! 1. **全局**：`<配置目录>/riot/AGENTS.md` —— 用户的跨项目偏好；
//! 2. **项目**：`<项目根>/AGENTS.md`（回退 `CLAUDE.md`）—— 进仓库的团队约定。
//!
//! 顺序即注入顺序：全局在前、项目在后 —— 越靠近对话的越晚出现，模型
//! 对后出现的内容权重更高（CC 文档原话："latest files are highest
//! priority"），项目约定应当压过全局偏好。
//!
//! `[取舍]` 和 CC 的两处不同：
//! - **认 AGENTS.md 优先**。它是业界的中立标准（CC 不认它，因为 CC 有
//!   自家的 CLAUDE.md 生态）；Riot 没有历史包袱，跟标准走，同时回退
//!   CLAUDE.md 照顾存量仓库。同一目录两个都有时**只取 AGENTS.md** ——
//!   两份都注入的话内容多半重复，还会互相矛盾。
//! - **不向上遍历父目录**。CC 从文件系统根一路收到 cwd（monorepo 场景），
//!   但 Riot 的会话围栏哲学是"绑定 root、不出去"——工具出不去的地方，
//!   宿主悄悄读文件同样反直觉。monorepo 用户可以用 `@../AGENTS.md`
//!   显式引进来，显式比隐式好。
//!
//! # `@path` 引用（对齐 CC 的 include 语法）
//!
//! 记忆文件里 `@./docs/style.md` 这样的行内引用会展开成被引文件的内容：
//! 相对路径以**包含它的文件**所在目录为基准；`~/` 展开到家目录；
//! 深度上限 5；循环引用靠已处理集合掐断；文件不存在静默跳过（CC 同款
//! 语义 —— 引用是可选增强，坏引用不该让整个记忆加载失败）。
//! 围栏代码块和行内反引号里的 `@` 不展开 —— 那是代码，不是引用。
//!
//! # 注入
//!
//! 会话的**第一条用户消息**里，以 [`Attachment::Memory`] 附在正文之前
//! （providers 会包成 system-reminder）。只注入一次：它随消息进入历史和
//! transcript，往后每轮都自然带着。压缩后的重注入由压缩管线负责。
//!
//! 豁免理由：宿主层，读的是用户自己的记忆文件。

#![allow(clippy::disallowed_methods)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// 单文件上限（字符）。CC 对 40k 只警告不截断；我们再放宽一点但**硬截**：
/// 记忆是每个会话都付的常驻成本，一个失控的大文件该在这里被拦住，
/// 而不是让每次对话都少半个上下文窗口。
const MAX_FILE_CHARS: usize = 64 * 1024;

/// CC 的警告线。超过说明这文件该拆了（用 @ 引用拆成模块）。
const WARN_FILE_CHARS: usize = 40 * 1024;

/// `@path` 引用的最大深度（CC: MAX_INCLUDE_DEPTH = 5）。
const MAX_INCLUDE_DEPTH: usize = 5;

/// 一份找到的记忆。
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryFile {
    pub path: PathBuf,
    pub content: String,
}

/// 收集一个会话该带上的全部记忆文件（已展开 @ 引用、已截断超限内容）。
pub fn collect(project_root: &Path) -> Vec<MemoryFile> {
    let global_dir = crate::config::config_path()
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();
    collect_in(&global_dir, project_root)
}

/// 路径参数化的实现。测试用临时目录，不碰真实配置。
fn collect_in(global_dir: &Path, project_root: &Path) -> Vec<MemoryFile> {
    let mut out = Vec::new();
    // 全局只认 AGENTS.md —— 配置目录是 Riot 自己的地盘，没有存量
    // CLAUDE.md 要照顾。
    if let Some(m) = load_expanded(&global_dir.join("AGENTS.md")) {
        out.push(m);
    }
    // 项目层：AGENTS.md 优先，回退 CLAUDE.md。只取一个。
    for name in ["AGENTS.md", "CLAUDE.md"] {
        if let Some(m) = load_expanded(&project_root.join(name)) {
            out.push(m);
            break;
        }
    }
    out
}

/// 读一个记忆文件并展开它的 @ 引用。不存在返回 None。
fn load_expanded(path: &Path) -> Option<MemoryFile> {
    let raw = std::fs::read_to_string(path).ok()?;
    if raw.trim().is_empty() {
        return None; // 空文件等于没有，别注入一个空壳附件
    }
    let mut visited = HashSet::new();
    // realpath 进已访问集合：软链两条路径指向同一文件时也要认出环。
    visited.insert(canonical(path));
    let content = expand(&raw, path, 0, &mut visited);
    Some(MemoryFile {
        path: path.to_path_buf(),
        content: cap(content, path),
    })
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// 超限截断。CC 只警告；我们截断 + 尾注说明（理由见常量注释）。
fn cap(mut s: String, path: &Path) -> String {
    let chars = s.chars().count();
    if chars > MAX_FILE_CHARS {
        tracing::warn!(path = %path.display(), chars, "记忆文件超过上限，已截断");
        s = s.chars().take(MAX_FILE_CHARS).collect();
        s.push_str("\n\n[记忆文件超长已截断。把大段内容拆成单独文件，用 @路径 按需引用。]");
    } else if chars > WARN_FILE_CHARS {
        tracing::warn!(
            path = %path.display(),
            chars,
            "记忆文件偏大（每个会话都要付这份上下文），考虑拆分"
        );
    }
    s
}

/// 展开一段记忆文本里的 @ 引用。
///
/// 逐行扫描：围栏代码块整段跳过；行内反引号内的内容剥掉再扫 ——
/// `` `npm i @types/node` `` 里的 @ 是代码，不是引用。
fn expand(text: &str, base: &Path, depth: usize, visited: &mut HashSet<PathBuf>) -> String {
    let base_dir = base.parent().unwrap_or(Path::new("."));
    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;

    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        out.push_str(line);
        out.push('\n');
        if in_fence {
            continue;
        }

        for raw_ref in extract_refs(line) {
            let Some(target) = resolve_ref(&raw_ref, base_dir) else {
                continue;
            };
            let real = canonical(&target);
            if visited.contains(&real) || depth + 1 >= MAX_INCLUDE_DEPTH {
                continue; // 环或太深：静默掐断（引用是可选增强）
            }
            let Ok(included) = std::fs::read_to_string(&target) else {
                continue; // 不存在静默跳过 —— CC 同款语义
            };
            visited.insert(real);
            let expanded = expand(&included, &target, depth + 1, visited);
            out.push_str(&format!(
                "\n<引用文件 路径=\"{}\">\n{}\n</引用文件>\n",
                target.display(),
                expanded.trim_end(),
            ));
        }
    }
    out
}

/// 从一行文本里挑出 @ 引用（已保证不在围栏代码块里）。
///
/// 规则：`@` 前面必须是行首或空白（`user@host` 不算）；路径到下一个
/// 空白为止；行内反引号包住的内容先剥掉。
fn extract_refs(line: &str) -> Vec<String> {
    // 剥行内代码：`...` 之间的内容替换成空格，长度无所谓，只是别扫进去。
    let mut cleaned = String::with_capacity(line.len());
    let mut in_tick = false;
    for c in line.chars() {
        if c == '`' {
            in_tick = !in_tick;
            cleaned.push(' ');
        } else {
            cleaned.push(if in_tick { ' ' } else { c });
        }
    }

    let mut refs = Vec::new();
    let mut prev_is_space = true;
    for (i, c) in cleaned.char_indices() {
        if c == '@' && prev_is_space {
            let rest: String = cleaned[i + 1..]
                .chars()
                .take_while(|c| !c.is_whitespace())
                .collect();
            // 至少要像个路径：纯 @ 或 @handle 这种社交语法不算。
            // 约束到带路径特征的（含 / 或 . 开头或 ~ 开头）。
            if !rest.is_empty()
                && (rest.contains('/') || rest.starts_with('.') || rest.starts_with('~'))
            {
                refs.push(rest);
            }
        }
        prev_is_space = c.is_whitespace();
    }
    refs
}

/// 把引用路径解析成绝对路径。支持 `@/abs`、`@~/home`、`@./rel`、`@rel`。
fn resolve_ref(r: &str, base_dir: &Path) -> Option<PathBuf> {
    // 去掉尾部常见的标点（引用出现在句子里："见 @./docs/a.md。"）
    let r = r.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '）', '。', '，', '；']);
    if r.is_empty() {
        return None;
    }
    if let Some(home_rel) = r.strip_prefix("~/") {
        let home = std::env::var("HOME").ok()?;
        return Some(Path::new(&home).join(home_rel));
    }
    let p = Path::new(r);
    Some(if p.is_absolute() { p.to_path_buf() } else { base_dir.join(p) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("临时目录")
    }

    fn write(base: &Path, rel: &str, content: &str) -> PathBuf {
        let p = base.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("建目录");
        }
        std::fs::write(&p, content).expect("写文件");
        p
    }

    #[test]
    fn 全局和项目按序收集() {
        let d = dir();
        let global = d.path().join("cfg");
        let project = d.path().join("proj");
        write(&global, "AGENTS.md", "全局偏好");
        write(&project, "AGENTS.md", "项目约定");

        let files = collect_in(&global, &project);
        assert_eq!(files.len(), 2);
        assert!(files[0].content.contains("全局偏好"), "全局在前");
        assert!(files[1].content.contains("项目约定"), "项目在后 —— 越晚出现权重越高");
    }

    #[test]
    fn 项目层_agents_优先_claude_回退_不重复注入() {
        let d = dir();
        let global = d.path().join("cfg");
        let project = d.path().join("proj");
        write(&project, "AGENTS.md", "A");
        write(&project, "CLAUDE.md", "C");

        let files = collect_in(&global, &project);
        assert_eq!(files.len(), 1, "两个都有时只取一个 —— 两份多半重复还互相矛盾");
        assert!(files[0].content.contains('A'), "AGENTS.md 优先");

        std::fs::remove_file(project.join("AGENTS.md")).expect("删");
        let files = collect_in(&global, &project);
        assert_eq!(files.len(), 1);
        assert!(files[0].content.contains('C'), "没有 AGENTS.md 时回退 CLAUDE.md");
    }

    #[test]
    fn 都没有时为空_空文件不注入() {
        let d = dir();
        let project = d.path().join("proj");
        std::fs::create_dir_all(&project).expect("建目录");
        assert!(collect_in(&d.path().join("cfg"), &project).is_empty());

        write(&project, "AGENTS.md", "   \n  ");
        assert!(
            collect_in(&d.path().join("cfg"), &project).is_empty(),
            "空文件等于没有，别注入空壳附件"
        );
    }

    #[test]
    fn 引用展开_相对路径以包含文件为基准() {
        let d = dir();
        let project = d.path().join("proj");
        write(&project, "AGENTS.md", "总则。\n细节见 @./docs/style.md 一文。\n");
        write(&project, "docs/style.md", "缩进用两个空格。\n再看 @./naming.md\n");
        write(&project, "docs/naming.md", "驼峰命名。");

        let files = collect_in(&d.path().join("cfg"), &project);
        let c = &files[0].content;
        assert!(c.contains("缩进用两个空格"), "一层引用要展开：{c}");
        assert!(
            c.contains("驼峰命名"),
            "嵌套引用相对于 docs/ 解析（包含文件的目录），不是项目根：{c}"
        );
        assert!(c.contains("<引用文件"), "展开要带来源路径，模型才知道内容从哪来");
    }

    #[test]
    fn 循环引用被掐断() {
        let d = dir();
        let project = d.path().join("proj");
        write(&project, "AGENTS.md", "@./a.md");
        write(&project, "a.md", "甲 @./b.md");
        write(&project, "b.md", "乙 @./a.md");

        let files = collect_in(&d.path().join("cfg"), &project);
        let c = &files[0].content;
        assert!(c.contains('甲') && c.contains('乙'), "链条本身要展开：{c}");
        assert_eq!(c.matches('甲').count(), 1, "环必须掐断，不能无限展开");
    }

    #[test]
    fn 代码块和行内代码里的引用不展开() {
        let d = dir();
        let project = d.path().join("proj");
        write(
            &project,
            "AGENTS.md",
            "正文 @./real.md\n```\n@./fenced.md\n```\n安装 `npm i @types/node` 即可。\n",
        );
        write(&project, "real.md", "真引用");
        write(&project, "fenced.md", "不该出现");

        let c = &collect_in(&d.path().join("cfg"), &project)[0].content;
        assert!(c.contains("真引用"));
        assert!(!c.contains("不该出现"), "围栏代码块里的 @ 是代码不是引用：{c}");
        assert!(!c.contains("引用文件 路径=\"@types"), "行内反引号里的 @ 不是引用");
    }

    #[test]
    fn 社交语法的_at_不当成引用() {
        let d = dir();
        let project = d.path().join("proj");
        write(&project, "AGENTS.md", "联系 user@example.com 或 @teamname 询问。");
        let files = collect_in(&d.path().join("cfg"), &project);
        assert!(
            !files[0].content.contains("<引用文件"),
            "邮箱和 @提及 不是文件引用"
        );
    }

    #[test]
    fn 不存在的引用静默跳过() {
        let d = dir();
        let project = d.path().join("proj");
        write(&project, "AGENTS.md", "见 @./nope.md 里的说明。");
        let files = collect_in(&d.path().join("cfg"), &project);
        assert_eq!(files.len(), 1, "坏引用不该让整个记忆加载失败");
        assert!(files[0].content.contains("见 @./nope.md"), "原文保留");
    }

    #[test]
    fn 超限截断并留说明() {
        let d = dir();
        let project = d.path().join("proj");
        write(&project, "AGENTS.md", &"长".repeat(MAX_FILE_CHARS + 100));
        let files = collect_in(&d.path().join("cfg"), &project);
        let c = &files[0].content;
        assert!(c.chars().count() < MAX_FILE_CHARS + 200, "必须截断 —— 这是每个会话都付的成本");
        assert!(c.contains("已截断"), "要告诉模型内容不完整");
    }

    #[test]
    fn 句尾标点不进引用路径() {
        let d = dir();
        let project = d.path().join("proj");
        write(&project, "AGENTS.md", "规范见 @./style.md。");
        write(&project, "style.md", "规范内容");
        let c = &collect_in(&d.path().join("cfg"), &project)[0].content;
        assert!(c.contains("规范内容"), "句号该被剥掉：{c}");
    }
}
