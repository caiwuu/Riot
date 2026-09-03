//! 事件合批。
//!
//! LLM token 流每秒上百条，而 UI 每帧只能渲染一次。逐条过 IPC 是纯浪费 ——
//! 这一层通常能把 IPC 消息数降低一到两个数量级，收益比换传输方式还明显。
//!
//! # 什么能合、什么不能
//!
//! `[约束]` 只有**同一目标**的 `Delta` 能合并。三条边界，越过任意一条都必须 flush：
//!
//! 1. **种类不同不能合。**`Text` 和 `Thinking` 拼在一起，UI 会把思考过程
//!    渲染成正文。
//! 2. **id 不同不能合。**两条消息的文本拼起来就是串台。
//! 3. **非 Delta 事件是语义边界。**工具调用、权限询问、Done 之前必须把
//!    累积的文本吐出去，否则 UI 上会出现「工具调用出现在它前面的解释文字之前」。
//!
//! `is_durable() == false` 的事件（Delta / Progress）不进 transcript，
//! 所以这层合并不影响黄金回放 —— 回放断言看的是 Message 和 Done。
//!
//! # 子 agent 的增量也在这里合
//!
//! Task 工具把子 agent 的事件流套在 `Progress { Nested }` 里上转，其中的
//! Delta 和主 agent 的一样是逐 token 的。它们**不能**绕过这层：一个子
//! agent 写报告就是每秒几十条 IPC，三个并行就是上百条 —— 正是这层要
//! 挡的东西。合并键多带一个"套在哪个 Task 里"（[`Key::via`]），其余
//! 三条边界原样适用：不同子 agent 的文本不能拼在一起，嵌套的 Message /
//! ToolStart 同样是边界。

use std::time::Duration;

use riot_protocol::event::{AgentEvent, ProgressPayload, StreamDelta};
use riot_protocol::id::{MessageId, ToolUseId};

/// 一帧。60fps 下 UI 感知不到延迟，但 IPC 压力降一个数量级。
pub const FRAME: Duration = Duration::from_millis(16);

/// 累积目标。两个 delta 只有目标完全相同才能合并。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Text(MessageId),
    Thinking(MessageId),
    ToolInput(ToolUseId),
}

/// 合并键：目标 + 它经由哪个 Task 工具套进来的。
///
/// `via` 为 None 是主 agent 自己的增量。两个子 agent 各写各的正文时
/// 消息 id 本就不同，`via` 看似多余 —— 但它决定**吐出去时套不套壳**：
/// 少了它，子 agent 的文本会以顶层 Delta 的形态到前端，被当成主回答渲染。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    via: Option<ToolUseId>,
    target: Target,
}

impl Key {
    /// 按键还原成事件：主 agent 的是裸 Delta，子 agent 的套回 Nested。
    fn into_event(self, text: String) -> AgentEvent {
        wrap(self.via, self.target.into_delta(text))
    }
}

/// 把增量放回它来时的壳：`via` 为 None 是顶层 Delta，否则套进那个 Task
/// 工具的 Nested 进度里。剥壳在 [`Coalescer::push`]，装壳只有这一处 ——
/// 两边不对称的话，子 agent 的文本会以主回答的形态出现在界面上。
fn wrap(via: Option<ToolUseId>, delta: StreamDelta) -> AgentEvent {
    let event = AgentEvent::Delta(delta);
    match via {
        None => event,
        Some(tool_use_id) => AgentEvent::Progress {
            tool_use_id,
            payload: ProgressPayload::Nested {
                event: Box::new(event),
            },
        },
    }
}

impl Target {
    /// `None` = 这条 delta 不参与合并，见 [`Coalescer::push`]。
    fn of(delta: &StreamDelta) -> Option<Self> {
        match delta {
            StreamDelta::Text { message_id, .. } => Some(Target::Text(message_id.clone())),
            StreamDelta::Thinking { message_id, .. } => Some(Target::Thinking(message_id.clone())),
            StreamDelta::ToolInput { tool_use_id, .. } => {
                Some(Target::ToolInput(tool_use_id.clone()))
            }
            StreamDelta::ToolStart { .. } => None,
        }
    }

