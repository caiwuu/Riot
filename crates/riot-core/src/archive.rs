//! 压缩归档：把被压掉的历史渲染成一份给**模型**翻的原文。
//!
//! # 为什么要有它
//!
//! LLM 总结是有损的，而且写总结的时候不知道后面会需要什么 —— 精确的报错
//! 原文、具体路径、某个数字、用户某句原话，总结完基本就没了。总结负责让
//! 模型知道"自己在干什么"，这份归档负责"想查细节的时候查得到"：宿主把它
//! 落成文件，续接消息里给出路径，模型用 Read/Grep 自己去翻。
//!
//! # 格式取舍
//!
//! 读者是 grep，不是人。所以：
//!
//! - 一条消息一个 `## [序号] 角色` 小节，序号跨段连续（宿主传入起点），
//!   模型能用 `\[123\]` 定位到一条。
//! - 工具结果**截断**（[`MAX_RESULT_BYTES`]）。会话里绝大部分字节是文件
//!   内容和命令输出，模型要文件可以重新 Read；留头部是因为报错和命令输出
//!   的要点通常在前几十行。截断处注明原始长度，模型知道后面还有。
//! - 图片换占位，思考块丢弃（脚手架不是事实），记忆/工作集附件只留路径
//!   （压缩后会重注一份新的，这里再抄一遍纯占地方）。
//! - 不做任何转义。Markdown 是给结构的，不是给渲染的 —— 转义反而让 grep
//!   搜不到原文。

use riot_protocol::message::{
    AssistantContent, Attachment, Message, ToolResultContent, UserContent,
};

/// 单条工具结果在归档里最多保留多少字节。
///
/// 8k 字节约 2k token：一份编译报错、一段 `git status`、一个 grep 结果的
/// 要点都装得下；整个源文件装不下 —— 那本来就该重新 Read。
pub const MAX_RESULT_BYTES: usize = 8_000;

/// 工具调用参数最多保留多少字节。Write/Edit 的参数就是文件内容，能到
/// 几十 k；调用了什么、改了哪个文件在前几行就看得出来。
const MAX_INPUT_BYTES: usize = 2_000;

/// 把一段消息渲染成归档文本。`first_index` 是第一条消息的序号（1 起）——
/// 宿主按已归档的条数传入，多次压缩的序号才能连续。
pub fn render(messages: &[Message], first_index: usize) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# 压缩边界：以下 {} 条消息（[{}] – [{}]）已从上下文中移出\n\n",
        messages.len(),
        first_index,
        first_index + messages.len().saturating_sub(1),
    ));
    for (i, m) in messages.iter().enumerate() {
        render_message(&mut out, m, first_index + i);
        out.push('\n');
    }
    out
}

fn render_message(out: &mut String, m: &Message, index: usize) {
    match m {
        Message::User { id, content, meta } => {
            let role = if meta.synthetic {
                "用户（系统合成）"
            } else if content
                .iter()
                .all(|c| matches!(c, UserContent::ToolResult { .. }))
            {
                "工具结果"
            } else {
                "用户"
            };
            out.push_str(&format!("## [{index}] {role} ({})\n\n", id.as_str()));
            for c in content {
                render_user_content(out, c);
            }
        }
        Message::Assistant {
            id, content, meta, ..
        } => {
            let flag = if meta.interrupted {
                "，被用户中断"
            } else {
                ""
            };
            out.push_str(&format!("## [{index}] 助手 ({}{flag})\n\n", id.as_str()));
            for c in content {
                match c {
                    AssistantContent::Text { text } => {
                        out.push_str(text.trim_end());
                        out.push_str("\n\n");
                    }
                    AssistantContent::Thinking { .. } => {}
                    AssistantContent::ToolUse { id, name, input } => {
                        let raw =
                            serde_json::to_string_pretty(input).unwrap_or_else(|_| "{}".into());
                        out.push_str(&format!("### 调用工具 {name}（{}）\n\n", id.as_str()));
                        push_truncated(out, &raw, MAX_INPUT_BYTES);
                        out.push('\n');
                    }
                }
            }
        }
        Message::System { id, level, text } => {
            out.push_str(&format!(
                "## [{index}] 系统提示 ({}, {level:?})\n\n{}\n\n",
                id.as_str(),
                text.trim_end()
            ));
        }
    }
}

