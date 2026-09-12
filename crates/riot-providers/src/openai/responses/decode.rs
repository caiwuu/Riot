//! Responses 流式事件 → [`ProviderEvent`]。
//!
//! 认 JSON 里的 `type`，不依赖 SSE 的 `event:` 行 —— 有些网关会漏。
//! 参数只在 `finish` 时 parse 一次，每个 delta 都 parse 是 O(n²)。

use std::collections::HashMap;

use async_stream::stream;
use futures::StreamExt;
use riot_protocol::event::StreamDelta;
use riot_protocol::id::{MessageId, ToolUseId};
use riot_protocol::message::{AssistantContent, Message, MessageMeta, Usage};
use riot_protocol::provider::{ProviderError, ProviderEvent};

use super::wire::{
    ReasoningSig, WireEvent, WireOutputItem, WireResponse, WireStreamError, WireUsage,
};
use crate::sse::SseParser;
use crate::transport::ByteStream;

/// 一条响应里最多认多少个并行工具调用。理由同 Chat Completions 解码器。
const MAX_TOOL_CALLS: usize = 1024;

#[derive(Debug, Default, Clone)]
struct ToolAcc {
    call_id: String,
    name: String,
    args: String,
    started: bool,
}

#[derive(Debug, Default)]
pub struct StreamDecoder {
    message_id: Option<MessageId>,
    model: Option<String>,
    text: String,
    thinking: String,
    reasoning_id: Option<String>,
    encrypted_content: Option<String>,
    tools: Vec<Option<ToolAcc>>,
    item_to_index: HashMap<String, usize>,
    usage: Usage,
    finished: bool,
    error: Option<String>,
    output_limit: bool,
    completed_output: Option<Vec<WireOutputItem>>,
}

impl StreamDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, sse: &crate::sse::SseEvent) -> Vec<ProviderEvent> {
        let data = sse.data.trim();
        if data.is_empty() || data == "[DONE]" {
            return Vec::new();
        }

