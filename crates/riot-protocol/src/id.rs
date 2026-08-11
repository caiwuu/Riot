//! 类型化 ID。
//!
//! 不用裸 `String` 传 ID —— 把 `ToolUseId` 传成 `MessageId` 这类错误
//! 在 TS 里要靠测试发现，这里编译器直接挡掉。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! typed_id {
    ($name:ident, $prefix:literal) => {
        #[derive(
            Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// 从已有字符串构造。反序列化与测试用。
            pub fn from_raw(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub const PREFIX: &'static str = $prefix;
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<$name> for String {
            fn from(v: $name) -> String {
                v.0
            }
        }
    };
}

typed_id!(SessionId, "ses");
typed_id!(MessageId, "msg");
typed_id!(ToolUseId, "tu");
typed_id!(RequestId, "req");
typed_id!(AgentId, "agt");
typed_id!(TurnId, "turn");

/// ID 生成器。
///
/// 抽成 trait 是为了黄金回放测试 —— 测试实现产出
/// `msg_001`、`msg_002` 这样的确定性序列。
/// 见 docs/VERIFICATION.md §4.2
pub trait IdGenerator: Send + Sync {
    fn next_id(&self, prefix: &str) -> String;

    fn session_id(&self) -> SessionId {
        SessionId(self.next_id(SessionId::PREFIX))
    }
    fn message_id(&self) -> MessageId {
        MessageId(self.next_id(MessageId::PREFIX))
    }
    fn tool_use_id(&self) -> ToolUseId {
        ToolUseId(self.next_id(ToolUseId::PREFIX))
    }
    fn request_id(&self) -> RequestId {
        RequestId(self.next_id(RequestId::PREFIX))
    }
    fn agent_id(&self) -> AgentId {
        AgentId(self.next_id(AgentId::PREFIX))
    }
    fn turn_id(&self) -> TurnId {
        TurnId(self.next_id(TurnId::PREFIX))
    }
}

/// 生产实现。
pub struct NanoIdGenerator;

impl IdGenerator for NanoIdGenerator {
    fn next_id(&self, prefix: &str) -> String {
        format!("{prefix}_{}", nanoid::nanoid!(12))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_transparent_in_json() {
        let id = MessageId::from_raw("msg_abc");
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"msg_abc\"");
    }

    #[test]
    fn generator_uses_prefix() {
        let g = NanoIdGenerator;
        assert!(g.message_id().as_str().starts_with("msg_"));
        assert!(g.tool_use_id().as_str().starts_with("tu_"));
    }
}