fn render_user_content(out: &mut String, c: &UserContent) {
    match c {
        UserContent::Text { text } => {
            out.push_str(text.trim_end());
            out.push_str("\n\n");
        }
        UserContent::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            let tag = if *is_error { "，失败" } else { "" };
            out.push_str(&format!(
                "### 工具结果（{}{tag}）\n\n",
                tool_use_id.as_str()
            ));
            match content {
                ToolResultContent::Text { text } => push_truncated(out, text, MAX_RESULT_BYTES),
                ToolResultContent::Spilled {
                    path,
                    preview,
                    total_bytes,
                } => {
                    out.push_str(&format!(
                        "[结果过大（{total_bytes} 字节），完整内容在 {}]\n",
                        path.display()
                    ));
                    push_truncated(out, preview, MAX_RESULT_BYTES);
                }
                ToolResultContent::Cleared => out.push_str("[结果已在更早的清理中移除]\n"),
                ToolResultContent::Image { path, .. } => match path {
                    Some(p) => out.push_str(&format!("[图片结果，原图在 {}]\n", p.display())),
                    None => out.push_str("[图片结果]\n"),
                },
                ToolResultContent::DescribedImage { text, path, .. }
                | ToolResultContent::MarkedImage { text, path, .. } => {
                    if let Some(p) = path {
                        out.push_str(&format!("[图片结果，原图在 {}]\n", p.display()));
                    }
                    push_truncated(out, text, MAX_RESULT_BYTES);
                }
            }
            out.push('\n');
        }
        UserContent::Attachment(a) => render_attachment(out, a),
    }
}

fn render_attachment(out: &mut String, a: &Attachment) {
    match a {
        Attachment::Memory { path, .. } => {
            out.push_str(&format!("[项目记忆 {}，内容略]\n\n", path.display()));
        }
        Attachment::RestoredFile { path, .. } => {
            out.push_str(&format!(
                "[压缩后重注的工作集文件 {}，内容略]\n\n",
                path.display()
            ));
        }
        Attachment::UserFile { path, content } => {
            out.push_str(&format!("### 用户附带的文件 {}\n\n", path.display()));
            push_truncated(out, content, MAX_RESULT_BYTES);
            out.push('\n');
        }
        Attachment::Environment { text } | Attachment::SystemReminder { text } => {
            out.push_str("### 系统注入\n\n");
            push_truncated(out, text, MAX_RESULT_BYTES);
            out.push('\n');
        }
        Attachment::Image { .. } => out.push_str("[用户附的图片]\n\n"),
        Attachment::DescribedImage { text, .. } => {
            out.push_str("### 用户附的图片（转述）\n\n");
            push_truncated(out, text, MAX_RESULT_BYTES);
            out.push('\n');
        }
    }
}

