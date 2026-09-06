//! HTTP 失败 → [`ProviderError`]：两家 provider 共用的一张映射表。
//!
//! 界面上的错误文案由词典键决定（见 `riot_protocol::text`），键在这里
//! 选：认证、限流、过载、余额不足、模型不存在……能从状态码和响应正文里
//! 认出来的都单独给键，认不出的退回笼统的"服务方拒绝了请求"。服务方的
//! 原文一律放 `detail` —— 它是排查时唯一可靠的线索，但不翻译。
//!
//! 放在一个文件里而不是各写一份：两边的措辞分叉过一次（同一个 401 在
//! OpenAI 侧说"凭证无效"、Anthropic 侧说"凭证无效，刷新后仍然失败"），
//! 用户换个 provider 就看到另一套解释。

use riot_protocol::provider::ProviderError;
use riot_protocol::text::UiError;
use riot_protocol::ui_error;

use crate::retry::GiveUpReason;
use crate::transport::HttpError;

/// 认证失败。401 / 403，以及服务端事件流里的 `authentication_error`。
pub(crate) fn auth(detail: impl ToString) -> ProviderError {
    ProviderError::Auth {
        error: ui_error!("kernel.provider.auth"; detail),
    }
}

/// 限流（429）。订阅制账号的限流窗口是几小时，重试无意义，所以是
/// "重试耗尽"这一类而不是可恢复错误。
pub(crate) fn rate_limited(detail: impl ToString) -> ProviderError {
    ProviderError::RetriesExhausted {
        error: ui_error!("kernel.provider.rateLimited"; detail),
    }
}

/// 服务方过载（529 / 503 / `overloaded_error`），重试过仍然过载。
pub(crate) fn overloaded(detail: impl ToString) -> ProviderError {
    ProviderError::RetriesExhausted {
        error: ui_error!("kernel.provider.overloaded"; detail),
    }
}

/// 后台请求遇到过载直接放弃（不参与雪崩）。
pub(crate) fn background_overloaded() -> ProviderError {
    ProviderError::RetriesExhausted {
        error: ui_error!("kernel.provider.backgroundOverloaded"),
    }
}

/// 流建立之后出的问题：读到一半断了、帧不合法、没收到结尾。
pub(crate) fn stream_broken(detail: impl ToString) -> ProviderError {
    ProviderError::Transport {
        error: ui_error!("kernel.provider.streamBroken"; detail),
    }
}

/// 传输层失败（连不上、DNS、TLS）。按 [`HttpError`] 的字段挑更具体的键。
fn transport(e: &HttpError) -> ProviderError {
    let key_error = if e.timed_out {
        ui_error!("kernel.provider.timeout"; e)
    } else {
        ui_error!("kernel.provider.transport"; e)
    };
    ProviderError::Transport { error: key_error }
}

/// 服务端明确拒绝（4xx，没有重试过）。正文里认得出的原因单独给键。
pub(crate) fn refused(e: &HttpError) -> ProviderError {
    ProviderError::Refused {
        error: refusal_error(e.status, &e.body, e),
    }
}

/// 事件流里服务端报的拒绝（OpenAI 系 chunk 带 `error`）。没有状态码。
pub(crate) fn refused_in_stream(message: &str) -> ProviderError {
    ProviderError::Refused {
        error: match classify_body(None, message) {
            BodyKind::Quota => ui_error!("kernel.provider.quota"; message),
            BodyKind::ModelNotFound => ui_error!("kernel.provider.modelNotFound"; message),
            BodyKind::Other => ui_error!("kernel.provider.refusedInStream"; message),
        },
    }
}

fn refusal_error(status: Option<u16>, body: &str, detail: impl ToString) -> UiError {
    match classify_body(status, body) {
        BodyKind::Quota => ui_error!("kernel.provider.quota"; detail),
        BodyKind::ModelNotFound => ui_error!("kernel.provider.modelNotFound"; detail),
        BodyKind::Other => match status {
            Some(s) => ui_error!("kernel.provider.refused", status = s; detail),
            None => ui_error!("kernel.provider.transport"; detail),
        },
    }
}

/// 重试次数用完。最后一次失败是什么就说什么：429 说限流、5xx 说过载、
/// 连不上说连不上 —— "重试耗尽"本身对用户没有信息量。
fn exhausted(e: &HttpError) -> ProviderError {
    match e.status {
        Some(429) => rate_limited(e),
        Some(500..=599) => overloaded(e),
        Some(_) => ProviderError::RetriesExhausted {
            error: ui_error!("kernel.provider.retriesExhausted"; e),
        },
        None => ProviderError::RetriesExhausted {
            error: if e.timed_out {
                ui_error!("kernel.provider.timeout"; e)
            } else {
                ui_error!("kernel.provider.unreachable"; e)
            },
        },
    }
}

