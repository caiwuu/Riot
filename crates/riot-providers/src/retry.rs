//! 重试决策与退避。
//!
//! 整层是纯函数：给定「状态码 + 响应头 + 已重试次数 + 请求来源」，
//! 返回「重试还是放弃、等多久」。不碰时钟、不碰网络，所以每条规则都能
//! 单独摆进测试里。
//!
//! 见 ARCHITECTURE.md §11.4

use std::time::Duration;

/// 请求是谁发的。**决定 529 要不要重试。**
///
/// 这个区分不是过度设计。容量雪崩时每一次重试都是数倍的网关放大，
/// 而后台请求（标题生成、摘要）失败了用户根本看不见 —— 为看不见的东西
/// 参与雪崩是纯粹的损失。
///
/// 判断标准只有一条：**用户此刻是不是在等这个结果。**
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RequestSource {
    /// 用户在等：主循环、压缩、子 agent。
    Foreground,
    /// 用户看不见：标题生成、摘要、建议。
    ///
    /// 这是默认值，取的是 fail-closed 的方向：漏标 `Foreground` 只会让某个
    /// 请求少重试几次（功能退化，能看见），漏标 `Background` 会让它在过载时
    /// 参与雪崩放大（伤害别人，看不见）。
    #[default]
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryDecision {
    Retry {
        after: Duration,
    },
    /// 别重试了。带上原因，它会进日志和给用户的错误消息。
    GiveUp(GiveUpReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GiveUpReason {
    /// 服务端明确说了别重试。
    ServerSaidNo,
    /// 重试次数用完。
    Exhausted,
    /// 这类错误重试没有意义（4xx 参数错误等）。
    NotRetryable,
    /// 后台请求遇到过载，立刻放弃。
    BackgroundOverload,
    /// 订阅制用户的 429。他们的限流窗口是几小时，重试毫无意义。
    SubscriptionRateLimit,
    /// 刷新凭证之后仍然认证失败。再试也是一样的结果。
    AuthUnrecoverable,
}

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base: Duration,
    pub cap: Duration,
    /// 抖动比例。0.25 = 在退避值上下浮动 ±25%。
    pub jitter: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 10,
            base: Duration::from_millis(500),
            cap: Duration::from_secs(32),
            jitter: 0.25,
        }
    }
}

/// 一次失败请求的信息。
#[derive(Debug, Clone, Default)]
pub struct FailureContext<'a> {
    pub status: Option<u16>,
    /// 传输层错误（连接被拒、DNS 失败、TLS 握手失败）。
    pub transport_error: bool,
    /// `retry-after` 响应头，秒。
    pub retry_after_secs: Option<u64>,
    /// `x-should-retry` 响应头。
    pub x_should_retry: Option<bool>,
    pub source: RequestSource,
    /// 订阅制账号。影响 429 的处理。
    pub is_subscription: bool,
    /// 已经重试过几次（首次失败时为 0）。
    pub attempt: u32,
    pub error_body: &'a str,
}

/// 决定要不要重试、等多久。
///
/// `[约束]` 判定顺序不能改。`x-should-retry` 必须排在所有规则前面 ——
/// 它是服务端对这一个具体请求的指令，比任何本地推断都准。把它放在
/// 状态码判定之后，会出现「服务端说别重了但我们还在重」的情况。
pub fn decide(policy: &RetryPolicy, ctx: &FailureContext<'_>, jitter_seed: u64) -> RetryDecision {
    // 1. 服务端指令优先级最高
    if ctx.x_should_retry == Some(false) {
        return RetryDecision::GiveUp(GiveUpReason::ServerSaidNo);
    }

    // 2. 次数用完
    if ctx.attempt >= policy.max_attempts {
        return RetryDecision::GiveUp(GiveUpReason::Exhausted);
    }

    // 3. 这类错误值不值得重试
    if ctx.x_should_retry != Some(true) {
        match classify(ctx) {
            Retryability::No(reason) => return RetryDecision::GiveUp(reason),
            Retryability::Yes => {}
        }
    }

    // 4. 等多久。服务端给了 Retry-After 就听它的 —— 本地退避算的是
    //    「我猜多久能好」，服务端知道的是「实际多久能好」。
    let delay = match ctx.retry_after_secs {
        Some(secs) => Duration::from_secs(secs.min(policy.cap.as_secs().max(secs.min(300)))),
        None => backoff(policy, ctx.attempt, jitter_seed),
    };

    RetryDecision::Retry { after: delay }
}

