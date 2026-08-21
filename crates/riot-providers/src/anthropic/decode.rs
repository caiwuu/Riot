//! SSE 事件 → `ProviderEvent`。
//!
//! # 两个必须做对的地方
//!
//! **一、`partial_json` 只在 `content_block_stop` 时 parse 一次。**
//! 每个 delta 都做一次增量解析是 O(n²)，工具参数几十 KB 时很明显。
//! Agent 也根本不需要中途拿到半成品参数 —— 拿到了也不能用。
//!
//! **二、usage 是累计值。**`message_delta` 里的 input/cache 字段可能回 0，
//! 直接覆盖会抹掉 `message_start` 的真值。走 `Usage::merge` 的 `> 0` 守卫。
//! 这个 bug 不报错，只让成本统计静默偏小。
//!
//! # 一条响应产出一条 Message
//!
//! `[取舍]` Claude Code 在每个 `content_block_stop` 就吐一条消息，好处是
//! 工具能早一点开始跑。我们改成在 `message_stop` 时吐一条完整的 —— 因为
//! Anthropic 不接受连续两条 assistant 消息，按块拆分的话重放 transcript 时
//! 还得再合并回去，那个合并逻辑是纯粹的负债。
//!
//! 代价是工具执行晚了一个 RTT 的尾巴。实际影响很小：并行工具本来就要等
//! 全部 `tool_use` 到齐。
//!
//! 见 ARCHITECTURE.md §11.3

use riot_protocol::event::StreamDelta;
use riot_protocol::id::{MessageId, ToolUseId};
use riot_protocol::message::{AssistantContent, Message, MessageMeta, Usage};
use riot_protocol::provider::{ProviderError, ProviderEvent};

use super::wire::{WireBlockStart, WireDelta, WireEvent, WireUsage};
use crate::sse::SseEvent;

/// 一个内容块的累加状态。
#[derive(Debug, Clone)]
enum BlockAccumulator {
    Text {
        text: String,
    },
    Thinking {
        text: String,
        signature: Option<String>,
    },
    ToolUse {
        id: ToolUseId,
        name: String,
        /// **原始字符串**，不是解析中的 JSON。见模块文档。
        partial_json: String,
    },
    /// 服务端加密的思考块。原样带回即可，我们不解读。
    RedactedThinking {
        data: String,
    },
    /// 不认识的块类型。保留占位以免后续 index 错位。
    Unknown,
}

#[derive(Debug, Default)]
pub struct StreamDecoder {
    message_id: Option<MessageId>,
    model: Option<String>,
    /// 按 index 存。用 Vec 而不是 HashMap，因为 Anthropic 保证 index 连续递增，
    /// 而顺序正是我们需要的 —— content 的顺序决定了工具的调用顺序。
    blocks: Vec<Option<BlockAccumulator>>,
    usage: Usage,
    saw_message_start: bool,
    finished: bool,
}

