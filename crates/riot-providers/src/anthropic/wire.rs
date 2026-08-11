//! Anthropic Messages API 的线上格式。
//!
//! 只定义我们实际会读的字段。**不加 `deny_unknown_fields`** ——
//! 服务端加字段是常态，拒绝未知字段会让某天的一次后端发布把客户端打死。
//!
//! 这些类型是内部实现细节，不导出给 protocol。协议层的
//! [`riot_protocol::message::Message`] 才是规范格式。

use serde::Deserialize;

/// SSE `data:` 里的一个事件。
///
/// `[约束]` 必须有 `#[serde(other)]` 兜底。Anthropic 加过新事件类型
/// （`content_block_start` 的 thinking 变体就是后来加的），
/// 没有兜底分支的话，反序列化失败会让整条流断掉，而实际上只是多了个
/// 我们不关心的事件。
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireEvent {
    MessageStart {
        message: WireMessageStart,
    },
    ContentBlockStart {
        index: usize,
        content_block: WireBlockStart,
    },
    ContentBlockDelta {
        index: usize,
        delta: WireDelta,
    },
    ContentBlockStop {
        index: usize,
    },
    MessageDelta {
        delta: WireMessageDelta,
        #[serde(default)]
        usage: Option<WireUsage>,
    },
    MessageStop,
    Ping,
    Error {
        error: WireError,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WireMessageStart {
    pub id: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub usage: Option<WireUsage>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireBlockStart {
    Text {
        #[serde(default)]
        text: String,
    },
    Thinking {
        #[serde(default)]
        thinking: String,
    },
    RedactedThinking {
        #[serde(default)]
        data: String,
    },
    ToolUse {
        id: String,
        name: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireDelta {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        thinking: String,
    },
    /// 签名是**一次性**给的，不是增量拼接的（尽管名字叫 delta）。
    SignatureDelta {
        signature: String,
    },
    /// 工具参数的 JSON 片段。
    ///
    /// `[约束]` 这里的 `partial_json` 必须原样累加成字符串，
    /// 每片都 parse 一次是 O(n²)。见 `decode::BlockAccumulator`。
    InputJsonDelta {
        partial_json: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WireMessageDelta {
    /// 记录用于遥测。
    ///
    /// `[约束]` **不得参与控制流。**循环是否继续只看有没有 tool_use 块。
    /// 这个字段实测不可靠，用它判断会导致提前退出或死循环。
    /// 见 ARCHITECTURE.md §5.2
    #[serde(default)]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub stop_sequence: Option<String>,
}

/// 流式 usage。
///
/// `[约束]` 这是**累计值不是增量**。`message_delta` 里的 input/cache 字段
/// 可能回 0，直接覆盖会抹掉 `message_start` 报的真值。
/// 合并必须走 [`riot_protocol::message::Usage::merge`]，它有 `> 0` 守卫。
///
/// 这个 bug 不会报错，只会让成本统计静默偏小。
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct WireUsage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_creation_input_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: u32,
}

impl From<WireUsage> for riot_protocol::message::Usage {
    fn from(w: WireUsage) -> Self {
        Self {
            input_tokens: w.input_tokens,
            output_tokens: w.output_tokens,
            cache_creation_tokens: w.cache_creation_input_tokens,
            cache_read_tokens: w.cache_read_input_tokens,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct WireError {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 未知事件类型不会打断整条流() {
        // Anthropic 加过新事件类型。没有 #[serde(other)] 兜底的话，
        // 一次后端发布就能把所有客户端打死。
        let ev: WireEvent =
            serde_json::from_str(r#"{"type":"some_future_event","payload":{}}"#).expect("要能兜住");
        assert!(matches!(ev, WireEvent::Unknown));
    }

    #[test]
    fn 未知字段被忽略() {
        let ev: WireEvent = serde_json::from_str(r#"{"type":"message_stop","future_field":123}"#)
            .expect("多出来的字段不该导致失败");
        assert!(matches!(ev, WireEvent::MessageStop));
    }

    #[test]
    fn 未知内容块类型不会打断() {
        let ev: WireEvent = serde_json::from_str(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"video","url":"x"}}"#,
        )
        .expect("要能兜住");
        match ev {
            WireEvent::ContentBlockStart { content_block, .. } => {
                assert!(matches!(content_block, WireBlockStart::Unknown));
            }
            other => panic!("解析成了 {other:?}"),
        }
    }

    #[test]
    fn usage_字段名映射正确() {
        let w: WireUsage = serde_json::from_str(
            r#"{"input_tokens":100,"output_tokens":20,
                "cache_creation_input_tokens":5,"cache_read_input_tokens":300}"#,
        )
        .expect("解析");
        let u: riot_protocol::message::Usage = w.into();
        assert_eq!(u.input_tokens, 100);
        assert_eq!(u.cache_read_tokens, 300, "线上字段名带 _input_，别映射错");
        assert_eq!(u.cache_creation_tokens, 5);
    }

    #[test]
    fn usage_缺字段时填零() {
        let w: WireUsage = serde_json::from_str(r#"{"output_tokens":250}"#).expect("解析");
        assert_eq!(w.output_tokens, 250);
        assert_eq!(w.input_tokens, 0, "message_delta 常见形态：只带 output");
    }
}
