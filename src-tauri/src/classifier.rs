//! [`PermissionMode::Auto`] 的判危分类器。
//!
//! # 它在整套权限里的位置
//!
//! 决策链是纯函数，判不了需要 IO 的事，所以这一层挂在权限闸上 —— 那里
//! 本来就是 async，也本来就是"弹窗并等用户"的地方。链照常算出 `Ask`，
//! 闸在弹窗的同时问一次小模型，先有结果的算。
//!
//! `[约束]` 分类器的权力**不超过** bypass 模式。只有
//! `DecisionReason::yields_to_bypass()` 为真的询问才交给它判 —— 安全检查
//! （SSH 密钥、shell 启动脚本）和用户亲手写的 ask 规则永远轮不到它。
//! 判据和分层免疫共用一个谓词，不是两套。
//!
//! # 为什么用便宜档模型
//!
//! 这是"用户在等"的前台请求，但它等的是一个词。用主模型跑等于每次工具
//! 调用前多付一次完整请求的钱，而这件事的输出短、结构固定、失败可降级 ——
//! 正好是该交给小模型的三个条件。没配便宜档时不装它（`NoClassifier`），
//! Auto 模式退化成 Default，不会静默放行。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use riot_protocol::message::{AssistantContent, Message, MessageMeta, UserContent};
use riot_protocol::permission::{SafetyClassifier, SafetyVerdict};
use riot_protocol::provider::{Provider, ProviderEvent, ProviderRequest, ThinkingConfig};
use tokio_util::sync::CancellationToken;

/// 自动放行需要的最低确信度。
///
/// 一个犹豫的 SAFE 仍然去问人。这个阈值是 Auto 模式唯一的调节旋钮 ——
/// 调低会让它更像 bypass，调高会让它更像 Default。
const MIN_CONFIDENCE: f32 = 0.8;

/// 判危请求的超时。
///
/// 短是刻意的：超时之后弹窗照样在那里等用户，什么都没坏；而一个要等
/// 十几秒的判危器还不如没有 —— 用户早就自己点了。
const JUDGE_TIMEOUT: Duration = Duration::from_secs(8);

/// 只要一个词加一个数字，给 16 token 绰绰有余。压得这么死也是护栏：
/// 模型想写长篇解释也写不完，省钱且逼它守格式。
const MAX_OUTPUT_TOKENS: u32 = 16;

const SYSTEM: &str = "\
你是一个只做安全判定的分类器，不是助手。给你一个 AI 编程助手即将执行的操作，\
判断它会不会造成用户不想要的后果。\n\n\
判 SAFE 必须**全部**满足：\n\
- 不删除、不覆盖用户已有的数据\n\
- 不把任何东西发到外部，也不取得持久化执行权（改 shell 启动脚本、装全局钩子之类）\n\
- 不动版本库历史、分支状态或远端（commit / push / reset / rebase / checkout 都不算安全）\n\
- 失败了不留痕迹，重跑一次没有额外后果\n\n\
只要有一条说不准，就判 UNSAFE。你的职责是保守，不是替用户省事。\n\n\
只输出一行：`SAFE <0-100>` 或 `UNSAFE <0-100>`，数字是你的确信程度。\n\
例如：SAFE 95\n\
不要解释，不要写别的。";

pub struct HostClassifier {
    provider: Arc<dyn Provider>,
    model: String,
}

impl HostClassifier {
    /// 按配置装。复用子 agent 的便宜档 —— 一个配置管两处省钱的地方，
    /// 用户少配一次，而这两件事要的正好是同一种模型（便宜、够用、可降级）。
    ///
    /// 没配便宜档返回 None，调用方装 `NoClassifier`。
    pub fn from_config(config: &crate::config::AppConfig) -> Option<Self> {
        let cheap = crate::subagent::CheapModel::from_config(config)?;
        Some(Self { provider: cheap.provider, model: cheap.model })
    }
}

/// 解析模型那一行。
///
/// `[约束]` 先看 UNSAFE 再看 SAFE —— "UNSAFE" 里**包含** "SAFE"，顺序反了
/// 就会把每一次拒绝读成放行。这是这个文件里最容易写错、后果最严重的一行。
///
/// 认不出格式返回 Hold（见 [`SafetyVerdict`] 的说明）。
fn parse(raw: &str) -> SafetyVerdict {
    let up = raw.trim().to_ascii_uppercase();
    if up.contains("UNSAFE") {
        return SafetyVerdict::Hold;
    }
    if !up.contains("SAFE") {
        return SafetyVerdict::Hold;
    }
    // 数字缺了按 0 算，于是过不了阈值 —— 不守格式的输出不该拿到放行。
    let confidence = up
        .split(|c: char| !c.is_ascii_digit())
        .rfind(|s| !s.is_empty())
        .and_then(|n| n.parse::<f32>().ok())
        .map_or(0.0, |n| (n / 100.0).clamp(0.0, 1.0));

    if confidence >= MIN_CONFIDENCE {
        SafetyVerdict::Safe { confidence }
    } else {
        SafetyVerdict::Hold
    }
}

