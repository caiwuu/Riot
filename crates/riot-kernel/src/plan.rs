//! 规划模式的会话侧派生：这段对话**当前的计划**是哪份文件、它带哪些待办。
//!
//! 给模型看的文字在 [`crate::prompt`]（`current_plan_context`），这里只回答
//! "状态是什么"。
//!
//! # 当前计划 = 整份记录里最近一次成功的 CreatePlan
//!
//! 和前端 `src/lib/plan.ts` 的 `latestPlan` 同一个定义、同一个数据源 ——
//! 界面归档 + 活历史，也就是整份 transcript。右侧面板画的、「构建」键指的、
//! 内核注给模型的，三处永远是同一份文件。旧做法只看活历史里有没有出现过
//! CreatePlan：压缩把那次调用吞进归档之后，内核以为"没有计划"，而面板还
//! 开着那份 —— 模型重新调研再交一份，面板跳到新文件，构建键指向的也换了。
//!
//! # 文件才是计划
//!
//! 路径从结果首行取（`Plan file: …`，和前端同一条约定）；结果被轻档压缩
//! 清成占位符时按 [`plan_rel_path`] 从调用参数重新算出来 —— 写文件的那个
//! 函数是确定的，名字加调用 id 就够。内容每轮从磁盘现读：用户可能在面板外
//! 直接改过，模型手里那份（CreatePlan 的参数）已经不是现状。文件不在了
//! （用户删了）就是没有计划。
//!
//! # 待办随计划走，直到 TodoWrite 接管
//!
//! 计划的待办（CreatePlan 的 `todos`）住在那次调用的参数里 —— Riot 的待办
//! 清单没有独立状态，一直是"历史里最后一次 TodoWrite 的输入"（面板、
//! riot-core 的兜底提醒都这么认）。计划的待办是这份清单的**初稿**：构建时
//! 模型用 TodoWrite 把同一批条目落成清单、之后逐项更新状态。所以这里只在
//! **计划之后还没有任何 TodoWrite** 时带上待办：一旦模型开始用 TodoWrite
//! 跟踪，它自己最后那次调用就是权威，再把初稿摆给它只会出现两份互相矛盾
//! 的清单。前端 `planTodos` 是同一条规则的显示侧。

use std::path::Path;

use riot_protocol::message::{AssistantContent, Message, ToolResultContent, UserContent};
use riot_tools::tools::names::{CREATE_PLAN, TODO_WRITE};
use riot_tools::tools::plan::{PLAN_FILE_LINE_PREFIX, plan_rel_path, todos_of};

/// 注给模型的内容上限（字节）。计划正文本身在 CreatePlan 那头限 64 KB，
/// 这里再收一档：它每轮都跟着用户消息走，一份失控的大计划不该每轮吃掉
/// 上万 token。超过的部分截掉、注明去读文件。
const MAX_INLINE_BYTES: usize = 32 * 1024;

/// 记录里认出来的当前计划：文件在哪、带哪些待办。还没碰磁盘。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlanRef {
    /// 相对项目根，CreatePlan 结果首行里的那个。
    pub rel_path: String,
    /// 计划的待办措辞，按顺序。计划之后已经有 TodoWrite 接管清单时为空
    /// （见模块文档）；计划本来就没给待办时也为空。
    pub todos: Vec<String>,
}

/// 当前计划：路径 + 此刻磁盘上的内容 + 待办。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CurrentPlan {
    /// 相对项目根，CreatePlan 结果首行里的那个。
    pub rel_path: String,
    /// 磁盘上的现状；超过 [`MAX_INLINE_BYTES`] 截断并注明。
    pub content: String,
    /// 同 [`PlanRef::todos`]。
    pub todos: Vec<String>,
}

/// 记录里最近一次**成功**的 CreatePlan：文件路径 + 待办。
///
/// `archived` 在前（压缩边界之前的消息），`live` 在后。顺序扫一遍：记住最近
/// 一次 CreatePlan 调用，等到它的结果出现再定案 —— 调用和结果分在两条消息
/// 里，倒着找要来回配对，顺着找一趟就够。最后一次调用失败（`is_error`）
/// 算没有计划，不退回更早的那份：前端面板显示的也是最后那次调用。
pub(crate) fn latest_plan(archived: &[Message], live: &[Message]) -> Option<PlanRef> {
    // 最近一次 CreatePlan 调用（id、参数里的 name、todos），结果还没出现。
    let mut pending: Option<(String, String, Vec<String>)> = None;
    // 最近一次已配上结果的调用：Some(计划) 成功，None 失败。
    let mut latest: Option<Option<PlanRef>> = None;
    for m in archived.iter().chain(live.iter()) {
        match m {
            Message::Assistant { content, .. } => {
                for c in content {
                    let AssistantContent::ToolUse { id, name, input } = c else {
                        continue;
                    };
                    if name == CREATE_PLAN {
                        let plan_name = input
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_owned();
                        pending = Some((id.as_str().to_owned(), plan_name, todos_of(input)));
                    } else if name == TODO_WRITE
                        && let Some(Some(plan)) = latest.as_mut()
                    {
                        // 模型开始用 TodoWrite 跟踪了：它的清单才是权威。
                        plan.todos.clear();
                    }
                }
            }
            Message::User { content, .. } => {
                for c in content {
                    let UserContent::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } = c
                    else {
                        continue;
                    };
                    let Some((id, plan_name, todos)) = &pending else {
                        continue;
                    };
                    if tool_use_id.as_str() != id {
                        continue;
                    }
                    latest = Some((!is_error).then(|| {
                        PlanRef {
                            // 结果原文在就按约定的首行取；被清成占位符就重算。
                            rel_path: path_from_result(content)
                                .unwrap_or_else(|| plan_rel_path(plan_name, id)),
                            todos: todos.clone(),
                        }
                    }));
                    pending = None;
                }
            }
            Message::System { .. } => {}
        }
    }
    latest.flatten()
}

