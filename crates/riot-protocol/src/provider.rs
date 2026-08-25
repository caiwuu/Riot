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
use crate::message::{Attachment, Message, ToolResultContent, Usage, UserContent};

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
    ///
    /// `[约束]` 但**进了请求的图片按张算，不按 base64 的字节算** —— 见
    /// [`estimate_image_tokens`] 与 [`wire_images`]。上面那条只管住了"不该
    /// 发的别算"，而真发出去的图如果跟着走字节口径，一张就够顶穿阈值。
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

/// 从哪儿开始需要粗估，以及在那之前的**真实**计数。
///
/// 返回 `(起始下标, 基准 token 数)`：下标之前的内容由基准值代表，从下标起
/// 的消息才需要按字节估。没有任何一条带 usage 时返回 `(0, 0)` —— 退化成
/// 全量粗估，也就是这个机制存在之前的行为。
///
/// 为什么要它：粗估按 4 字节/token 折算，对代码和英文偏低（实测一个真实
/// 会话里差了一成半）。偏低的表现不是"数字不好看"，而是**该压的时候没压**，
/// 然后撞上服务方的硬上限 —— 那时反应式压缩要花一次总结的钱才能救回来。
/// 服务方回报的 usage 是那一刻上下文的准确尺寸，拿它打底，误差就只剩最后
/// 几条新消息那一小段。
///
/// `[约束]` 只认主 agent（`agent_id` 为空）的消息。子 agent 有自己的上下文，
/// 它的 usage 描述的是另一个窗口的大小 —— 拿来给主历史打底会差出一整个
/// 数量级，而且是往小了差。
///
/// `[取舍]` 历史被**改写过**（轻档压缩把旧工具结果清成占位符）之后，基准值
/// 仍然是改写前那次请求的大小，于是偏大。接受这个偏差：偏大只会让压缩早来
/// 一轮，而偏小是撞上限；何况压缩之后紧接着就会发一次请求，新的 usage 立刻
/// 把基准顶掉，窗口只有一轮。方向和 [`BYTES_PER_TOKEN`] 那条注释是一致的。
#[must_use]
pub fn last_usage_checkpoint(messages: &[Message]) -> (usize, u32) {
    messages
        .iter()
        .enumerate()
        .rev()
        .find_map(|(i, m)| match m {
            Message::Assistant {
                usage: Some(u),
                meta,
                ..
            } if meta.agent_id.is_none() => Some((i + 1, u.total())),
            _ => None,
        })
        .unwrap_or((0, 0))
}

/// 一张进了请求的图片折多少 token。
///
/// `[约束]` 图片**不能**走 [`estimate_tokens`]。模型按像素给图片计费，和
/// base64 的长度没有关系，而 base64 每 3 字节原始数据要涨成 4 个字符 ——
/// 一张 100 KB 的 JPEG 按字节折算是 33,000 token，它真实只值一千多。后果
/// 不是"多留了点余量"：两三张图就能顶穿压缩阈值，于是每一轮开工前都先做
/// 一次全量总结，而那是一次真实的模型调用加一次有损的历史改写。
///
/// 取值:产出方统一把图压到 115 万像素（`riot-tools` 的 `shrink` 模块）之后，
/// Anthropic 的 `(宽×高)/750` 口径约 1533，OpenAI 的 32×32 patch 口径约 1123。
/// 取 1600 比两家都高一点 —— 和 [`BYTES_PER_TOKEN`] 一样宁可偏保守，只是
/// 这里的"保守"该按几成算，不是按几十倍。
const IMAGE_TOKENS: u32 = 1_600;

/// 图片张数 → token 估算。
#[must_use]
pub const fn estimate_image_tokens(count: u32) -> u32 {
    count.saturating_mul(IMAGE_TOKENS)
}