    fn into_delta(self, text: String) -> StreamDelta {
        match self {
            Target::Text(message_id) => StreamDelta::Text { message_id, text },
            Target::Thinking(message_id) => StreamDelta::Thinking { message_id, text },
            Target::ToolInput(tool_use_id) => StreamDelta::ToolInput {
                tool_use_id,
                partial_json: text,
            },
        }
    }
}

fn text_of(delta: &StreamDelta) -> &str {
    match delta {
        StreamDelta::Text { text, .. } | StreamDelta::Thinking { text, .. } => text,
        StreamDelta::ToolInput { partial_json, .. } => partial_json,
        // 没有可累加的正文，不参与合并。
        StreamDelta::ToolStart { .. } => "",
    }
}

/// 增量累积器。
///
/// 独立成类型是为了能不起 runtime 就测合并逻辑 —— 上面那三条边界是纯逻辑，
/// 不该只能靠跑真实 token 流来验证。
#[derive(Debug, Default)]
pub struct Coalescer {
    pending: Option<(Key, String)>,
}

impl Coalescer {
    /// 吃进一个事件，返回**必须立即发出**的事件序列。
    ///
    /// 返回两个的情况：累积中的 delta 遇到了边界，此时要先吐 delta 再吐边界事件，
    /// 顺序不能反。
    pub fn push(&mut self, event: AgentEvent) -> Vec<AgentEvent> {
        // 剥出可累加的增量：顶层 Delta，或 Task 卡片里套着的子 agent Delta。
        // 其余一律是边界。
        let (via, delta) = match event {
            AgentEvent::Delta(d) => (None, d),
            AgentEvent::Progress {
                tool_use_id,
                payload: ProgressPayload::Nested { event: inner },
            } => match *inner {
                AgentEvent::Delta(d) => (Some(tool_use_id), d),
                other => {
                    return self.boundary(AgentEvent::Progress {
                        tool_use_id,
                        payload: ProgressPayload::Nested {
                            event: Box::new(other),
                        },
                    });
                }
            },
            other => return self.boundary(other),
        };

        let Some(target) = Target::of(&delta) else {
            // 工具开始也是语义边界：它没有可累加的正文，而它的全部价值
            // 就在于"早" —— 攒一帧再发等于把刚争取到的提前量还回去。
            return self.boundary(wrap(via, delta));
        };
        let key = Key { via, target };
        match &mut self.pending {
            Some((k, buf)) if *k == key => {
                buf.push_str(text_of(&delta));
                Vec::new()
            }
            Some(_) => {
                let flushed = self.take();
                self.pending = Some((key, text_of(&delta).to_owned()));
                flushed.into_iter().collect()
            }
            None => {
                self.pending = Some((key, text_of(&delta).to_owned()));
                Vec::new()
            }
        }
    }

    /// 帧到期时调用。
    pub fn tick(&mut self) -> Option<AgentEvent> {
        self.take()
    }

    /// 边界事件：先吐累积的增量，再吐它自己。
    fn boundary(&mut self, event: AgentEvent) -> Vec<AgentEvent> {
        let mut out = Vec::with_capacity(2);
        out.extend(self.take());
        out.push(event);
        out
    }

