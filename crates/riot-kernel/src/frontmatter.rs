//! SKILL.md 与命令文件共用的 frontmatter 解析。
//!
//! 两件事：把 `---` 包起来的头从正文里拆出来（[`split`]），把头里的
//! `key: value` 读成键值对（[`fields`]）。值认三种写法：
//!
//! ```text
//! description: 一行说完                     单行标量（引号剥掉）
//! description: >-                           块标量：`>` 折叠成一段、`|` 保留换行，
//!   两三行                                  可带 `-` / `+`（对 trim 过的值没有区别）
//!   缩进正文
//! description:                              空值 + 缩进续行，按折叠处理
//!   两三行缩进正文
//! ```
//!
//! 列表、嵌套映射、流式集合都不认：未知的键整体忽略，嵌套映射的子键
//! （缩进且形如 `key: value`）也当成不认识的键忽略。
//!
//! # 为什么要认块标量
//!
//! Cursor 和 Claude Code 生态里的 SKILL.md 普遍把 description 写成
//! `description: >-` 加两三行缩进正文。老解析按行 split，拿到的 description
//! 是字面的 `>-`：非空、过校验、静默进模型清单 —— 技能在清单里显示为
//! `- foo: >-`，模型无从判断要不要加载，设置页也不报错。用户从别处拷一个
//! 技能目录过来就会踩到，这正是最忌讳的静默失效。
//!
//! # 为什么不上 YAML 库
//!
//! frontmatter 里出现过的形状就上面这几种。完整的 YAML 解析器带来的是锚点、
//! 多文档、类型推断（`yes` 变 true、`08:00` 变六十进制）这一堆和技能文件
//! 无关的行为，每一样都是一个"用户写了 X，读出来是 Y"的坑。

use std::iter::Peekable;

/// [`split`] 的结果。
#[derive(Debug, PartialEq, Eq)]
pub enum Split<'a> {
    /// 文件不以 `---` 开头：没有 frontmatter，整个文件是正文。
    None,
    /// 开了 `---` 没关。
    Unterminated,
    /// 头（不含两条 `---`）和正文。
    Some { front: &'a str, body: &'a str },
}

/// 把 frontmatter 从正文里拆出来。
pub fn split(raw: &str) -> Split<'_> {
    let Some(rest) = raw.strip_prefix("---") else {
        return Split::None;
    };
    match rest.split_once("\n---") {
        Some((front, body)) => Split::Some { front, body },
        None => Split::Unterminated,
    }
}