enum Retryability {
    Yes,
    No(GiveUpReason),
}

fn classify(ctx: &FailureContext<'_>) -> Retryability {
    if ctx.transport_error {
        return Retryability::Yes;
    }

    let Some(status) = ctx.status else {
        return Retryability::No(GiveUpReason::NotRetryable);
    };

    match status {
        408 | 409 => Retryability::Yes,

        // 429：订阅制用户的限流窗口是几小时，重试只是白等
        429 if ctx.is_subscription => Retryability::No(GiveUpReason::SubscriptionRateLimit),
        429 => Retryability::Yes,

        // 401 / 403 只重试**一次**，给调用方一个刷新凭证的机会。
        //
        // `[约束]` 这里必须限次，不能只靠 max_attempts 兜。凭证刷新不了的话，
        // 重试十次就是十次 401 —— 每次都是一个完整的网络往返，用户干等十轮
        // 退避（累计一分多钟）才看到「密钥无效」。而这个结论第一次就知道了。
        401 | 403 if ctx.attempt > 0 => Retryability::No(GiveUpReason::AuthUnrecoverable),
        401 => Retryability::Yes,
        403 if ctx.error_body.contains("token revoked") => Retryability::Yes,
        403 => Retryability::No(GiveUpReason::NotRetryable),

        // 529 过载：只有用户在等的请求才值得参与重试
        529 => match ctx.source {
            RequestSource::Foreground => Retryability::Yes,
            RequestSource::Background => Retryability::No(GiveUpReason::BackgroundOverload),
        },

        500..=599 => Retryability::Yes,

        // 其余 4xx 是我们自己的问题（参数错误、内容策略），重试一百次也一样
        _ => Retryability::No(GiveUpReason::NotRetryable),
    }
}

/// 指数退避：`base × 2^attempt`，封顶后加抖动。
///
/// `[约束]` 抖动必须有。没有抖动的话，同时失败的一批请求会在同一毫秒
/// 一起重试，把刚恢复的服务再打垮一次 —— 这正是过载时最不该做的事。
///
/// 抖动用调用方给的 seed 而不是内部随机，是为了让重试时序在测试里可复现。
pub fn backoff(policy: &RetryPolicy, attempt: u32, jitter_seed: u64) -> Duration {
    let exp = policy
        .base
        .saturating_mul(2u32.saturating_pow(attempt.min(16)));
    let capped = exp.min(policy.cap);

    if policy.jitter <= 0.0 {
        return capped;
    }

    // seed → [-1, 1] 的确定性映射。
    //
    // 必须先打散再取模。调用方最可能传的是重试序号或时间戳这类**连续值**，
    // 直接 `seed % n` 会让它们全落在同一段区间里，结果抖动变成单向的 ——
    // 那等于没抖，同时失败的请求照样挤在一起重试。
    let mixed = jitter_seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .rotate_left(31)
        ^ jitter_seed.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let unit = ((mixed % 2001) as f64 / 1000.0) - 1.0;
    let factor = 1.0 + unit * policy.jitter;
    capped.mul_f64(factor.max(0.0))
}

