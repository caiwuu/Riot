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

use std::time::Duration;

use riot_protocol::event::{AgentEvent, StreamDelta};
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
    pending: Option<(Target, String)>,
}

impl Coalescer {
    /// 吃进一个事件，返回**必须立即发出**的事件序列。
    ///
    /// 返回两个的情况：累积中的 delta 遇到了边界，此时要先吐 delta 再吐边界事件，
    /// 顺序不能反。
    pub fn push(&mut self, event: AgentEvent) -> Vec<AgentEvent> {
        let AgentEvent::Delta(delta) = event else {
            let mut out = Vec::with_capacity(2);
            out.extend(self.take());
            out.push(event);
            return out;
        };

        let Some(target) = Target::of(&delta) else {
            // 工具开始也是语义边界：它没有可累加的正文，而它的全部价值
            // 就在于"早" —— 攒一帧再发等于把刚争取到的提前量还回去。
            let mut out = Vec::with_capacity(2);
            out.extend(self.take());
            out.push(AgentEvent::Delta(delta));
            return out;
        };
        match &mut self.pending {
            Some((t, buf)) if *t == target => {
                buf.push_str(text_of(&delta));
                Vec::new()
            }
            Some(_) => {
                let flushed = self.take();
                self.pending = Some((target, text_of(&delta).to_owned()));
                flushed.into_iter().collect()
            }
            None => {
                self.pending = Some((target, text_of(&delta).to_owned()));
                Vec::new()
            }
        }
    }

    /// 帧到期时调用。
    pub fn tick(&mut self) -> Option<AgentEvent> {
        self.take()
    }

    fn take(&mut self) -> Option<AgentEvent> {
        self.pending
            .take()
            .map(|(t, text)| AgentEvent::Delta(t.into_delta(text)))
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
}
