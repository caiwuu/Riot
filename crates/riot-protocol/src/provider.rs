//! 模型适配层的契约。
//!
//! 这里只定义接口，实现在 `riot-providers`。放在 protocol 是因为
//! 黄金回放要用一个从磁盘读 SSE 的假 Provider 替换真实现，
//! 而 core 只能依赖 protocol。
//!
//! 见 ARCHITECTURE.md §11

use std::pin::Pin;

use async_trait::async_trait;
use futures_core::Stream;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::event::StreamDelta;
use crate::message::{Message, Usage};

pub type ProviderStream = Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>;

#[async_trait]
pub trait Provider: Send + Sync {
    /// 发起一次请求并返回事件流。
    ///
    /// 重试与降级在**实现内部**完成，对主循环不可见 —— 主循环只关心
    /// "这次调用最终成功了还是失败了"。把重试暴露出去会让主循环同时管
    /// 两套恢复逻辑，那是 bug 温床。
    fn stream(&self, req: ProviderRequest, cancel: CancellationToken) -> ProviderStream;

    /// 估算消息序列的 token 数。上下文管理层用它决定何时压缩。
    ///
    /// `[约束]` 算的必须是**发出去的那份**，不是历史本身。两者差得远:历史
    /// 里有一大堆按设计不进请求的东西 —— `System` 消息、思考内容（DeepSeek
    /// 带上会 400）、消息 id 和 usage、以及视觉兼容路径下那张只给界面看的
    /// 图（模型收到的是文字转述，见 [`crate::message::ToolResultContent`]）。
    /// 按历史算的话，几张截图就能凭空变出几万个 token —— 实测有会话报到
    /// 十万的时候，其中四成多是根本不会发出去的图片 base64。代价是用户在
    /// 实际只用掉一半窗口的时候就被压一次，而每次压缩都是一次有损的历史
    /// 改写加一次真实的模型调用。
    fn count_tokens(&self, messages: &[Message]) -> u32;
}

/// 一个 token 折多少字节。
///
/// 4 字节对英文偏准，对中文偏保守（3 字节一个汉字，实际约 1.5 字符/token，
/// 所以这么算高估一成左右）。保守是对的方向:低估会让压缩来得太晚，
/// 然后撞上真正的溢出。
const BYTES_PER_TOKEN: usize = 4;

/// 字节数 → token 估算。
///
/// `[约束]` 所有需要这个换算的地方都必须走这里。散在各处的 `/ 4` 会漂移，
/// 而漂移的表现是"压缩后仍然超预算"这类判断时对时错 —— 见
/// [`Provider::count_tokens`] 与压缩器里对 `after` 的推算。
#[must_use]
pub const fn estimate_tokens(bytes: usize) -> u32 {
    // 饱和转换:u32 装不下的字节数在这里没有意义，夹住比回绕安全。
    let t = bytes / BYTES_PER_TOKEN;
    if t > u32::MAX as usize {
        u32::MAX
    } else {
        t as u32
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderRequest {
    pub model: String,
    /// 只包含 `goes_to_model() == true` 的消息。由 INV-7 断言。
    pub messages: Vec<Message>,
    pub system: String,
    pub tools: Vec<ToolSpec>,
    /// None = 用模型默认值。输出上限恢复时会被调低。
    pub max_output_tokens: Option<u32>,
    pub thinking: ThinkingConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// 单次请求的思考配置。由 [`ThinkingPolicy`] 按轮次解析而来。
///
/// `Off` 和 `Disabled` 是两回事：GLM / DeepSeek 这类模型**默认开着**思考，
/// `Off`（不发参数）意味着"随它去"，`Disabled` 才是真的关。合成一个的话，
/// 用户就失去了"简单续轮别思考"这个最大的提速手段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ThinkingConfig {
    /// 不发任何思考参数，用端点默认行为。
    #[default]
    Off,
    /// 显式关闭思考。
    ///
    /// `[约束]` 部分端点不支持（GLM-5.3 收到 disabled 会 400、OpenAI 官方
    /// 不认识 `thinking` 字段）—— 所以它只能来自用户的显式选择，不能当默认。
    Disabled,
    /// 力度档位。各协议映射到自家参数（OpenAI 系 `reasoning_effort`、
    /// Anthropic 折算成 `budget_tokens`）。
    Effort { level: ThinkingEffort },
    /// 固定预算。Anthropic 原生；OpenAI 兼容侧折算成最近的档位。
    Budget { tokens: u32 },
}

/// 思考力度档。取值刻意与 OpenAI 的 `reasoning_effort` 对齐 ——
/// low/medium/high 是各家（OpenAI / DeepSeek / GLM）都接受的交集，
/// DeepSeek 和 GLM 会把 medium 兼容映射到 high。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingEffort {
    Low,
    Medium,
    High,
}

impl ThinkingEffort {
    /// OpenAI 系 `reasoning_effort` 的字面值。
    pub fn as_openai_str(self) -> &'static str {
        match self {
            ThinkingEffort::Low => "low",
            ThinkingEffort::Medium => "medium",
            ThinkingEffort::High => "high",
        }
    }
}