impl StreamDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// 处理一个 SSE 事件。
    pub fn push(&mut self, sse: &SseEvent) -> Vec<ProviderEvent> {
        // Anthropic 的 `event:` 行和 data 里的 `type` 永远一致，
        // 以 data 为准 —— 有些代理会重写或丢掉 event 行。
        if sse.data.is_empty() {
            return Vec::new();
        }

        let wire: WireEvent = match serde_json::from_str(&sse.data) {
            Ok(w) => w,
            Err(e) => {
                // 解析不了单个事件不该炸掉整条流，但要留痕 ——
                // 静默吞掉的话，"模型少说了一句话"这种问题永远查不出来。
                tracing::warn!(error = %e, raw = %truncate(&sse.data, 200), "SSE 事件解析失败");
                return Vec::new();
            }
        };

        self.handle(wire)
    }

    fn handle(&mut self, wire: WireEvent) -> Vec<ProviderEvent> {
        match wire {
            WireEvent::MessageStart { message } => {
                self.saw_message_start = true;
                self.message_id = Some(MessageId::from_raw(message.id));
                self.model = message.model;
                if let Some(u) = message.usage {
                    self.merge_usage(u);
                }
                Vec::new()
            }

            WireEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                let acc = match content_block {
                    WireBlockStart::Text { text } => BlockAccumulator::Text { text },
                    WireBlockStart::Thinking { thinking } => BlockAccumulator::Thinking {
                        text: thinking,
                        signature: None,
                    },
                    WireBlockStart::RedactedThinking { data } => {
                        BlockAccumulator::RedactedThinking { data }
                    }
                    WireBlockStart::ToolUse { id, name } => BlockAccumulator::ToolUse {
                        id: ToolUseId::from_raw(id),
                        name,
                        partial_json: String::new(),
                    },
                    WireBlockStart::Unknown => BlockAccumulator::Unknown,
                };
                // 工具块一开头就说一声。参数（Write 的整份文件）要生成
                // 几十秒，而完整的 tool_use 到 message_stop 才给得出来 ——
                // 不在这里报，界面那几十秒里连卡片都画不出来。
                let started = match &acc {
                    BlockAccumulator::ToolUse { id, name, .. } => Some((id.clone(), name.clone())),
                    _ => None,
                };
                self.put_block(index, acc);
                match started {
                    Some((tool_use_id, name)) => {
                        vec![ProviderEvent::Delta(StreamDelta::ToolStart {
                            tool_use_id,
                            name,
                        })]
                    }
                    None => Vec::new(),
                }
            }

            WireEvent::ContentBlockDelta { index, delta } => self.apply_delta(index, delta),

            // 这里**不做** JSON 解析。等 message_stop 一起处理 ——
            // 提前 parse 只是把 O(n²) 换成 O(n×块数)，没有收益。
            WireEvent::ContentBlockStop { .. } => Vec::new(),

            WireEvent::MessageDelta { usage, .. } => {
                if let Some(u) = usage {
                    self.merge_usage(u);
                }
                Vec::new()
            }

            WireEvent::MessageStop => self.finish_message(),

            WireEvent::Error { error } => {
                self.finished = true;
                vec![ProviderEvent::Error(map_error(&error.kind, &error.message))]
            }

            WireEvent::Ping | WireEvent::Unknown => Vec::new(),
        }
    }

    fn apply_delta(&mut self, index: usize, delta: WireDelta) -> Vec<ProviderEvent> {
        let Some(msg_id) = self.message_id.clone() else {
            // 没有 message_start 就来 delta。代理乱序或丢包，忽略。
            tracing::warn!("收到 delta 但没有 message_start");
            return Vec::new();
        };
        let Some(Some(block)) = self.blocks.get_mut(index) else {
            tracing::warn!(index, "收到未知 index 的 delta");
            return Vec::new();
        };

        match (block, delta) {
            (BlockAccumulator::Text { text }, WireDelta::TextDelta { text: chunk }) => {
                text.push_str(&chunk);
                vec![ProviderEvent::Delta(StreamDelta::Text {
                    message_id: msg_id,
                    text: chunk,
                })]
            }

            (
                BlockAccumulator::Thinking { text, .. },
                WireDelta::ThinkingDelta { thinking: chunk },
            ) => {
                text.push_str(&chunk);
                vec![ProviderEvent::Delta(StreamDelta::Thinking {
                    message_id: msg_id,
                    text: chunk,
                })]
            }

            // 签名是一次性给的，不累加。名字里的 delta 是误导。
            (
                BlockAccumulator::Thinking { signature, .. },
                WireDelta::SignatureDelta { signature: s },
            ) => {
                *signature = Some(s);
                Vec::new()
            }

            (
                BlockAccumulator::ToolUse {
                    id, partial_json, ..
                },
                WireDelta::InputJsonDelta {
                    partial_json: chunk,
                },
            ) => {
                partial_json.push_str(&chunk);
                vec![ProviderEvent::Delta(StreamDelta::ToolInput {
                    tool_use_id: id.clone(),
                    partial_json: chunk,
                })]
            }

            // 块类型和 delta 类型对不上。不该发生，但代理会制造。
            (block, delta) => {
                tracing::warn!(?block, ?delta, "块类型与 delta 类型不匹配");
                Vec::new()
            }
        }
    }

    fn finish_message(&mut self) -> Vec<ProviderEvent> {
        self.finished = true;

        let Some(id) = self.message_id.clone() else {
            // 有 message_stop 却没有 message_start。中间代理制造的脏状态。
            return vec![ProviderEvent::Error(ProviderError::Transport {
                message: "流里没有 message_start".into(),
            })];
        };

        let mut content = Vec::new();
        for block in self.blocks.drain(..).flatten() {
            match block {
                BlockAccumulator::Text { text } => {
                    // 空文本块不进 transcript。模型在 tool_use 前后经常吐空块，
                    // 留着会让 UI 出现多余的空气泡。
                    if !text.is_empty() {
                        content.push(AssistantContent::Text { text });
                    }
                }
                BlockAccumulator::Thinking { text, signature } => {
                    content.push(AssistantContent::Thinking { text, signature });
                }
                BlockAccumulator::RedactedThinking { data } => {
                    // 加密块没有明文，但签名位要留着，否则重放时服务端会拒。
                    content.push(AssistantContent::Thinking {
                        text: String::new(),
                        signature: Some(data),
                    });
                }
                BlockAccumulator::ToolUse {
                    id,
                    name,
                    partial_json,
                } => {
                    // ← 整条流里唯一一次 JSON 解析
                    let input = match parse_tool_input(&partial_json) {
                        Ok(v) => v,
                        Err(e) => {
                            return vec![ProviderEvent::Error(ProviderError::Transport {
                                message: format!(
                                    "工具 {name} 的参数不是合法 JSON（{e}）。\
                                     原始内容：{}",
                                    truncate(&partial_json, 200)
                                ),
                            })];
                        }
                    };
                    content.push(AssistantContent::ToolUse { id, name, input });
                }
                BlockAccumulator::Unknown => {}
            }
        }

        // 空响应：有 message_start 但一个内容块都没有。
        // 真实网络会制造这种状态（网关截断、上游超时）。
        // 交给主循环当成"没有 tool_use"处理即可，它会正常收场。
        vec![
            ProviderEvent::Message(Message::Assistant {
                id,
                content,
                usage: Some(self.usage),
                meta: MessageMeta {
                    model_origin: self.model.clone(),
                    ..Default::default()
                },
            }),
            ProviderEvent::Usage(self.usage),
        ]
    }

    /// 流结束时调用，处理没有 `message_stop` 的情况。
    ///
    /// 网关截断、上游超时都会这样。不处理的话，主循环会拿到一个
    /// 既没有消息也没有错误的空流，然后当成"模型什么都没说"正常结束 ——
    /// 用户看到的是 agent 莫名其妙停下来了。
    pub fn finish(&mut self) -> Vec<ProviderEvent> {
        if self.finished {
            return Vec::new();
        }
        if !self.saw_message_start {
            return vec![ProviderEvent::Error(ProviderError::Transport {
                message: "流在收到任何数据前就结束了".into(),
            })];
        }
        // 有内容但没收到 message_stop：把已有的吐出去，同时报错。
        // 半条消息比没有消息有用 —— 至少用户能看到模型说到哪了。
        let mut out = self.finish_message();
        out.push(ProviderEvent::Error(ProviderError::Transport {
            message: "流被截断：没有收到 message_stop".into(),
        }));
        out
    }

    fn put_block(&mut self, index: usize, acc: BlockAccumulator) {
        if self.blocks.len() <= index {
            self.blocks.resize_with(index + 1, || None);
        }
        self.blocks[index] = Some(acc);
    }

    fn merge_usage(&mut self, w: WireUsage) {
        // 必须走 merge 的 > 0 守卫，不能直接赋值。
        self.usage.merge(&w.into());
    }
}

