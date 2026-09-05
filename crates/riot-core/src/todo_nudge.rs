//! 待办清单的兜底提醒。
//!
//! TodoWrite 的语义规则（"开工前标 in_progress、做完立刻标 completed"）只写在
//! prompt 里，不在代码里强制（见 riot-tools 的 todo.rs）。服从度高的模型
//! 没问题；服从度低的模型（会话日志里 deepseek 一次把五项标成 in_progress，
//! 然后二十几分钟不碰清单，最后一次性全标完成）会让界面上的进度条整轮
//! 不动 —— 用户看到的就是"全做完了才更新"。
//!
//! 这里做的是 Claude Code 同款的**带外提醒**：连续若干次工具调用没碰
//! TodoWrite、而清单里还有没做完的项，就往下一批工具结果后面塞一条
//! system-reminder，把清单现状摆给模型看。提醒过后计数归零 —— 模型继续
//! 无视的话，再过同样多次调用会再提一次，而不是每批都催。
//!
//! `[取舍]` 不在这里替模型改清单。清单的状态机在模型的上下文里（工具是
//! 无状态的整表替换），内核猜"哪项做完了"只会猜错；提醒它自己对账，
//! 比替它对账可靠。
//!
//! 清单来源是历史里最后一次 TodoWrite 的**输入**（不是结果 —— 结果是固定
//! 文案，不回显清单）。压缩把它压掉了就不提醒：模型本来也看不到清单了。

use riot_protocol::message::{AssistantContent, Message};

/// 连续这么多次工具调用没碰 TodoWrite 就提醒。和系统 prompt 里"NEVER go
/// more than 8 tool calls without an update"对齐 —— 两边说的是同一件事。
pub const NUDGE_AFTER_CALLS: usize = 8;

/// 工具名。riot-core 不依赖 riot-tools（依赖方向是反的），只能在这里再写
/// 一遍；riot-tools 的 names.rs 里有测试盯着两边一致。
pub const TODO_WRITE: &str = "TodoWrite";

/// 一批工具调用之后计数器该变成多少。
///
/// 按调用顺序走：碰到 TodoWrite 归零，其余的各加一。这样"TodoWrite 后面
/// 跟着五个调用"算五次，而不是算零次 —— 模型习惯把清单更新和后面的活
/// 放同一条消息里，那五个调用同样是清单没跟上的证据。
pub fn advance(count: usize, call_names: impl IntoIterator<Item = impl AsRef<str>>) -> usize {
    call_names.into_iter().fold(count, |n, name| {
        if name.as_ref() == TODO_WRITE {
            0
        } else {
            n + 1
        }
    })
}