/// 从 400 错误里解析出上下文溢出的实际数字。
///
/// Anthropic 的报错长这样：
/// `input length and max_tokens exceed context limit: 188059 + 20000 > 200000`
///
/// 解析出来就能自动调低 `max_tokens` 重试，而不是直接把错误甩给用户。
/// 解析不出来返回 None —— 措辞变了就退回普通错误处理，不要猜。
pub fn parse_context_overflow(body: &str) -> Option<ContextOverflow> {
    let tail = body.split("context limit:").nth(1)?;

    let nums: Vec<u32> = tail
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .take(3)
        .filter_map(|s| s.parse().ok())
        .collect();

    match nums.as_slice() {
        [input, max_tokens, limit] => Some(ContextOverflow {
            input_tokens: *input,
            max_tokens: *max_tokens,
            context_limit: *limit,
        }),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextOverflow {
    pub input_tokens: u32,
    pub max_tokens: u32,
    pub context_limit: u32,
}

impl ContextOverflow {
    /// 重试时该用多大的 max_tokens。
    ///
    /// 留 5% 余量，因为 input_tokens 是服务端按它自己的分词器算的，
    /// 而我们下一次请求的内容可能因为重新组装而略有出入。贴着上限
    /// 算出来的值会让重试再撞一次同样的错。
    pub fn suggested_max_tokens(&self) -> Option<u32> {
        let available = self.context_limit.checked_sub(self.input_tokens)?;
        let with_margin = (available as f64 * 0.95) as u32;
        (with_margin >= 1024).then_some(with_margin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn ctx(status: u16) -> FailureContext<'static> {
        FailureContext {
            status: Some(status),
            source: RequestSource::Foreground,
            ..Default::default()
        }
    }

    #[test]
    fn 服务端说别重试就别重试() {
        let mut c = ctx(500); // 500 本来是要重试的
        c.x_should_retry = Some(false);
        assert_eq!(
            decide(&RetryPolicy::default(), &c, 0),
            RetryDecision::GiveUp(GiveUpReason::ServerSaidNo),
            "x-should-retry 必须压过状态码判定"
        );
    }

    #[test]
    fn 服务端说重试就重试_哪怕状态码不该重() {
        let mut c = ctx(400);
        c.x_should_retry = Some(true);
        assert!(matches!(
            decide(&RetryPolicy::default(), &c, 0),
            RetryDecision::Retry { .. }
        ));
    }

    #[test]
    fn 后台请求不参与_529_雪崩() {
        let mut c = ctx(529);
        c.source = RequestSource::Background;
        assert_eq!(
            decide(&RetryPolicy::default(), &c, 0),
            RetryDecision::GiveUp(GiveUpReason::BackgroundOverload),
            "标题生成失败用户看不见，为它参与雪崩是纯损失"
        );

        c.source = RequestSource::Foreground;
        assert!(matches!(
            decide(&RetryPolicy::default(), &c, 0),
            RetryDecision::Retry { .. }
        ));
    }

    #[test]
    fn 订阅制的_429_不重试() {
        let mut c = ctx(429);
        c.is_subscription = true;
        assert_eq!(
            decide(&RetryPolicy::default(), &c, 0),
            RetryDecision::GiveUp(GiveUpReason::SubscriptionRateLimit),
            "限流窗口是几小时，重试只是白等"
        );
    }

    #[test]
    fn 参数错误不重试() {
        for status in [400, 404, 422] {
            assert_eq!(
                decide(&RetryPolicy::default(), &ctx(status), 0),
                RetryDecision::GiveUp(GiveUpReason::NotRetryable),
                "status {status}"
            );
        }
    }

    #[test]
    fn 认证失败重试一次让调用方刷凭证() {
        assert!(matches!(
            decide(&RetryPolicy::default(), &ctx(401), 0),
            RetryDecision::Retry { .. }
        ));

        // 但只有一次。刷不出新凭证的话，重试十次就是十次 401 ——
        // 用户干等一分多钟才看到「密钥无效」，而这个结论第一次就知道了。
        let mut second = ctx(401);
        second.attempt = 1;
        assert_eq!(
            decide(&RetryPolicy::default(), &second, 0),
            RetryDecision::GiveUp(GiveUpReason::AuthUnrecoverable),
        );

        let mut revoked = ctx(403);
        revoked.error_body = "OAuth token revoked";
        assert!(matches!(
            decide(&RetryPolicy::default(), &revoked, 0),
            RetryDecision::Retry { .. }
        ));

        // 普通 403 是真的没权限，重试无意义
        assert_eq!(
            decide(&RetryPolicy::default(), &ctx(403), 0),
            RetryDecision::GiveUp(GiveUpReason::NotRetryable)
        );
    }

    #[test]
    fn 传输错误重试() {
        let c = FailureContext {
            transport_error: true,
            source: RequestSource::Foreground,
            ..Default::default()
        };
        assert!(matches!(
            decide(&RetryPolicy::default(), &c, 0),
            RetryDecision::Retry { .. }
        ));
    }

    #[test]
    fn retry_after_压过本地退避() {
        let mut c = ctx(429);
        c.retry_after_secs = Some(5);
        c.attempt = 0; // 本地退避会算出 500ms

        assert_eq!(
            decide(&RetryPolicy::default(), &c, 0),
            RetryDecision::Retry {
                after: Duration::from_secs(5)
            },
            "服务端知道实际多久能好，本地退避只是在猜"
        );
    }

    #[test]
    fn 次数用完就放弃() {
        let mut c = ctx(500);
        c.attempt = 10;
        assert_eq!(
            decide(&RetryPolicy::default(), &c, 0),
            RetryDecision::GiveUp(GiveUpReason::Exhausted)
        );
    }

    #[test]
    fn 退避是指数的且有上限() {
        let p = RetryPolicy {
            jitter: 0.0,
            ..Default::default()
        };
        assert_eq!(backoff(&p, 0, 0), Duration::from_millis(500));
        assert_eq!(backoff(&p, 1, 0), Duration::from_secs(1));
        assert_eq!(backoff(&p, 3, 0), Duration::from_secs(4));
        assert_eq!(backoff(&p, 20, 0), p.cap, "必须封顶，否则等到天荒地老");
    }

    #[test]
    fn 抖动在正负区间内() {
        let p = RetryPolicy::default();
        let base = Duration::from_secs(4); // attempt=3

        // 用连续的小 seed —— 这正是调用方最可能传的（重试序号、递增计数）。
        // 不打散直接取模的话，它们会全落在同一段区间，抖动变成单向的。
        let mut lower = 0;
        let mut upper = 0;
        for seed in 0..200u64 {
            let d = backoff(&p, 3, seed);
            assert!(
                d >= base.mul_f64(0.75) && d <= base.mul_f64(1.25),
                "seed {seed} 抖出界: {d:?}"
            );
            if d < base {
                lower += 1;
            }
            if d > base {
                upper += 1;
            }
        }
        assert!(
            lower > 60 && upper > 60,
            "抖动分布严重偏向一侧（下 {lower} / 上 {upper}）。\
             只往一边抖等于没抖 —— 同时失败的请求还是会挤在一起重试"
        );
    }

    #[test]
    fn 抖动可复现() {
        let p = RetryPolicy::default();
        assert_eq!(backoff(&p, 3, 42), backoff(&p, 3, 42));
    }

    #[test]
    fn 解析上下文溢出的错误() {
        let body = "input length and max_tokens exceed context limit: 188059 + 20000 > 200000";
        assert_eq!(
            parse_context_overflow(body),
            Some(ContextOverflow {
                input_tokens: 188059,
                max_tokens: 20000,
                context_limit: 200000,
            })
        );
    }

    #[test]
    fn 措辞变了就不猜() {
        assert_eq!(parse_context_overflow("something went wrong"), None);
        assert_eq!(parse_context_overflow("context limit: 100"), None);
    }

    #[test]
    fn 建议的_max_tokens_留了余量() {
        let o = ContextOverflow {
            input_tokens: 180_000,
            max_tokens: 30_000,
            context_limit: 200_000,
        };
        let suggested = o.suggested_max_tokens().expect("还有 20k 空间");
        assert!(
            suggested < 20_000,
            "贴着上限算会让重试再撞一次同样的错：{suggested}"
        );
        assert_eq!(suggested, 19_000);
    }

    #[test]
    fn 空间不够就别重试了() {
        let o = ContextOverflow {
            input_tokens: 199_900,
            max_tokens: 20_000,
            context_limit: 200_000,
        };
        assert_eq!(
            o.suggested_max_tokens(),
            None,
            "只剩 100 token，调低上限也没用，该走压缩路径"
        );
    }
}