    fn take(&mut self) -> Option<AgentEvent> {
        self.pending.take().map(|(k, text)| k.into_event(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use riot_protocol::event::TerminalReason;

    fn text(id: &str, s: &str) -> AgentEvent {
        AgentEvent::Delta(StreamDelta::Text {
            message_id: MessageId::from_raw(id),
            text: s.into(),
        })
    }

    fn thinking(id: &str, s: &str) -> AgentEvent {
        AgentEvent::Delta(StreamDelta::Thinking {
            message_id: MessageId::from_raw(id),
            text: s.into(),
        })
    }

    fn body(e: &AgentEvent) -> String {
        match e {
            AgentEvent::Delta(d) => text_of(d).to_owned(),
            other => panic!("不是 Delta: {other:?}"),
        }
    }

    #[test]
    fn 同目标的增量合并成一条() {
        let mut c = Coalescer::default();
        assert!(c.push(text("m1", "Hel")).is_empty());
        assert!(c.push(text("m1", "lo, ")).is_empty());
        assert!(c.push(text("m1", "世界")).is_empty());

        assert_eq!(
            body(&c.tick().expect("帧到期应吐出累积文本")),
            "Hello, 世界"
        );
        assert!(c.tick().is_none(), "吐过之后不该重复");
    }

    /// 工具卡片就靠这条事件提前十几秒出现在界面上。攒进缓冲等下一帧
    /// 是把提前量还回去；排在累积文本前面则会让卡片显示在解释文字上面。
    #[test]
    fn 工具开始立刻发出且排在累积文本之后() {
        let mut c = Coalescer::default();
        assert!(c.push(text("m1", "先写文件：")).is_empty());

        let start = AgentEvent::Delta(StreamDelta::ToolStart {
            tool_use_id: ToolUseId::from_raw("u1"),
            name: "Write".into(),
        });
        let out = c.push(start.clone());

        assert_eq!(out.len(), 2, "先吐累积的文本，再吐工具开始");
        assert_eq!(body(&out[0]), "先写文件：");
        assert_eq!(out[1], start);
        assert!(c.tick().is_none(), "它不该留在缓冲里等下一帧");
    }

    #[test]
    fn 思考与正文不能合并() {
        let mut c = Coalescer::default();
        c.push(thinking("m1", "让我想想"));
        let out = c.push(text("m1", "答案是 42"));

        assert_eq!(out.len(), 1, "换种类必须先 flush");
        assert_eq!(body(&out[0]), "让我想想");
        assert!(
            matches!(&out[0], AgentEvent::Delta(StreamDelta::Thinking { .. })),
            "flush 出来的必须还是 Thinking —— 混进正文会让 UI 把思考过程当答案渲染"
        );
        assert_eq!(body(&c.tick().unwrap()), "答案是 42");
    }

    #[test]
    fn 不同消息的文本不能串台() {
        let mut c = Coalescer::default();
        c.push(text("m1", "第一条"));
        let out = c.push(text("m2", "第二条"));

        assert_eq!(out.len(), 1);
        assert_eq!(body(&out[0]), "第一条");
        assert_eq!(body(&c.tick().unwrap()), "第二条");
    }

    #[test]
    fn 非增量事件是边界且排在累积文本之后() {
        let mut c = Coalescer::default();
        c.push(text("m1", "工具调用前的解释"));

        let boundary = AgentEvent::Done {
            reason: TerminalReason::Completed,
        };
        let out = c.push(boundary.clone());

        assert_eq!(out.len(), 2, "累积文本和边界事件都要发出");
        assert_eq!(body(&out[0]), "工具调用前的解释");
        assert_eq!(
            out[1], boundary,
            "边界必须在文本之后 —— 反了 UI 上就会看到结束早于内容"
        );
    }

    #[test]
    fn 工具参数增量按_tool_use_id_分组() {
        let mut c = Coalescer::default();
        let mk = |id: &str, s: &str| {
            AgentEvent::Delta(StreamDelta::ToolInput {
                tool_use_id: ToolUseId::from_raw(id),
                partial_json: s.into(),
            })
        };
        c.push(mk("u1", r#"{"path":"#));
        assert!(c.push(mk("u1", r#""a.rs"}"#)).is_empty());
        let out = c.push(mk("u2", "{"));

        assert_eq!(body(&out[0]), r#"{"path":"a.rs"}"#);
    }

    /// Task 卡片里套着的子 agent 增量。
    fn nested(task: &str, inner: AgentEvent) -> AgentEvent {
        AgentEvent::Progress {
            tool_use_id: ToolUseId::from_raw(task),
            payload: ProgressPayload::Nested {
                event: Box::new(inner),
            },
        }
    }

    /// 剥出嵌套事件里的那条；不是嵌套的就 panic —— 测试要抓的正是
    /// "子 agent 的文本以顶层 Delta 形态漏出去"。
    fn unwrap_nested<'e>(e: &'e AgentEvent, task: &str) -> &'e AgentEvent {
        match e {
            AgentEvent::Progress {
                tool_use_id,
                payload: ProgressPayload::Nested { event },
            } => {
                assert_eq!(tool_use_id.as_str(), task, "套回了别的 Task");
                event
            }
            other => panic!("该套在 Task {task} 里，实际是顶层事件: {other:?}"),
        }
    }

    /// 子 agent 写报告是逐 token 的流，和主 agent 一样得攒帧；吐出去时
    /// 必须还套在原来的 Task 里 —— 裸出去就是被当成主回答渲染。
    #[test]
    fn 子_agent_的增量攒帧且套回原来的_task() {
        let mut c = Coalescer::default();
        assert!(c.push(nested("task1", text("s1", "入口"))).is_empty());
        assert!(c.push(nested("task1", text("s1", "在 "))).is_empty());
        assert!(c.push(nested("task1", text("s1", "main.rs"))).is_empty());

        let out = c.tick().expect("帧到期该吐出累积文本");
        let inner = unwrap_nested(&out, "task1");
        assert_eq!(body(inner), "入口在 main.rs");
        assert!(c.tick().is_none());
    }

    /// 并行的两个子 agent 各写各的，不能拼到一起。
    #[test]
    fn 不同_task_里的增量不能串台() {
        let mut c = Coalescer::default();
        c.push(nested("task1", text("s1", "甲")));
        let out = c.push(nested("task2", text("s2", "乙")));

        assert_eq!(out.len(), 1, "换 Task 必须先 flush");
        assert_eq!(body(unwrap_nested(&out[0], "task1")), "甲");
        assert_eq!(body(unwrap_nested(&c.tick().unwrap(), "task2")), "乙");
    }

    /// 主 agent 的正文和子 agent 的正文即使消息 id 撞了也不能合 ——
    /// 合了要么主回答里混进子 agent 的话，要么反过来。
    #[test]
    fn 主_agent_与子_agent_的增量互为边界() {
        let mut c = Coalescer::default();
        c.push(text("m1", "主"));
        let out = c.push(nested("task1", text("m1", "子")));

        assert_eq!(out.len(), 1);
        assert!(matches!(&out[0], AgentEvent::Delta(_)), "主 agent 的要以裸 Delta 出去");
        assert_eq!(body(&out[0]), "主");
        assert_eq!(body(unwrap_nested(&c.tick().unwrap(), "task1")), "子");
    }

    /// 嵌套的工具开始 / 完整消息是边界：立发，并排在累积文本之后。
    #[test]
    fn 嵌套的边界事件立发且排在累积文本之后() {
        let mut c = Coalescer::default();
        c.push(nested("task1", text("s1", "先看看：")));

        let start = nested(
            "task1",
            AgentEvent::Delta(StreamDelta::ToolStart {
                tool_use_id: ToolUseId::from_raw("u9"),
                name: "Grep".into(),
            }),
        );
        let out = c.push(start.clone());
        assert_eq!(out.len(), 2, "先吐累积的文本，再吐工具开始");
        assert_eq!(body(unwrap_nested(&out[0], "task1")), "先看看：");
        assert_eq!(out[1], start, "嵌套的 ToolStart 要原样立发");
        assert!(c.tick().is_none());

        c.push(nested("task1", text("s2", "半句")));
        let done = nested(
            "task1",
            AgentEvent::Done {
                reason: TerminalReason::Completed,
            },
        );
        let out = c.push(done.clone());
        assert_eq!(out.len(), 2);
        assert_eq!(body(unwrap_nested(&out[0], "task1")), "半句");
        assert_eq!(out[1], done);
    }
}