/// 清单里没做完的项，取自历史里最后一次 TodoWrite 的输入。
///
/// 返回 `None` 表示不需要提醒：没用过清单、清单已全部完成、或者输入
/// 解析不出来（那次调用本身就失败了，模型已经从错误里知道了）。
pub fn unfinished(messages: &[Message]) -> Option<Vec<UnfinishedItem>> {
    let input = messages.iter().rev().find_map(|m| match m {
        Message::Assistant { content, .. } => content.iter().rev().find_map(|c| match c {
            AssistantContent::ToolUse { name, input, .. } if name == TODO_WRITE => Some(input),
            _ => None,
        }),
        _ => None,
    })?;
    let todos = input.get("todos")?.as_array()?;
    let items: Vec<UnfinishedItem> = todos
        .iter()
        .filter_map(|t| {
            let status = t.get("status")?.as_str()?;
            if status == "completed" {
                return None;
            }
            Some(UnfinishedItem {
                content: t.get("content")?.as_str()?.to_owned(),
                in_progress: status == "in_progress",
            })
        })
        .collect();
    if items.is_empty() { None } else { Some(items) }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnfinishedItem {
    pub content: String,
    pub in_progress: bool,
}

/// 该不该在这一批工具结果后面提醒；该的话给出提醒正文。
///
/// 两个条件都要满足：计数到线，且清单确实还有没做完的。只有前者
/// （模型压根没用清单）不提 —— 要不要用清单是 prompt 的事，这里只管
/// "用了却不更新"。
pub fn reminder(calls_since_todo: usize, messages: &[Message]) -> Option<String> {
    if calls_since_todo < NUDGE_AFTER_CALLS {
        return None;
    }
    let items = unfinished(messages)?;
    let mut text = format!(
        "You have made {calls_since_todo} tool calls since you last updated the todo list, \
         and it still shows {} item(s) not completed:\n",
        items.len()
    );
    for it in &items {
        let mark = if it.in_progress {
            "in_progress"
        } else {
            "pending"
        };
        text.push_str(&format!("- [{mark}] {}\n", it.content));
    }
    text.push_str(
        "Reconcile it now: call TodoWrite with the complete list, marking what is actually \
         finished as completed and exactly one item as in_progress. Put the call in your \
         next message alongside your next tool call. If the list no longer reflects the \
         work, rewrite it. This is an automated reminder, not a user message — do not \
         mention it to the user.",
    );
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use riot_protocol::id::{MessageId, ToolUseId};
    use riot_protocol::message::MessageMeta;

    fn todo_write(todos: serde_json::Value) -> Message {
        Message::Assistant {
            id: MessageId::from_raw("m"),
            content: vec![AssistantContent::ToolUse {
                id: ToolUseId::from_raw("t"),
                name: TODO_WRITE.into(),
                input: serde_json::json!({ "todos": todos }),
            }],
            usage: None,
            meta: MessageMeta::default(),
        }
    }

    fn other_tool(name: &str) -> Message {
        Message::Assistant {
            id: MessageId::from_raw("m2"),
            content: vec![AssistantContent::ToolUse {
                id: ToolUseId::from_raw("t2"),
                name: name.into(),
                input: serde_json::json!({}),
            }],
            usage: None,
            meta: MessageMeta::default(),
        }
    }

    #[test]
    fn 计数按顺序走_碰到_todo_write_归零() {
        assert_eq!(advance(0, ["Read", "Bash"]), 2);
        assert_eq!(advance(5, ["Read", TODO_WRITE]), 0);
        // TodoWrite 后面跟着的调用照样算：它们是清单没跟上的证据。
        assert_eq!(advance(5, [TODO_WRITE, "Read", "Bash"]), 2);
    }

    #[test]
    fn 取最后一次_todo_write_的未完成项() {
        let msgs = vec![
            todo_write(serde_json::json!([
                { "content": "旧的", "status": "pending", "activeForm": "x" },
            ])),
            other_tool("Read"),
            todo_write(serde_json::json!([
                { "content": "跑测试", "status": "completed", "activeForm": "x" },
                { "content": "修 bug", "status": "in_progress", "activeForm": "x" },
                { "content": "写文档", "status": "pending", "activeForm": "x" },
            ])),
            other_tool("Bash"),
        ];
        let got = unfinished(&msgs).expect("有未完成项");
        assert_eq!(
            got,
            vec![
                UnfinishedItem {
                    content: "修 bug".into(),
                    in_progress: true
                },
                UnfinishedItem {
                    content: "写文档".into(),
                    in_progress: false
                },
            ]
        );
    }

    #[test]
    fn 全部完成或没用过清单都不提醒() {
        let done = vec![todo_write(serde_json::json!([
            { "content": "a", "status": "completed", "activeForm": "x" },
        ]))];
        assert!(unfinished(&done).is_none());
        assert!(unfinished(&[other_tool("Read")]).is_none());
        assert!(reminder(100, &done).is_none());
        assert!(reminder(100, &[other_tool("Read")]).is_none());
    }

    #[test]
    fn 没到线不提醒_到线且有未完成项才提醒() {
        let msgs = vec![todo_write(serde_json::json!([
            { "content": "修 bug", "status": "in_progress", "activeForm": "x" },
        ]))];
        assert!(reminder(NUDGE_AFTER_CALLS - 1, &msgs).is_none());
        let text = reminder(NUDGE_AFTER_CALLS, &msgs).expect("该提醒");
        assert!(text.contains("修 bug"), "{text}");
        assert!(text.contains("[in_progress]"), "{text}");
        assert!(text.contains(TODO_WRITE), "得告诉它用哪个工具：{text}");
    }
}