/// 追加一段文本，超出 `max` 字节就截在字符边界上并注明原始长度。
fn push_truncated(out: &mut String, text: &str, max: usize) {
    let text = text.trim_end();
    if text.len() <= max {
        out.push_str(text);
        out.push('\n');
        return;
    }
    let mut cut = max;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    out.push_str(&text[..cut]);
    out.push_str(&format!(
        "\n…[已截断，原文共 {} 字节，这里只保留前 {cut} 字节]\n",
        text.len()
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use riot_protocol::id::{MessageId, ToolUseId};
    use riot_protocol::message::MessageMeta;

    fn user(id: &str, text: &str) -> Message {
        Message::User {
            id: MessageId::from_raw(id),
            content: vec![UserContent::Text { text: text.into() }],
            meta: MessageMeta::default(),
        }
    }

    #[test]
    fn 每条消息一个带序号的小节_序号从起点连续() {
        let msgs = vec![user("m1", "第一句"), user("m2", "第二句")];
        let s = render(&msgs, 41);
        assert!(s.contains("## [41] 用户 (m1)"), "{s}");
        assert!(s.contains("## [42] 用户 (m2)"), "{s}");
        assert!(s.contains("[41] – [42]"), "头部要写清范围：{s}");
        assert!(s.contains("第一句") && s.contains("第二句"));
    }

    #[test]
    fn 工具调用与结果都保留_思考丢弃() {
        let msgs = vec![
            Message::Assistant {
                id: MessageId::from_raw("a1"),
                content: vec![
                    AssistantContent::Thinking {
                        text: "内心戏".into(),
                        signature: None,
                    },
                    AssistantContent::Text {
                        text: "我来读文件".into(),
                    },
                    AssistantContent::ToolUse {
                        id: ToolUseId::from_raw("t1"),
                        name: "Read".into(),
                        input: serde_json::json!({"path": "/p/a.rs"}),
                    },
                ],
                usage: None,
                meta: MessageMeta::default(),
            },
            Message::User {
                id: MessageId::from_raw("r1"),
                content: vec![UserContent::ToolResult {
                    tool_use_id: ToolUseId::from_raw("t1"),
                    content: ToolResultContent::text("error[E0308]: mismatched types"),
                    is_error: true,
                }],
                meta: MessageMeta::default(),
            },
        ];
        let s = render(&msgs, 1);
        assert!(!s.contains("内心戏"), "思考是脚手架，不进归档：{s}");
        assert!(s.contains("我来读文件"));
        assert!(
            s.contains("调用工具 Read（t1）") && s.contains("/p/a.rs"),
            "{s}"
        );
        assert!(
            s.contains("## [2] 工具结果"),
            "纯工具结果的消息要标成工具结果：{s}"
        );
        assert!(s.contains("（t1，失败）"), "失败要标出来：{s}");
        assert!(s.contains("E0308"), "报错原文是归档最重要的东西：{s}");
    }

    #[test]
    fn 超长结果截断并注明原始长度() {
        let big = "x".repeat(MAX_RESULT_BYTES * 3);
        let msgs = vec![Message::User {
            id: MessageId::from_raw("r1"),
            content: vec![UserContent::ToolResult {
                tool_use_id: ToolUseId::from_raw("t1"),
                content: ToolResultContent::text(big.clone()),
                is_error: false,
            }],
            meta: MessageMeta::default(),
        }];
        let s = render(&msgs, 1);
        assert!(s.len() < big.len() / 2, "没截断：{}", s.len());
        assert!(
            s.contains(&format!("原文共 {} 字节", big.len())),
            "要告诉模型后面还有多少：{s}"
        );
    }

    #[test]
    fn 截断落在字符边界上() {
        // 中文 3 字节一个，MAX 不是 3 的倍数时直接切会劈开一个字 —— 那是 panic。
        let big = "中".repeat(MAX_RESULT_BYTES);
        let msgs = vec![user("m1", &big)];
        // 用户文本不截断；用工具结果那条路来测截断逻辑。
        let msgs2 = vec![Message::User {
            id: MessageId::from_raw("r1"),
            content: vec![UserContent::ToolResult {
                tool_use_id: ToolUseId::from_raw("t1"),
                content: ToolResultContent::text(big),
                is_error: false,
            }],
            meta: MessageMeta::default(),
        }];
        let _ = render(&msgs, 1);
        let s = render(&msgs2, 1);
        assert!(s.contains("已截断"), "{s}");
    }

    #[test]
    fn 图片换占位_记忆只留路径() {
        let msgs = vec![Message::User {
            id: MessageId::from_raw("m1"),
            content: vec![
                UserContent::Attachment(Attachment::Memory {
                    path: "/p/AGENTS.md".into(),
                    content: "很长的约定".repeat(100),
                }),
                UserContent::Attachment(Attachment::Image {
                    media_type: "image/png".into(),
                    data: "AAAA".repeat(1000),
                }),
                UserContent::Text {
                    text: "看图".into(),
                },
            ],
            meta: MessageMeta::default(),
        }];
        let s = render(&msgs, 1);
        assert!(!s.contains("AAAA"), "base64 不进归档：{s}");
        assert!(
            s.contains("/p/AGENTS.md") && !s.contains("很长的约定"),
            "{s}"
        );
        assert!(s.contains("看图"));
    }

    #[test]
    fn 合成消息与中断标记可见() {
        let msgs = vec![
            Message::User {
                id: MessageId::from_raw("m1"),
                content: vec![UserContent::Text {
                    text: "前文总结".into(),
                }],
                meta: MessageMeta {
                    synthetic: true,
                    ..Default::default()
                },
            },
            Message::Assistant {
                id: MessageId::from_raw("a1"),
                content: vec![AssistantContent::Text {
                    text: "半截".into(),
                }],
                usage: None,
                meta: MessageMeta {
                    interrupted: true,
                    ..Default::default()
                },
            },
        ];
        let s = render(&msgs, 1);
        assert!(
            s.contains("用户（系统合成）"),
            "合成消息不能冒充用户原话：{s}"
        );
        assert!(s.contains("被用户中断"), "{s}");
    }
}
