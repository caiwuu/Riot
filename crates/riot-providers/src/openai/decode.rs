//! OpenAI 流式响应 → [`ProviderEvent`]。
//!
//! 和 Anthropic 那边最大的区别：**没有块边界事件**。Anthropic 有
//! `content_block_start` / `_stop` 明确告诉你一个块开始和结束了；OpenAI
//! 只有一串 delta，什么时候算完只能靠 `finish_reason` 或者流结束来判断。
//!
//! 所以这里全程累积，到最后一次性产出完整的 `Message`。工具参数同理 ——
//! 每个 delta 都尝试 parse 一次是 O(n²)，大参数下很明显。

use riot_protocol::event::StreamDelta;
use riot_protocol::id::{MessageId, ToolUseId};
use riot_protocol::message::{AssistantContent, Message, MessageMeta, Usage};
use riot_protocol::provider::{ProviderError, ProviderEvent};

use super::wire::{WireChunk, WireUsage};
use crate::sse::SseEvent;

#[derive(Debug, Default, Clone)]
struct ToolAcc {
    id: String,
    name: String,
    args: String,
}

#[derive(Debug, Default)]
pub struct StreamDecoder {
    message_id: Option<MessageId>,
    model: Option<String>,
    text: String,
    thinking: String,
    /// 按 index 存。OpenAI 用 index 关联分片，而顺序就是工具的调用顺序。
    tools: Vec<Option<ToolAcc>>,
    usage: Usage,
    finish_reason: Option<String>,
    finished: bool,
    /// 服务端在 SSE 数据里报的错。有些兼容实现这么干，而不是给个 HTTP 状态码。
    error: Option<String>,
}

impl StreamDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, sse: &SseEvent) -> Vec<ProviderEvent> {
        let data = sse.data.trim();
        if data.is_empty() {
            return Vec::new();
        }

        // 流结束标记。收到它之后不该再有内容，但收尾统一交给 finish()。
        if data == "[DONE]" {
            return Vec::new();
        }

        let chunk: WireChunk = match serde_json::from_str(data) {
            Ok(c) => c,
            Err(e) => {
                // 解析不了就跳过这一帧。整条流因为一个畸形帧就中断，
                // 比丢一帧糟糕得多 —— 前面已经产出的内容会全部作废。
                tracing::warn!(error = %e, raw = %truncate(data), "跳过无法解析的 chunk");
                return Vec::new();
            }
        };

        if let Some(err) = chunk.error {
            self.error = Some(if err.message.is_empty() {
                err.kind.unwrap_or_else(|| "服务端返回了错误".into())
            } else {
                err.message
            });
            return Vec::new();
        }

        if self.message_id.is_none()
            && let Some(id) = chunk.id
        {
            self.message_id = Some(MessageId::from_raw(id));
        }
        if self.model.is_none() {
            self.model = chunk.model;
        }
        if let Some(u) = chunk.usage {
            self.merge_usage(u);
        }

        // message_id 可能还没到（有些实现第一帧不带 id）。给一个占位的，
        // 否则这一帧的文本增量就得丢掉 —— 而它是用户最先看到的内容。
        let msg_id = self
            .message_id
            .get_or_insert_with(|| MessageId::from_raw("stream"))
            .clone();

        let mut out = Vec::new();

        for choice in chunk.choices {
            if let Some(r) = choice.finish_reason {
                self.finish_reason = Some(r);
            }

            if let Some(t) = choice.delta.reasoning_content
                && !t.is_empty()
            {
                self.thinking.push_str(&t);
                out.push(ProviderEvent::Delta(StreamDelta::Thinking {
                    message_id: msg_id.clone(),
                    text: t,
                }));
            }

            if let Some(t) = choice.delta.content
                && !t.is_empty()
            {
                self.text.push_str(&t);
                out.push(ProviderEvent::Delta(StreamDelta::Text {
                    message_id: msg_id.clone(),
                    text: t,
                }));
            }

            for tc in choice.delta.tool_calls.unwrap_or_default() {
                if self.tools.len() <= tc.index {
                    self.tools.resize(tc.index + 1, None);
                }
                let slot = self.tools[tc.index].get_or_insert_with(ToolAcc::default);

                if let Some(id) = tc.id {
                    slot.id = id;
                }
                if let Some(f) = tc.function {
                    if let Some(n) = f.name {
                        // 名字也可能是分片来的
                        slot.name.push_str(&n);
                    }
                    if let Some(a) = f.arguments
                        && !a.is_empty()
                    {
                        slot.args.push_str(&a);
                        if !slot.id.is_empty() {
                            out.push(ProviderEvent::Delta(StreamDelta::ToolInput {
                                tool_use_id: ToolUseId::from_raw(slot.id.clone()),
                                partial_json: a,
                            }));
                        }
                    }
                }
            }
        }