/// 会真发给模型的图片:张数，以及它们的 base64 在报文里占的字节数。
///
/// 两个返回值是配套用的:把 base64 的字节数从报文长度里扣掉，再按张加回
/// [`estimate_image_tokens`]。只给张数的话调用方没法扣，扣不掉就还是字节口径。
///
/// `[约束]` 判据是**协议语义**而不是线格式 —— `DescribedImage` 那张图只给
/// 界面看，两家 provider 都不会发（见 [`ToolResultContent`]）。所以这个函数
/// 放在这里给两家共用:各写一遍的话，一边改了另一边不会报错，只会表现成
/// 压缩时机变得莫名其妙，而那正是 [`Provider::count_tokens`] 上两条约束
/// 反复强调的那种偏差。
#[must_use]
pub fn wire_images(messages: &[Message]) -> (u32, usize) {
    let mut count = 0u32;
    let mut bytes = 0usize;
    for m in messages {
        // System 不进请求，Assistant 产不出图 —— 只有 user 消息带图。
        let Message::User { content, .. } = m else {
            continue;
        };
        for c in content {
            // 穷尽匹配而不是 `_`:日后新增一个带图的变体，这里必须编译不过。
            // 漏掉的表现是它的 base64 悄悄回到字节口径，没有任何报错。
            let data = match c {
                UserContent::ToolResult {
                    content:
                        ToolResultContent::Image { data, .. }
                        | ToolResultContent::MarkedImage { data, .. },
                    ..
                }
                | UserContent::Attachment(Attachment::Image { data, .. }) => data,

                // 视觉兼容路径:模型收到的是转述文字，base64 不出报文。
                UserContent::ToolResult {
                    content: ToolResultContent::DescribedImage { .. },
                    ..
                }
                | UserContent::Attachment(Attachment::DescribedImage { .. }) => continue,

                // 不带图的:正文、落盘预览、清理占位符、各类文件与提醒附件。
                UserContent::Text { .. }
                | UserContent::ToolResult {
                    content:
                        ToolResultContent::Text { .. }
                        | ToolResultContent::Spilled { .. }
                        | ToolResultContent::Cleared,
                    ..
                }
                | UserContent::Attachment(
                    Attachment::Memory { .. }
                    | Attachment::RestoredFile { .. }
                    | Attachment::UserFile { .. }
                    | Attachment::Environment { .. }
                    | Attachment::SystemReminder { .. },
                ) => continue,
            };
            count = count.saturating_add(1);
            bytes = bytes.saturating_add(data.len());
        }
    }
    (count, bytes)
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

    /// 图片按张计价的地基:只数会发出去的图。
    ///
    /// Image / MarkedImage / 用户附图要计数并累计 base64 长度（调用方靠它
    /// 从报文字节里扣掉）；转述路径（DescribedImage）的 base64 不出报文，
    /// 一张都不能算 —— 算了就回到"凭空多出几万 token"的老问题。
    #[test]
    fn wire_images_只数会发出去的图() {
        use crate::id::{MessageId, ToolUseId};
        use crate::message::MessageMeta;

        let b64 = "A".repeat(100_000);
        let tool_result = |content: ToolResultContent| UserContent::ToolResult {
            tool_use_id: ToolUseId::from_raw("t1"),
            content,
            is_error: false,
        };
        let messages = vec![Message::User {
            id: MessageId::from_raw("u1"),
            content: vec![
                tool_result(ToolResultContent::Image {
                    media_type: "image/jpeg".into(),
                    data: b64.clone(),
                    path: None,
                }),
                tool_result(ToolResultContent::MarkedImage {
                    media_type: "image/jpeg".into(),
                    data: b64.clone(),
                    path: None,
                    text: "编号清单".into(),
                }),
                UserContent::Attachment(Attachment::Image {
                    media_type: "image/png".into(),
                    data: b64.clone(),
                }),
                // 下面两个是转述路径:图只给界面，不该计。
                tool_result(ToolResultContent::DescribedImage {
                    media_type: "image/jpeg".into(),
                    data: b64.clone(),
                    path: None,
                    text: "转述".into(),
                }),
                UserContent::Attachment(Attachment::DescribedImage {
                    media_type: "image/png".into(),
                    data: b64.clone(),
                    text: "转述".into(),
                }),
            ],
            meta: MessageMeta::default(),
        }];

        assert_eq!(wire_images(&messages), (3, b64.len() * 3));
    }

    mod checkpoint {
        use super::*;
        use crate::id::MessageId;
        use crate::message::{AssistantContent, MessageMeta, Usage};

        fn user(id: &str) -> Message {
            Message::User {
                id: MessageId::from_raw(id),
                content: vec![UserContent::Text { text: "话".into() }],
                meta: MessageMeta::default(),
            }
        }

        fn assistant(id: &str, usage: Option<Usage>, agent: Option<&str>) -> Message {
            Message::Assistant {
                id: MessageId::from_raw(id),
                content: vec![AssistantContent::Text { text: "答".into() }],
                usage,
                meta: MessageMeta {
                    agent_id: agent.map(crate::id::AgentId::from_raw),
                    ..MessageMeta::default()
                },
            }
        }

        fn usage(input: u32, cache_read: u32, output: u32) -> Option<Usage> {
            Some(Usage {
                input_tokens: input,
                cache_read_tokens: cache_read,
                cache_creation_tokens: 0,
                output_tokens: output,
            })
        }

        /// 一条 usage 都没有（全新会话、或压缩把历史换成了一条总结）时
        /// 退化成全量粗估 —— 也就是这个机制存在之前的行为。
        #[test]
        fn 没有用量时退化成全量粗估() {
            assert_eq!(last_usage_checkpoint(&[]), (0, 0));
            assert_eq!(
                last_usage_checkpoint(&[user("u1"), assistant("a1", None, None)]),
                (0, 0)
            );
        }

        /// 基准取**最后一条**，而且四项都算进去：input 是那次请求发出去的
        /// 全部（含缓存命中，两家的口径已在 provider 侧统一），output 是它
        /// 写回历史的部分，下一次请求要连着一起发。
        #[test]
        fn 取最后一条并把四项都算上() {
            let msgs = vec![
                user("u1"),
                assistant("a1", usage(1_000, 0, 100), None),
                user("u2"),
                assistant("a2", usage(20_000, 80_000, 500), None),
            ];
            assert_eq!(last_usage_checkpoint(&msgs), (4, 100_500));
        }

        /// 起点是 checkpoint 的**下一条**。含进它自己的话，那条 assistant
        /// 会被算两遍（一次在 usage 的 output 里，一次在字节里）。
        #[test]
        fn 起点在基准之后() {
            let msgs = vec![
                user("u1"),
                assistant("a1", usage(5_000, 0, 200), None),
                user("u2"),
            ];
            let (from, base) = last_usage_checkpoint(&msgs);
            assert_eq!((from, base), (2, 5_200));
            assert!(matches!(&msgs[from..], [Message::User { .. }]), "只剩新来的那条");
        }

        /// `[约束]` 子 agent 的 usage 描述的是**另一个窗口**。拿它给主历史
        /// 打底会差出一个数量级，而且是往小了差 —— 主历史看起来一直很空，
        /// 压缩永远不触发，直到撞上服务方的硬上限。
        #[test]
        fn 子_agent_的用量不能给主历史打底() {
            let msgs = vec![
                user("u1"),
                assistant("a1", usage(90_000, 0, 1_000), None),
                user("u2"),
                assistant("sub", usage(300, 0, 50), Some("agent-1")),
            ];
            let (from, base) = last_usage_checkpoint(&msgs);
            assert_eq!(base, 91_000, "该用主 agent 那条，不是子 agent 的 350");
            assert_eq!(from, 2);
        }
    }

    /// 回归:字节口径下一张 200 KB 的预览图折五万 token，三四张就顶穿
    /// 100k 的默认压缩阈值，每轮开工前都白做一次全量总结。按张计价后
    /// 几十张也到不了阈值的一半。
    #[test]
    fn 图片按张计价顶不穿压缩阈值() {
        assert!(estimate_image_tokens(1) < 10_000, "单张图是千级成本");
        assert!(estimate_image_tokens(20) < 100_000 / 2);
    }
}