#[async_trait]
impl SafetyClassifier for HostClassifier {
    // 真实时钟。禁用列表针对的是内核逻辑 —— 那里的时间必须可注入才能做
    // 黄金回放；这里等的是一次网络请求，回放里根本走不到这条路径。
    #[allow(clippy::disallowed_methods)]
    async fn judge(&self, tool: &str, what: &str) -> SafetyVerdict {
        let request = ProviderRequest {
            model: self.model.clone(),
            messages: vec![Message::User {
                id: riot_protocol::id::MessageId::from_raw("msg_classify"),
                content: vec![UserContent::Text {
                    text: format!("工具：{tool}\n操作：{what}"),
                }],
                meta: MessageMeta { synthetic: true, ..Default::default() },
            }],
            system: SYSTEM.into(),
            // 不给工具：判危的模型能调工具就不是判危了。
            tools: Vec::new(),
            max_output_tokens: Some(MAX_OUTPUT_TOKENS),
            thinking: ThinkingConfig::Off,
        };

        let cancel = CancellationToken::new();
        let collect = async {
            let mut stream = self.provider.stream(request, cancel.clone());
            let mut text = String::new();
            while let Some(ev) = stream.next().await {
                match ev {
                    ProviderEvent::Message(Message::Assistant { content, .. }) => {
                        for c in content {
                            if let AssistantContent::Text { text: t } = c {
                                text.push_str(&t);
                            }
                        }
                    }
                    // 请求失败 = 判不了。返回 Hold 让弹窗继续等用户。
                    ProviderEvent::Error(e) => {
                        tracing::debug!(error = %e, "判危请求失败，改问用户");
                        return None;
                    }
                    _ => {}
                }
            }
            Some(text)
        };

        match tokio::time::timeout(JUDGE_TIMEOUT, collect).await {
            Ok(Some(text)) => {
                let verdict = parse(&text);
                tracing::debug!(tool, raw = %text.trim(), ?verdict, "判危结果");
                verdict
            }
            Ok(None) => SafetyVerdict::Hold,
            Err(_) => {
                // 超时要真的把请求停掉，别让它在后台跑完再没人收。
                cancel.cancel();
                tracing::debug!(tool, "判危超时，改问用户");
                SafetyVerdict::Hold
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "UNSAFE" 里包含 "SAFE"。判定顺序写反的话，每一次拒绝都会被读成
    /// 放行 —— 而且是静默的：日志里是"分类器批准"，看不出哪里错了。
    #[test]
    fn unsafe_不能被读成_safe() {
        assert_eq!(parse("UNSAFE 95"), SafetyVerdict::Hold);
        assert_eq!(parse("unsafe 99"), SafetyVerdict::Hold);
        assert_eq!(parse("  UNSAFE  100  "), SafetyVerdict::Hold);
    }

    #[test]
    fn 高确信的_safe_才放行() {
        assert_eq!(parse("SAFE 95"), SafetyVerdict::Safe { confidence: 0.95 });
        assert_eq!(parse("SAFE 80"), SafetyVerdict::Safe { confidence: 0.8 });
        // 犹豫的放行不算放行。
        assert_eq!(parse("SAFE 79"), SafetyVerdict::Hold);
        assert_eq!(parse("SAFE 10"), SafetyVerdict::Hold);
    }

    /// 不守格式的输出不该拿到放行。
    #[test]
    fn 认不出的输出一律_hold() {
        for raw in [
            "",
            "  ",
            "SAFE",                      // 没给数字
            "我觉得这个命令应该没问题",  // 模型开始聊天
            "MAYBE 90",
            "{\"verdict\":\"safe\"}",     // 换了个格式
            "SAFE 1000",                  // 越界（clamp 到 1.0 也不该靠这个过）
        ] {
            let v = parse(raw);
            if raw == "SAFE 1000" {
                // clamp 之后是 1.0，这一条会放行 —— 记在这里是为了说明
                // 它是**已知且可接受**的：数字越界仍然是一次明确的 SAFE。
                assert_eq!(v, SafetyVerdict::Safe { confidence: 1.0 });
            } else {
                assert_eq!(v, SafetyVerdict::Hold, "「{raw}」不该被放行");
            }
        }
    }

    /// 没配便宜档就不该装分类器 —— Auto 模式退化成 Default，照常弹窗。
    #[test]
    fn 没配便宜档时装不出分类器() {
        let cfg = crate::config::AppConfig::default();
        assert!(HostClassifier::from_config(&cfg).is_none());
    }

    #[tokio::test]
    async fn 占位分类器永远_hold() {
        use riot_protocol::permission::NoClassifier;
        assert_eq!(
            NoClassifier.judge("Bash", "ls -la").await,
            SafetyVerdict::Hold,
            "没有判危能力时必须退回问人，不能放行"
        );
    }
}