        out
    }

    /// 收尾：产出完整消息、用量，以及可恢复的错误。
    pub fn finish(&mut self) -> Vec<ProviderEvent> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;

        if let Some(msg) = self.error.take() {
            return vec![ProviderEvent::Error(ProviderError::Refused { message: msg })];
        }

        let mut out = Vec::new();
        let mut content: Vec<AssistantContent> = Vec::new();

        if !self.thinking.is_empty() {
            content.push(AssistantContent::Thinking {
                text: std::mem::take(&mut self.thinking),
                // OpenAI 格式没有签名机制。留 None，INV-9 的剥离逻辑
                // 对它是空操作。
                signature: None,
            });
        }
        if !self.text.is_empty() {
            content.push(AssistantContent::Text {
                text: std::mem::take(&mut self.text),
            });
        }

        for t in self.tools.iter().flatten() {
            if t.id.is_empty() || t.name.is_empty() {
                tracing::warn!(id = %t.id, name = %t.name, "工具调用信息不完整，丢弃");
                continue;
            }
            // 参数为空串是常见的（无参工具），当成 `{}`。
            let raw = if t.args.trim().is_empty() {
                "{}"
            } else {
                t.args.as_str()
            };
            let input = match serde_json::from_str(raw) {
                Ok(v) => v,
                Err(e) => {
                    // 流被截断时会出现半个 JSON。构造一个能让模型看懂的
                    // 错误参数，比丢掉整个 tool_use 好 —— 丢掉的话
                    // tool_use/tool_result 配对就断了。
                    tracing::warn!(error = %e, "工具参数不是合法 JSON");
                    serde_json::json!({ "__parse_error": raw })
                }
            };
            content.push(AssistantContent::ToolUse {
                id: ToolUseId::from_raw(t.id.clone()),
                name: t.name.clone(),
                input,
            });
        }

        if !content.is_empty() {
            out.push(ProviderEvent::Message(Message::Assistant {
                id: self
                    .message_id
                    .clone()
                    .unwrap_or_else(|| MessageId::from_raw("stream")),
                content,
                usage: (self.usage != Usage::default()).then_some(self.usage),
                meta: MessageMeta {
                    model_origin: self.model.clone(),
                    ..Default::default()
                },
            }));
        }

        if self.usage != Usage::default() {
            out.push(ProviderEvent::Usage(self.usage));
        }

        // `[约束]` 输出被 max_tokens 截断要报成可恢复错误，主循环会调低
        // max_tokens 重试。当成正常结束的话，模型的回答会缺一截而没有
        // 任何人知道。
        if self.finish_reason.as_deref() == Some("length") {
            out.push(ProviderEvent::Error(ProviderError::OutputLimit));
        }

        out
    }

    fn merge_usage(&mut self, u: WireUsage) {
        // `>0` 守卫：兼容实现常常在中间的 chunk 里发一个全零的 usage，
        // 直接覆盖会把最后那个真实的数字冲掉。
        if u.prompt_tokens > 0 {
            self.usage.input_tokens = u.prompt_tokens;
        }
        if u.completion_tokens > 0 {
            self.usage.output_tokens = u.completion_tokens;
        }
        if let Some(c) = u.prompt_cache_hit_tokens
            && c > 0
        {
            self.usage.cache_read_tokens = c;
        }
    }
}

fn truncate(s: &str) -> String {
    let max = 200;
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}