        let ev: WireEvent = match serde_json::from_str(data) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(error = %e, raw = %truncate(data), "跳过无法解析的 Responses 事件");
                return Vec::new();
            }
        };

        // 出错事件有两种形态（见 `WireEvent`）：嵌套的 `error` 对象，或者
        // 文档写的扁平 `code` / `message`。任一命中都算出错。
        if let Some(err) = ev.error {
            self.note_error(err);
            return Vec::new();
        }
        if ev.kind == "error" {
            self.note_error(WireStreamError {
                message: ev.message.unwrap_or_default(),
                kind: ev.code,
            });
            return Vec::new();
        }

        if let Some(resp) = ev.response {
            self.ingest_response(&resp);
        }

        let msg_id = self
            .message_id
            .get_or_insert_with(|| MessageId::from_raw("stream"))
            .clone();

        match ev.kind.as_str() {
            "response.output_text.delta" => self.text_delta(msg_id, ev.delta),
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                self.thinking_delta(msg_id, ev.delta)
            }
            "response.function_call_arguments.delta" => {
                self.tool_args_delta(ev.item_id.as_deref(), ev.output_index, ev.delta)
            }
            "response.output_item.added" | "response.output_item.done" => {
                if let Some(item) = ev.item {
                    self.output_item(ev.kind.as_str(), ev.output_index, item)
                } else {
                    Vec::new()
                }
            }
            // `response.failed` 的错误在 `response.error` 里，上面 ingest_response
            // 已经记下；`error` 在更上面拦掉了。
            _ => Vec::new(),
        }
    }

    pub fn finish(&mut self) -> Vec<ProviderEvent> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;

        if let Some(msg) = self.error.take() {
            return vec![ProviderEvent::Error(crate::errors::refused_in_stream(&msg))];
        }

        let mut out = Vec::new();
        let content = if let Some(items) = self.completed_output.take() {
            self.content_from_output(items)
        } else {
            self.content_from_acc()
        };

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

        if self.output_limit {
            out.push(ProviderEvent::Error(ProviderError::OutputLimit));
        }

        out
    }

    fn ingest_response(&mut self, resp: &WireResponse) {
        if self.message_id.is_none()
            && let Some(id) = resp.id.as_deref()
        {
            self.message_id = Some(MessageId::from_raw(id));
        }
        if self.model.is_none() {
            self.model = resp.model.clone();
        }
        if let Some(err) = &resp.error {
            self.note_error(WireStreamError {
                message: err.message.clone(),
                kind: err.kind.clone(),
            });
        }
        if let Some(u) = &resp.usage {
            self.merge_usage(u);
        }
        if resp
            .incomplete_details
            .as_ref()
            .and_then(|d| d.reason.as_deref())
            == Some("max_output_tokens")
        {
            self.output_limit = true;
        }
        if !resp.output.is_empty() {
            self.completed_output = Some(resp.output.clone());
            for item in &resp.output {
                if item.kind == "reasoning" {
                    if let Some(id) = &item.id {
                        self.reasoning_id = Some(id.clone());
                    }
                    if let Some(enc) = &item.encrypted_content {
                        self.encrypted_content = Some(enc.clone());
                    }
                }
            }
        }
    }

    fn text_delta(&mut self, msg_id: MessageId, delta: Option<String>) -> Vec<ProviderEvent> {
        let Some(t) = delta.filter(|s| !s.is_empty()) else {
            return Vec::new();
        };
        self.text.push_str(&t);
        vec![ProviderEvent::Delta(StreamDelta::Text {
            message_id: msg_id,
            text: t,
        })]
    }

    fn thinking_delta(&mut self, msg_id: MessageId, delta: Option<String>) -> Vec<ProviderEvent> {
        let Some(t) = delta.filter(|s| !s.is_empty()) else {
            return Vec::new();
        };
        self.thinking.push_str(&t);
        vec![ProviderEvent::Delta(StreamDelta::Thinking {
            message_id: msg_id,
            text: t,
        })]
    }

    fn output_item(
        &mut self,
        kind: &str,
        index: Option<usize>,
        item: WireOutputItem,
    ) -> Vec<ProviderEvent> {
        match item.kind.as_str() {
            "function_call" => self.function_call_item(index, item),
            "reasoning" => {
                if let Some(id) = item.id {
                    self.reasoning_id = Some(id);
                }
                if let Some(enc) = item.encrypted_content {
                    self.encrypted_content = Some(enc);
                }
                if kind == "response.output_item.done" && self.thinking.is_empty() {
                    for part in item.summary {
                        if let Some(t) = part.text {
                            self.thinking.push_str(&t);
                        }
                    }
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn function_call_item(
        &mut self,
        index: Option<usize>,
        item: WireOutputItem,
    ) -> Vec<ProviderEvent> {
        let idx = match self.slot_for(item.id.as_deref(), index) {
            Some(i) => i,
            None => return Vec::new(),
        };
        let slot = self.tools[idx].get_or_insert_with(ToolAcc::default);
        if let Some(id) = item.call_id {
            slot.call_id = id;
        }
        if let Some(name) = item.name {
            slot.name = name;
        }
        if let Some(args) = item.arguments
            && !args.is_empty()
        {
            slot.args = args;
        }
        self.maybe_start(idx)
    }

    fn tool_args_delta(
        &mut self,
        item_id: Option<&str>,
        index: Option<usize>,
        delta: Option<String>,
    ) -> Vec<ProviderEvent> {
        let idx = match self.slot_for(item_id, index) {
            Some(i) => i,
            None => return Vec::new(),
        };
        let mut out = self.maybe_start(idx);
        if let Some(a) = delta.filter(|s| !s.is_empty()) {
            let slot = self.tools[idx].get_or_insert_with(ToolAcc::default);
            slot.args.push_str(&a);
            if !slot.call_id.is_empty() {
                out.push(ProviderEvent::Delta(StreamDelta::ToolInput {
                    tool_use_id: ToolUseId::from_raw(slot.call_id.clone()),
                    partial_json: a,
                }));
            }
        }
        out
    }

    fn slot_for(&mut self, item_id: Option<&str>, index: Option<usize>) -> Option<usize> {
        if let Some(id) = item_id
            && let Some(&idx) = self.item_to_index.get(id)
        {
            return Some(idx);
        }
        let idx = index.unwrap_or(self.tools.len());
        if idx >= MAX_TOOL_CALLS {
            tracing::warn!(index = idx, "工具调用 index 超出上限，丢弃这一帧");
            return None;
        }
        if self.tools.len() <= idx {
            self.tools.resize(idx + 1, None);
        }
        if let Some(id) = item_id {
            self.item_to_index.insert(id.to_owned(), idx);
        }
        Some(idx)
    }

    fn maybe_start(&mut self, idx: usize) -> Vec<ProviderEvent> {
        let Some(slot) = self.tools.get_mut(idx).and_then(|s| s.as_mut()) else {
            return Vec::new();
        };
        if slot.started || slot.call_id.is_empty() || slot.name.is_empty() {
            return Vec::new();
        }
        slot.started = true;
        vec![ProviderEvent::Delta(StreamDelta::ToolStart {
            tool_use_id: ToolUseId::from_raw(slot.call_id.clone()),
            name: slot.name.clone(),
        })]
    }

    fn content_from_output(&self, items: Vec<WireOutputItem>) -> Vec<AssistantContent> {
        let mut content = Vec::new();
        for item in items {
            match item.kind.as_str() {
                "reasoning" => {
                    let text = item
                        .summary
                        .iter()
                        .filter_map(|p| p.text.as_deref())
                        .collect::<Vec<_>>()
                        .join("");
                    let text = if text.is_empty() {
                        self.thinking.clone()
                    } else {
                        text
                    };
                    let enc = item
                        .encrypted_content
                        .clone()
                        .or_else(|| self.encrypted_content.clone());
                    let signature = enc.map(|encrypted_content| {
                        serde_json::to_string(&ReasoningSig {
                            id: item.id.clone().or_else(|| self.reasoning_id.clone()),
                            encrypted_content,
                        })
                        .unwrap_or_default()
                    });
                    if !text.is_empty() || signature.is_some() {
                        content.push(AssistantContent::Thinking { text, signature });
                    }
                }
                "message" => {
                    let text = item
                        .content
                        .iter()
                        .filter(|p| p.kind == "output_text")
                        .filter_map(|p| p.text.as_deref())
                        .collect::<Vec<_>>()
                        .join("");
                    if !text.is_empty() {
                        content.push(AssistantContent::Text { text });
                    }
                }
                "function_call" => {
                    let Some(call_id) = item.call_id.filter(|s| !s.is_empty()) else {
                        continue;
                    };
                    let Some(name) = item.name.filter(|s| !s.is_empty()) else {
                        continue;
                    };
                    content.push(tool_use(
                        call_id,
                        name,
                        item.arguments.as_deref().unwrap_or(""),
                    ));
                }
                _ => {}
            }
        }
        content
    }

    fn content_from_acc(&mut self) -> Vec<AssistantContent> {
        let mut content = Vec::new();
        let signature = self.encrypted_content.as_ref().map(|enc| {
            serde_json::to_string(&ReasoningSig {
                id: self.reasoning_id.clone(),
                encrypted_content: enc.clone(),
            })
            .unwrap_or_default()
        });
        if !self.thinking.is_empty() || signature.is_some() {
            content.push(AssistantContent::Thinking {
                text: std::mem::take(&mut self.thinking),
                signature,
            });
        }
        if !self.text.is_empty() {
            content.push(AssistantContent::Text {
                text: std::mem::take(&mut self.text),
            });
        }
        for t in self.tools.iter().flatten() {
            if t.call_id.is_empty() || t.name.is_empty() {
                tracing::warn!(id = %t.call_id, name = %t.name, "工具调用信息不完整，丢弃");
                continue;
            }
            content.push(tool_use(t.call_id.clone(), t.name.clone(), &t.args));
        }
        content
    }

    fn merge_usage(&mut self, u: &WireUsage) {
        let cached = u
            .input_tokens_details
            .as_ref()
            .map(|d| d.cached_tokens)
            .unwrap_or(0);
        if u.input_tokens > 0 {
            // Responses 的 input_tokens 是总输入（含缓存），和 Chat Completions
            // 的 prompt_tokens 一样。统一扣掉缓存，对齐 Anthropic 语义。
            self.usage.input_tokens = u.input_tokens.saturating_sub(cached);
        }
        if u.output_tokens > 0 {
            self.usage.output_tokens = u.output_tokens;
        }
        if cached > 0 {
            self.usage.cache_read_tokens = cached;
        }
    }

    fn note_error(&mut self, err: WireStreamError) {
        self.error = Some(if err.message.is_empty() {
            err.kind
                .unwrap_or_else(|| "server returned an error".into())
        } else {
            err.message
        });
    }
}

fn tool_use(call_id: String, name: String, args: &str) -> AssistantContent {
    let raw = if args.trim().is_empty() { "{}" } else { args };
    let input = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "工具参数不是合法 JSON");
            serde_json::json!({ "__parse_error": raw })
        }
    };
    AssistantContent::ToolUse {
        id: ToolUseId::from_raw(call_id),
        name,
        input,
    }
}

pub fn decode_stream(
    mut bytes: ByteStream,
) -> impl futures_core::Stream<Item = ProviderEvent> + Send {
    stream! {
        let mut parser = SseParser::new();
        let mut decoder = StreamDecoder::new();

        while let Some(chunk) = bytes.next().await {
            match chunk {
                Ok(bytes) => {
                    let events = match parser.push(&bytes) {
                        Ok(evs) => evs,
                        Err(e) => {
                            for ev in decoder.finish() {
                                yield ev;
                            }
                            yield ProviderEvent::Error(crate::errors::stream_broken(e));
                            return;
                        }
                    };
                    for sse in events {
                        for ev in decoder.push(&sse) {
                            yield ev;
                        }
                    }
                }
                Err(e) => {
                    for ev in decoder.finish() {
                        yield ev;
                    }
                    yield ProviderEvent::Error(crate::errors::stream_broken(format!(
                        "failed to read response stream: {e}"
                    )));
                    return;
                }
            }
        }

        if let Some(sse) = parser.finish() {
            for ev in decoder.push(&sse) {
                yield ev;
            }
        }
        for ev in decoder.finish() {
            yield ev;
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