/// 空参数的工具会给空字符串而不是 `{}`。
///
/// 这不是边缘情况 —— 无参工具（比如列出 todo）每次调用都会走这里。
/// 漏掉这个判断的话，它们百分之百解析失败。
fn parse_tool_input(raw: &str) -> Result<serde_json::Value, serde_json::Error> {
    if raw.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(raw)
}

fn map_error(kind: &str, message: &str) -> ProviderError {
    match kind {
        "overloaded_error" => ProviderError::RetriesExhausted {
            message: format!("服务过载：{message}"),
        },
        "authentication_error" | "permission_error" => ProviderError::Auth {
            message: message.to_owned(),
        },
        "invalid_request_error" if message.contains("context limit") => {
            match crate::retry::parse_context_overflow(message) {
                Some(o) => ProviderError::ContextOverflow {
                    used: o.input_tokens,
                    limit: o.context_limit,
                },
                None => ProviderError::Transport {
                    message: message.to_owned(),
                },
            }
        }
        "rate_limit_error" => ProviderError::RetriesExhausted {
            message: format!("限流：{message}"),
        },
        _ => ProviderError::Transport {
            message: format!("{kind}: {message}"),
        },
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn sse(data: &str) -> SseEvent {
        SseEvent {
            event: None,
            data: data.into(),
        }
    }

    /// 跑一串 data 字符串，收集所有产出。
    fn run(datas: &[&str]) -> Vec<ProviderEvent> {
        let mut d = StreamDecoder::new();
        let mut out = Vec::new();
        for data in datas {
            out.extend(d.push(&sse(data)));
        }
        out.extend(d.finish());
        out
    }

    const START: &str = r#"{"type":"message_start","message":{"id":"msg_01","model":"claude-x",
        "usage":{"input_tokens":5000,"cache_read_input_tokens":12000,"output_tokens":1}}}"#;

    fn final_message(events: &[ProviderEvent]) -> &Message {
        events
            .iter()
            .find_map(|e| match e {
                ProviderEvent::Message(m) => Some(m),
                _ => None,
            })
            .expect("应该有一条 Message")
    }

    #[test]
    fn 文本流() {
        let events = run(&[
            START,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"你"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"好"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"message_stop"}"#,
        ]);

        let deltas: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, ProviderEvent::Delta(_)))
            .collect();
        assert_eq!(deltas.len(), 2, "每个 delta 都要透传给 UI");

        match final_message(&events) {
            Message::Assistant { content, .. } => {
                assert_eq!(
                    content,
                    &vec![AssistantContent::Text {
                        text: "你好".into()
                    }]
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn 工具参数只在最后解析一次() {
        let events = run(&[
            START,
            r#"{"type":"content_block_start","index":0,
                "content_block":{"type":"tool_use","id":"toolu_1","name":"Read"}}"#,
            r#"{"type":"content_block_delta","index":0,
                "delta":{"type":"input_json_delta","partial_json":"{\"pa"}}"#,
            r#"{"type":"content_block_delta","index":0,
                "delta":{"type":"input_json_delta","partial_json":"th\":\"a.rs\"}"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"message_stop"}"#,
        ]);

        // 中途的 delta 必须是原始片段，不是解析后的对象
        let tool_deltas: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                ProviderEvent::Delta(StreamDelta::ToolInput { partial_json, .. }) => {
                    Some(partial_json.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            tool_deltas,
            vec![r#"{"pa"#, r#"th":"a.rs"}"#],
            "中途片段单独都不是合法 JSON —— 这正是不能逐片解析的原因"
        );

        match final_message(&events) {
            Message::Assistant { content, .. } => match &content[0] {
                AssistantContent::ToolUse { name, input, .. } => {
                    assert_eq!(name, "Read");
                    assert_eq!(input, &serde_json::json!({ "path": "a.rs" }));
                }
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn 无参工具给的是空字符串不是空对象() {
        // 不是边缘情况：无参工具每次调用都走这里。
        // 漏掉判断的话它们 100% 解析失败。
        let events = run(&[
            START,
            r#"{"type":"content_block_start","index":0,
                "content_block":{"type":"tool_use","id":"toolu_1","name":"ListTodos"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"message_stop"}"#,
        ]);

        match final_message(&events) {
            Message::Assistant { content, .. } => match &content[0] {
                AssistantContent::ToolUse { input, .. } => {
                    assert_eq!(input, &serde_json::json!({}));
                }
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn usage_累计值不会被零抹掉() {
        let events = run(&[
            START, // input=5000, cache_read=12000
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            // message_delta 的典型形态：只带 output，其余字段是 0
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},
                "usage":{"output_tokens":250}}"#,
            r#"{"type":"message_stop"}"#,
        ]);

        let usage = events
            .iter()
            .find_map(|e| match e {
                ProviderEvent::Usage(u) => Some(*u),
                _ => None,
            })
            .expect("应该有 Usage");

        assert_eq!(usage.input_tokens, 5000, "被 message_delta 的 0 抹掉了");
        assert_eq!(usage.cache_read_tokens, 12000, "cache_read 被抹掉了");
        assert_eq!(usage.output_tokens, 250);
    }

    #[test]
    fn thinking_的签名是一次性给的不是累加的() {
        let events = run(&[
            START,
            r#"{"type":"content_block_start","index":0,
                "content_block":{"type":"thinking","thinking":""}}"#,
            r#"{"type":"content_block_delta","index":0,
                "delta":{"type":"thinking_delta","thinking":"让我想想"}}"#,
            r#"{"type":"content_block_delta","index":0,
                "delta":{"type":"signature_delta","signature":"sig_abc"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"message_stop"}"#,
        ]);

        match final_message(&events) {
            Message::Assistant { content, .. } => {
                assert_eq!(
                    content[0],
                    AssistantContent::Thinking {
                        text: "让我想想".into(),
                        signature: Some("sig_abc".into()),
                    },
                    "signature 名字里带 delta，但它不是增量"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn 多个块保持顺序() {
        // content 的顺序决定工具的调用顺序，乱了就是串台
        let events = run(&[
            START,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"先读"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"content_block_start","index":1,
                "content_block":{"type":"tool_use","id":"toolu_1","name":"Read"}}"#,
            r#"{"type":"content_block_stop","index":1}"#,
            r#"{"type":"content_block_start","index":2,
                "content_block":{"type":"tool_use","id":"toolu_2","name":"Grep"}}"#,
            r#"{"type":"content_block_stop","index":2}"#,
            r#"{"type":"message_stop"}"#,
        ]);

        match final_message(&events) {
            Message::Assistant { content, .. } => {
                assert_eq!(content.len(), 3);
                assert!(matches!(content[0], AssistantContent::Text { .. }));
                match (&content[1], &content[2]) {
                    (
                        AssistantContent::ToolUse { name: a, .. },
                        AssistantContent::ToolUse { name: b, .. },
                    ) => {
                        assert_eq!((a.as_str(), b.as_str()), ("Read", "Grep"));
                    }
                    other => panic!("{other:?}"),
                }
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn 空文本块不进_transcript() {
        // 模型在 tool_use 前后经常吐空块，留着会让 UI 出现空气泡
        let events = run(&[
            START,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"content_block_start","index":1,
                "content_block":{"type":"tool_use","id":"toolu_1","name":"Read"}}"#,
            r#"{"type":"content_block_stop","index":1}"#,
            r#"{"type":"message_stop"}"#,
        ]);

        match final_message(&events) {
            Message::Assistant { content, .. } => {
                assert_eq!(content.len(), 1, "空文本块应该被丢掉");
                assert!(matches!(content[0], AssistantContent::ToolUse { .. }));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn 流被截断时把半条消息吐出来并报错() {
        // 网关截断、上游超时。不处理的话主循环会当成"模型什么都没说"
        // 正常结束，用户看到 agent 莫名其妙停下来了。
        let events = run(&[
            START,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"说到一半"}}"#,
            // 没有 message_stop
        ]);

        match final_message(&events) {
            Message::Assistant { content, .. } => {
                assert_eq!(
                    content[0],
                    AssistantContent::Text {
                        text: "说到一半".into()
                    }
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(
            matches!(events.last(), Some(ProviderEvent::Error(_))),
            "半条消息比没有消息有用，但错误也不能吞"
        );
    }

    #[test]
    fn 完全空的流报错而不是静默结束() {
        let events = run(&[]);
        assert!(matches!(
            events.as_slice(),
            [ProviderEvent::Error(ProviderError::Transport { .. })]
        ));
    }

    #[test]
    fn 只有_message_start_的空响应算正常结束() {
        // 有 message_start 和 message_stop 但没内容。主循环会当成
        // "没有 tool_use"正常收场 —— 这是对的，不该报错。
        let events = run(&[START, r#"{"type":"message_stop"}"#]);
        match final_message(&events) {
            Message::Assistant { content, .. } => assert!(content.is_empty()),
            other => panic!("{other:?}"),
        }
        assert!(!events.iter().any(|e| matches!(e, ProviderEvent::Error(_))));
    }

    #[test]
    fn 非法_json_的工具参数变成错误而不是崩溃() {
        let events = run(&[
            START,
            r#"{"type":"content_block_start","index":0,
                "content_block":{"type":"tool_use","id":"toolu_1","name":"Edit"}}"#,
            r#"{"type":"content_block_delta","index":0,
                "delta":{"type":"input_json_delta","partial_json":"{\"path\": unquoted}"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"message_stop"}"#,
        ]);

        match events.iter().find(|e| matches!(e, ProviderEvent::Error(_))) {
            Some(ProviderEvent::Error(ProviderError::Transport { message })) => {
                assert!(message.contains("Edit"), "错误要指出是哪个工具：{message}");
            }
            other => panic!("应该报错而不是塞一个半成品 tool_use 进去：{other:?}"),
        }
    }

    #[test]
    fn 服务端错误事件被映射成协议错误() {
        let events = run(&[
            START,
            r#"{"type":"error","error":{"type":"overloaded_error","message":"服务繁忙"}}"#,
        ]);
        assert!(matches!(
            events[0],
            ProviderEvent::Error(ProviderError::RetriesExhausted { .. })
        ));
    }

    #[test]
    fn 上下文溢出错误带上实际数字() {
        let events = run(&[
            START,
            r#"{"type":"error","error":{"type":"invalid_request_error",
                "message":"input length and max_tokens exceed context limit: 188059 + 20000 > 200000"}}"#,
        ]);
        assert_eq!(
            events[0],
            ProviderEvent::Error(ProviderError::ContextOverflow {
                used: 188059,
                limit: 200000
            }),
            "带上数字主循环才能算出该压缩到多少"
        );
    }

    #[test]
    fn 单个事件解析失败不炸掉整条流() {
        let mut d = StreamDecoder::new();
        d.push(&sse(START));
        let out = d.push(&sse("这不是 JSON"));
        assert!(out.is_empty(), "跳过它，别让一个坏事件毁掉整条响应");

        let out = d.push(&sse(r#"{"type":"message_stop"}"#));
        assert!(!out.is_empty(), "后续事件仍要正常处理");
    }

    #[test]
    fn ping_不产生任何东西() {
        let mut d = StreamDecoder::new();
        d.push(&sse(START));
        assert!(d.push(&sse(r#"{"type":"ping"}"#)).is_empty());
    }
}