/// 会话级的思考策略。宿主存这个，主循环每次请求时解析成 [`ThinkingConfig`]。
///
/// 策略和配置分开是因为 `Adaptive` 需要**每请求**决策（首请求 vs 工具续轮），
/// 而会话设置只在轮子开始前读一次 —— 合成一个类型的话，自适应就只能整轮同档。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ThinkingPolicy {
    /// 不干预，端点默认行为。这是默认值 —— 升级后老会话行为不变。
    #[default]
    Default,
    /// 自适应：首请求中档（那一次要理解任务、定计划），工具续轮低档
    /// （多数续轮只是"读结果、发起下一步"，全力思考纯属烧钱烧时间）。
    Adaptive,
    /// 每次请求都显式关闭思考。
    Disabled,
    /// 每次请求都用固定档位。
    Fixed { level: ThinkingEffort },
}

impl ThinkingPolicy {
    /// 解析本次请求的思考配置。`turn` 是主循环的请求序号（每个用户输入
    /// 从 0 开始，工具续轮递增）。
    pub fn config_for(self, turn: u32) -> ThinkingConfig {
        match self {
            ThinkingPolicy::Default => ThinkingConfig::Off,
            ThinkingPolicy::Adaptive => ThinkingConfig::Effort {
                level: if turn == 0 {
                    ThinkingEffort::Medium
                } else {
                    ThinkingEffort::Low
                },
            },
            ThinkingPolicy::Disabled => ThinkingConfig::Disabled,
            ThinkingPolicy::Fixed { level } => ThinkingConfig::Effort { level },
        }
    }

    /// 持久化时跳过默认值用。
    pub fn is_default(&self) -> bool {
        *self == ThinkingPolicy::Default
    }
}

/// Provider 流里的一个事件。
///
/// 可序列化是为了黄金回放：用例把模型响应存成 JSON，测试时原样喂回主循环。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ProviderEvent {
    Delta(StreamDelta),
    /// 一条完整的助手消息。
    Message(Message),
    /// 用量更新。累计值，用 [`Usage::merge`] 合并。
    Usage(Usage),
    /// 出错。**流在此结束**，不会再有后续事件。
    Error(ProviderError),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderError {
    /// 上下文溢出。可恢复：压缩后重试。
    #[error("上下文溢出：用了 {used}，上限 {limit}")]
    ContextOverflow { used: u32, limit: u32 },

    /// 输出 token 耗尽。可恢复：调低 max_output_tokens 后重试。
    #[error("输出 token 耗尽")]
    OutputLimit,

    /// 附件过大。可恢复：剥离媒体后重试。
    #[error("媒体过大：{bytes} 字节")]
    MediaTooLarge { bytes: u64 },

    /// 重试耗尽。不可恢复 —— provider 内部已经试过了。
    #[error("重试耗尽：{message}")]
    RetriesExhausted { message: String },

    #[error("认证失败：{message}")]
    Auth { message: String },

    #[error("传输错误：{message}")]
    Transport { message: String },

    /// 模型拒绝服务（内容策略等）。
    #[error("请求被拒绝：{message}")]
    Refused { message: String },
}

impl ProviderError {
    /// 是否值得主循环尝试恢复。
    ///
    /// `[约束]` 这个判断决定了错误走扣留路径还是直接终止。判错的后果：
    /// 把不可恢复的判成可恢复会导致无谓重试（认证失败重试一百次也不会成功），
    /// 反过来会让本可自愈的上下文溢出直接终止会话。
    ///
    /// 注意 `RetriesExhausted` **不可恢复** —— provider 内部已经退避重试过了，
    /// 主循环再来一遍只是把同样的失败再走一遍。
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            ProviderError::ContextOverflow { .. }
                | ProviderError::OutputLimit
                | ProviderError::MediaTooLarge { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 自适应策略的核心：首请求和工具续轮的力度必须不同。
    ///
    /// 一样的话它就退化成 Fixed，而这个策略存在的唯一理由就是
    /// "工具循环里的续轮不值得全力思考"。
    #[test]
    fn 自适应首请求中档续轮低档() {
        assert_eq!(
            ThinkingPolicy::Adaptive.config_for(0),
            ThinkingConfig::Effort {
                level: ThinkingEffort::Medium
            }
        );
        assert_eq!(
            ThinkingPolicy::Adaptive.config_for(1),
            ThinkingConfig::Effort {
                level: ThinkingEffort::Low
            }
        );
        assert_eq!(
            ThinkingPolicy::Adaptive.config_for(7),
            ThinkingConfig::Effort {
                level: ThinkingEffort::Low
            }
        );
    }

    /// Default 必须解析成 Off（一个参数都不发）。
    ///
    /// 发任何参数都可能被不支持的端点 400 —— 默认值的底线是升级后
    /// 老用户的请求和之前逐字节相同。
    #[test]
    fn 默认策略不发任何思考参数() {
        assert_eq!(ThinkingPolicy::Default.config_for(0), ThinkingConfig::Off);
        assert!(ThinkingPolicy::Default.is_default());
        assert!(!ThinkingPolicy::Adaptive.is_default());
    }

    #[test]
    fn 只有三类错误值得主循环恢复() {
        assert!(ProviderError::ContextOverflow { used: 1, limit: 1 }.is_recoverable());
        assert!(ProviderError::OutputLimit.is_recoverable());
        assert!(ProviderError::MediaTooLarge { bytes: 1 }.is_recoverable());

        assert!(
            !ProviderError::RetriesExhausted {
                message: "502".into()
            }
            .is_recoverable(),
            "provider 内部已经退避重试过，主循环再试一遍只是重复同样的失败"
        );
        assert!(
            !ProviderError::Auth {
                message: "401".into()
            }
            .is_recoverable(),
            "认证失败重试一百次也不会成功"
        );
    }
}