/// CreatePlan 结果首行 `Plan file: <路径>` 里的路径。
fn path_from_result(content: &ToolResultContent) -> Option<String> {
    let ToolResultContent::Text { text } = content else {
        return None;
    };
    let first = text.lines().next()?.trim();
    let rel = first.strip_prefix(PLAN_FILE_LINE_PREFIX)?.trim();
    (!rel.is_empty()).then(|| rel.to_owned())
}

/// 读回当前计划。文件不在了（用户删了）或读不出文本就是没有计划。
///
/// 豁免理由：宿主层，读的是模型自己写下、用户可能改过的计划文件；注入
/// FileSystem 抽象在这里没有意义（见 clippy.toml 的说明）。
#[allow(clippy::disallowed_methods)]
pub(crate) async fn load(cwd: &Path, plan: PlanRef) -> Option<CurrentPlan> {
    let bytes = tokio::fs::read(cwd.join(&plan.rel_path)).await.ok()?;
    let content = String::from_utf8(bytes).ok()?;
    Some(CurrentPlan {
        rel_path: plan.rel_path,
        content: cap(content),
        todos: plan.todos,
    })
}

/// 超限截断 + 尾注。截在字符边界上：中间劈开一个多字节字符 `truncate`
/// 直接 panic。
fn cap(mut content: String) -> String {
    if content.len() <= MAX_INLINE_BYTES {
        return content;
    }
    let mut cut = MAX_INLINE_BYTES;
    while !content.is_char_boundary(cut) {
        cut -= 1;
    }
    content.truncate(cut);
    content.push_str("\n\n[plan truncated here; Read the file for the rest]");
    content
}

#[cfg(test)]
mod tests {
    use riot_protocol::id::{MessageId, ToolUseId};
    use riot_protocol::message::MessageMeta;

    use super::*;

    fn tool_use(id: &str, name: &str, input: serde_json::Value) -> Message {
        Message::Assistant {
            id: MessageId::from_raw(format!("a-{id}")),
            content: vec![AssistantContent::ToolUse {
                id: ToolUseId::from_raw(id),
                name: name.into(),
                input,
            }],
            usage: None,
            meta: MessageMeta::default(),
        }
    }

    fn call(id: &str, name: &str) -> Message {
        tool_use(
            id,
            CREATE_PLAN,
            serde_json::json!({ "name": name, "plan": "1. 建表" }),
        )
    }

    fn call_with_todos(id: &str, name: &str, todos: &[&str]) -> Message {
        let todos: Vec<_> = todos
            .iter()
            .map(|t| serde_json::json!({ "content": t }))
            .collect();
        tool_use(
            id,
            CREATE_PLAN,
            serde_json::json!({ "name": name, "plan": "1. 建表", "todos": todos }),
        )
    }

    fn result(id: &str, content: ToolResultContent, is_error: bool) -> Message {
        Message::User {
            id: MessageId::from_raw(format!("u-{id}")),
            content: vec![UserContent::ToolResult {
                tool_use_id: ToolUseId::from_raw(id),
                content,
                is_error,
            }],
            meta: MessageMeta::default(),
        }
    }

    fn ok(id: &str, rel: &str) -> Message {
        result(
            id,
            ToolResultContent::text(format!("{PLAN_FILE_LINE_PREFIX}{rel}\nsaved.")),
            false,
        )
    }

    fn path(archived: &[Message], live: &[Message]) -> Option<String> {
        latest_plan(archived, live).map(|p| p.rel_path)
    }