/// 头里的键值对，按出现顺序。键两侧空白去掉；值见模块文档。
pub fn fields(front: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut lines = front.lines().peekable();
    while let Some(line) = lines.next() {
        // 顶层键不缩进。走到这里的缩进行只可能是嵌套映射的子键（块标量的
        // 续行在下面被整块吃掉了），不认，跳过。
        if line.trim().is_empty() || is_indented(line) {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        let value = match block_style(value) {
            Some(style) => collect_block(&mut lines, style),
            // 空值后面跟缩进的非映射行：YAML 的多行普通标量，等价于折叠。
            // 续行形如 `key: value` 的是嵌套映射，不是这一种 —— 普通标量
            // 里不允许出现「冒号 + 空格」，这正是 YAML 自己的区分规则。
            None if value.is_empty() && lines.peek().is_some_and(|l| is_plain_continuation(l)) => {
                collect_block(&mut lines, Style::Folded)
            }
            None => unquote(value),
        };
        out.push((key.trim().to_owned(), value));
    }
    out
}

#[derive(Clone, Copy)]
enum Style {
    /// `>`：行与行之间用空格连成一段，空行是段落分隔。
    Folded,
    /// `|`：保留换行。
    Literal,
}

/// `>` / `|`，后面只允许 chomping 指示符（`-` / `+`）和缩进指示符（一位数字）。
fn block_style(value: &str) -> Option<Style> {
    let style = match value.chars().next()? {
        '>' => Style::Folded,
        '|' => Style::Literal,
        _ => return None,
    };
    let rest = &value[1..];
    (rest.len() <= 2 && rest.chars().all(|c| matches!(c, '-' | '+') || c.is_ascii_digit()))
        .then_some(style)
}

fn is_indented(line: &str) -> bool {
    line.starts_with([' ', '\t'])
}

/// 缩进、非空、且不是 `key: value` 形状的行。
fn is_plain_continuation(line: &str) -> bool {
    let body = line.trim();
    is_indented(line) && !body.is_empty() && !body.contains(": ") && !body.ends_with(':')
}

/// 吃掉接下来所有缩进行和空行，按风格拼成一个值。首尾空行丢掉。
fn collect_block<'a, I>(lines: &mut Peekable<I>, style: Style) -> String
where
    I: Iterator<Item = &'a str>,
{
    let mut raw: Vec<&str> = Vec::new();
    while let Some(next) = lines.peek() {
        if next.trim().is_empty() {
            raw.push("");
        } else if is_indented(next) {
            raw.push(next);
        } else {
            break;
        }
        lines.next();
    }
    while raw.last() == Some(&"") {
        raw.pop();
    }
    let skip_leading = raw.iter().take_while(|l| l.is_empty()).count();
    let raw = &raw[skip_leading..];

    // 字面块要保留**相对**缩进：以第一行的缩进为基准，只剥掉那么多。
    let indent = raw
        .first()
        .map(|l| l.len() - l.trim_start().len())
        .unwrap_or(0);

    match style {
        Style::Literal => raw
            .iter()
            .map(|l| strip_indent(l, indent).trim_end())
            .collect::<Vec<_>>()
            .join("\n"),
        Style::Folded => {
            let mut s = String::new();
            let mut at_paragraph_start = true;
            for l in raw {
                if l.is_empty() {
                    // 连续空行只算一个段落分隔。
                    if !at_paragraph_start {
                        s.push('\n');
                    }
                    at_paragraph_start = true;
                    continue;
                }
                if !at_paragraph_start {
                    s.push(' ');
                }
                s.push_str(l.trim());
                at_paragraph_start = false;
            }
            s
        }
    }
}

/// 剥掉最多 `indent` 个字节的前导空白（和 `indent` 的算法同一把尺：字节）。
fn strip_indent(line: &str, indent: usize) -> &str {
    let mut cut = 0;
    for (i, c) in line.char_indices() {
        if i >= indent || !c.is_whitespace() {
            break;
        }
        cut = i + c.len_utf8();
    }
    &line[cut..]
}

/// 单行值：去空白、剥引号。用户从别处抄来的 frontmatter 常带引号。
fn unquote(value: &str) -> String {
    value.trim().trim_matches('"').trim_matches('\'').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(fields: &'a [(String, String)], key: &str) -> Option<&'a str> {
        fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn 拆头_三种情况() {
        assert_eq!(split("就是正文"), Split::None);
        assert_eq!(split("---\nname: x\n没关"), Split::Unterminated);
        assert_eq!(
            split("---\nname: x\n---\n正文\n"),
            Split::Some {
                front: "\nname: x",
                body: "\n正文\n"
            }
        );
    }

    #[test]
    fn 单行标量剥引号() {
        let f = fields("name: \"发布\"\ndescription: '发布新版本时用'\nflag: true");
        assert_eq!(get(&f, "name"), Some("发布"));
        assert_eq!(get(&f, "description"), Some("发布新版本时用"));
        assert_eq!(get(&f, "flag"), Some("true"));
    }

    /// Cursor 的 SKILL.md 就是这个形状。老解析拿到的是字面的 `>-`。
    #[test]
    fn 折叠块标量连成一段() {
        let f = fields(
            "name: skill-authoring\n\
             description: >-\n\
             \x20 When you notice a reusable multi-step task worth saving, or the user asks you\n\
             \x20 to save, change, or delete a skill.\n\
             globs: \"*.md\"",
        );
        assert_eq!(
            get(&f, "description"),
            Some(
                "When you notice a reusable multi-step task worth saving, or the user asks you \
                 to save, change, or delete a skill."
            )
        );
        assert_eq!(get(&f, "globs"), Some("*.md"), "块之后的键要照常读到");
        assert_eq!(get(&f, "name"), Some("skill-authoring"));
    }

    /// 续行里有冒号也是正文，不能被当成一个新键。
    #[test]
    fn 折叠块的续行含冒号仍是正文() {
        let f = fields("description: >\n  Use when: the user asks for X.\n  Not for: Y.\n");
        assert_eq!(
            get(&f, "description"),
            Some("Use when: the user asks for X. Not for: Y.")
        );
        assert!(get(&f, "Use when").is_none(), "{f:?}");
    }

    #[test]
    fn 折叠块空行是段落分隔() {
        let f = fields("description: >-\n  第一段\n  还是第一段\n\n  第二段\n");
        assert_eq!(get(&f, "description"), Some("第一段 还是第一段\n第二段"));
    }

    #[test]
    fn 字面块保留换行和相对缩进() {
        let f = fields("description: |\n  第一行\n    缩进的第二行\n  第三行\nname: x");
        assert_eq!(
            get(&f, "description"),
            Some("第一行\n  缩进的第二行\n第三行")
        );
        assert_eq!(get(&f, "name"), Some("x"));
    }

    /// `description:` 空着、下一行缩进正文 —— YAML 的多行普通标量。
    #[test]
    fn 空值加缩进续行按折叠处理() {
        let f = fields("description:\n  发布新版本时用。\n  跑测试、打 tag。\nname: x");
        assert_eq!(get(&f, "description"), Some("发布新版本时用。 跑测试、打 tag。"));
        assert_eq!(get(&f, "name"), Some("x"));
    }

    /// 嵌套映射不认：父键读到空值，子键不冒充顶层键。
    ///
    /// 这条守的是"子键不冒充顶层"：`metadata:` 下面藏一个 `description:`
    /// 不能把技能的 description 顶掉。
    #[test]
    fn 嵌套映射整体忽略() {
        let f = fields("metadata:\n  author: 张三\n  description: 里面的\ndescription: 外面的");
        assert_eq!(get(&f, "metadata"), Some(""));
        assert!(get(&f, "author").is_none(), "{f:?}");
        assert_eq!(get(&f, "description"), Some("外面的"));
    }

    #[test]
    fn 块指示符只认合法形状() {
        assert!(block_style(">").is_some());
        assert!(block_style(">-").is_some());
        assert!(block_style("|+").is_some());
        assert!(block_style("|2-").is_some());
        assert!(block_style(">> quoted").is_none(), "不是块标量的普通值");
        assert!(block_style("|x").is_none());
        assert!(block_style("").is_none());
    }

    #[test]
    fn 块到文件末尾也能收() {
        let f = fields("description: >-\n  最后一个键\n  没有后继\n");
        assert_eq!(get(&f, "description"), Some("最后一个键 没有后继"));
    }
}