/// 放弃重试之后的统一映射。`context_overflow` 是各协议自己认 400 正文的
/// 那一手 —— 两家的措辞不同，这里不猜。
pub(crate) fn map_giveup(
    reason: GiveUpReason,
    e: &HttpError,
    context_overflow: impl Fn(&str) -> Option<ProviderError>,
) -> ProviderError {
    match reason {
        GiveUpReason::AuthUnrecoverable => auth(e),
        GiveUpReason::SubscriptionRateLimit => rate_limited(e),
        GiveUpReason::BackgroundOverload => background_overloaded(),
        GiveUpReason::Exhausted => exhausted(e),
        GiveUpReason::ServerSaidNo | GiveUpReason::NotRetryable => match e.status {
            Some(401 | 403) => auth(e),
            Some(429) => rate_limited(e),
            // 402 是"付钱"：不管正文怎么写都是额度问题。
            Some(402) => ProviderError::Refused {
                error: ui_error!("kernel.provider.quota"; e),
            },
            Some(400) => match context_overflow(&e.body) {
                Some(overflow) => overflow,
                None => refused(e),
            },
            Some(_) => refused(e),
            None => transport(e),
        },
    }
}

enum BodyKind {
    Quota,
    ModelNotFound,
    Other,
}

/// 从错误正文里认原因。
///
/// 额度类只认带额度语境的措辞。裸的 "insufficient" 会误伤 —— DeepSeek 的
/// tool_calls 校验报错里就有 "insufficient tool messages"，真实余额没问题的
/// 用户被这句话带去查账单（生产事故）。覆盖的真实文案：DeepSeek
/// "Insufficient Balance"、OpenAI "insufficient_quota" / "You exceeded your
/// current quota"、Anthropic "credit balance is too low"、Kimi "balance is
/// insufficient"。
fn classify_body(status: Option<u16>, body: &str) -> BodyKind {
    let b = body.to_ascii_lowercase();
    let quota_words = [
        "insufficient_quota",
        "insufficient quota",
        "insufficient balance",
        "insufficient_balance",
        "insufficient funds",
        "insufficient credits",
        "balance is too low",
        "balance is insufficient",
        "exceeded your current quota",
        "quota exceeded",
        "out of credits",
        "out of quota",
        "余额不足",
        "欠费",
    ];
    if quota_words.iter().any(|w| b.contains(w)) {
        return BodyKind::Quota;
    }
    let model_words = [
        "model_not_found",
        "model not found",
        "does not exist",
        "unknown model",
        "no such model",
        "invalid model",
        "not_found_error",
    ];
    if (status == Some(404) || b.contains("model")) && model_words.iter().any(|w| b.contains(w)) {
        return BodyKind::ModelNotFound;
    }
    BodyKind::Other
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http(status: Option<u16>, body: &str) -> HttpError {
        HttpError {
            status,
            body: body.into(),
            transport: status.is_none(),
            ..Default::default()
        }
    }

    fn key(e: &ProviderError) -> String {
        e.ui_error().key().to_owned()
    }

    #[test]
    fn 余额类措辞认成额度不足_裸_insufficient_不算() {
        let e = map_giveup(
            GiveUpReason::NotRetryable,
            &http(Some(400), r#"{"error":{"message":"Insufficient Balance"}}"#),
            |_| None,
        );
        assert_eq!(key(&e), "kernel.provider.quota");

        // DeepSeek 的 tool_calls 校验报错，余额其实没问题。
        let e = map_giveup(
            GiveUpReason::NotRetryable,
            &http(Some(400), "insufficient tool messages following tool_calls"),
            |_| None,
        );
        assert_eq!(key(&e), "kernel.provider.refused");
    }

    #[test]
    fn 模型不存在有自己的键() {
        let e = map_giveup(
            GiveUpReason::NotRetryable,
            &http(
                Some(404),
                r#"{"error":{"message":"The model `gpt-9` does not exist","code":"model_not_found"}}"#,
            ),
            |_| None,
        );
        assert_eq!(key(&e), "kernel.provider.modelNotFound");
    }

    #[test]
    fn 耗尽按最后一次失败说话() {
        assert_eq!(
            key(&map_giveup(
                GiveUpReason::Exhausted,
                &http(Some(429), ""),
                |_| None
            )),
            "kernel.provider.rateLimited"
        );
        assert_eq!(
            key(&map_giveup(
                GiveUpReason::Exhausted,
                &http(Some(503), ""),
                |_| None
            )),
            "kernel.provider.overloaded"
        );
        assert_eq!(
            key(&map_giveup(
                GiveUpReason::Exhausted,
                &http(None, "dns error"),
                |_| None
            )),
            "kernel.provider.unreachable"
        );
        let mut timeout = http(None, "timed out");
        timeout.timed_out = true;
        assert_eq!(
            key(&map_giveup(GiveUpReason::Exhausted, &timeout, |_| None)),
            "kernel.provider.timeout"
        );
    }

    #[test]
    fn 服务方原文进_detail() {
        let e = map_giveup(
            GiveUpReason::NotRetryable,
            &http(Some(401), "invalid api key"),
            |_| None,
        );
        let ui = e.ui_error();
        assert_eq!(ui.key(), "kernel.provider.auth");
        assert!(
            ui.detail
                .as_deref()
                .unwrap_or_default()
                .contains("invalid api key")
        );
    }
}