    /// 路径按结果首行取；两次提交取后一次；失败的那次不算、也不退回前一份。
    #[test]
    fn 取最近一次成功提交的路径() {
        assert_eq!(path(&[], &[]), None);

        let first = [
            call("t1", "Teller"),
            ok("t1", ".riot/plans/teller-t1.plan.md"),
        ];
        assert_eq!(
            path(&[], &first).as_deref(),
            Some(".riot/plans/teller-t1.plan.md")
        );

        let second = [
            call("t2", "Teller v2"),
            ok("t2", ".riot/plans/teller-v2-t2.plan.md"),
        ];
        assert_eq!(
            path(&first, &second).as_deref(),
            Some(".riot/plans/teller-v2-t2.plan.md"),
            "第二次提交换成新文件；归档里的旧计划排在前面"
        );

        let failed = [
            call("t3", "Broken"),
            result("t3", ToolResultContent::text("写不了计划文件"), true),
        ];
        assert_eq!(
            path(&first, &failed),
            None,
            "最后一次失败 = 没有计划，和前端面板一致"
        );

        let dangling = [call("t4", "Half")];
        assert_eq!(path(&[], &dangling), None, "只有调用没有结果（半截流）不算");
    }

    /// 结果被轻档压缩清成占位符之后，路径从调用参数重算 —— 和 CreatePlan
    /// 写文件用的是同一个函数，算出来必须是同一个名字。
    #[test]
    fn 结果被清掉时从调用参数重算路径() {
        let history = [
            call("toolu_01AbC9xY", "Teller backend"),
            result("toolu_01AbC9xY", ToolResultContent::Cleared, false),
        ];
        assert_eq!(
            path(&[], &history).as_deref(),
            Some(plan_rel_path("Teller backend", "toolu_01AbC9xY").as_str())
        );
        assert_eq!(
            path(&[], &history).as_deref(),
            Some(".riot/plans/teller-backend-abc9xy.plan.md")
        );
    }

    /// 待办跟着计划走：计划之后一有 TodoWrite 就不再带（模型的清单才是
    /// 权威）；再交一份新计划又从新计划的待办开始。计划之前的 TodoWrite
    /// 不算接管。
    #[test]
    fn 待办随计划走_直到_todo_write_接管() {
        let todo = |id: &str| {
            tool_use(
                id,
                TODO_WRITE,
                serde_json::json!({ "todos": [{ "content": "建表", "status": "in_progress", "activeForm": "正在建表" }] }),
            )
        };
        let planned = [
            todo("w0"),
            call_with_todos("t1", "Teller", &["建表", "写路由"]),
            ok("t1", ".riot/plans/teller-t1.plan.md"),
        ];
        let p = latest_plan(&[], &planned).expect("有计划");
        assert_eq!(
            p.todos,
            vec!["建表", "写路由"],
            "计划之前的 TodoWrite 不算接管"
        );

        let building = [todo("w1")];
        let p = latest_plan(&planned, &building).expect("有计划");
        assert_eq!(p.rel_path, ".riot/plans/teller-t1.plan.md");
        assert!(p.todos.is_empty(), "TodoWrite 接管后不再带初稿");

        let replanned = [
            call_with_todos("t2", "Teller v2", &["改表"]),
            ok("t2", ".riot/plans/teller-v2-t2.plan.md"),
        ];
        let mut all = planned.to_vec();
        all.extend(building);
        let p = latest_plan(&all, &replanned).expect("有计划");
        assert_eq!(p.todos, vec!["改表"], "新计划从自己的待办重新开始");

        let none = [call("t3", "Bare"), ok("t3", ".riot/plans/bare-t3.plan.md")];
        assert!(latest_plan(&[], &none).expect("有计划").todos.is_empty());
    }

    /// 文件不在了就是没有计划；在的话内容按磁盘现状给，超长截断并注明。
    #[tokio::test]
    // 测试在临时目录里摆真实文件，被测的正是"从磁盘读"。
    #[allow(clippy::disallowed_methods)]
    async fn 读回磁盘上的计划_不在了就是没有() {
        let dir = tempfile::tempdir().expect("临时目录");
        let at = |rel: &str| PlanRef {
            rel_path: rel.into(),
            todos: vec!["建表".into()],
        };
        assert!(
            load(dir.path(), at(".riot/plans/gone.plan.md"))
                .await
                .is_none()
        );

        let plans = dir.path().join(".riot/plans");
        std::fs::create_dir_all(&plans).expect("建目录");
        std::fs::write(plans.join("a.plan.md"), "# A\n\n1. 用户改过的一步\n").expect("写文件");
        let p = load(dir.path(), at(".riot/plans/a.plan.md"))
            .await
            .expect("文件在就有计划");
        assert_eq!(p.rel_path, ".riot/plans/a.plan.md");
        assert!(p.content.contains("用户改过的一步"), "{}", p.content);
        assert_eq!(p.todos, vec!["建表"], "待办原样带过去");

        let huge = "计".repeat(MAX_INLINE_BYTES);
        std::fs::write(plans.join("big.plan.md"), &huge).expect("写文件");
        let big = load(dir.path(), at(".riot/plans/big.plan.md"))
            .await
            .expect("有计划");
        assert!(big.content.len() < huge.len(), "要截断");
        assert!(
            big.content.ends_with("Read the file for the rest]"),
            "截断要注明"
        );
        assert!(big.content.starts_with('计'), "截在字符边界上");
    }
}
