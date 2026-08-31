//! 流式请求的静默看门狗。
//!
//! # 为什么 HTTP timeout 不够
//!
//! `[约束]` 流式请求最脆弱的不是建连，**是建连之后的静默**。
//!
//! HTTP 客户端的 timeout 基本只覆盖初始 fetch —— 一旦响应头回来了，
//! body 挂在那里不动，它就管不着了。表现是 agent 转圈转到天荒地老，
//! 没有任何错误、没有任何日志，用户只能杀进程。
//!
//! 这条路径在本地开发时几乎碰不到，因为本地网络不会静默挂死。
//! 它专门发生在用户那边：企业代理、移动网络切换、云厂商的 LB 空闲回收。
//!
//! # 超时之后做什么
//!
//! 这一层只负责**发现**静默并结束流。降级到非流式请求是调用方的事 ——
//! 把重试策略塞进看门狗会让它同时管两件事，而这两件事的正确性条件完全不同。

use std::sync::Arc;
use std::time::Duration;

use async_stream::stream;
use futures::StreamExt;
use futures_core::Stream;
use riot_protocol::provider::{ProviderError, ProviderEvent};
use riot_protocol::tool::Clock;

/// 默认静默阈值。
///
/// 90 秒是权衡的结果：思考模式下模型确实可能几十秒不吐字，
/// 定得太短会误杀正常的长思考；定得太长则用户已经放弃等待了。
pub const DEFAULT_IDLE: Duration = Duration::from_secs(90);

/// 给事件流套一个静默看门狗。
///
/// 计时器在**每个事件**后重置 —— 看的是「两个事件之间隔了多久」，
/// 不是「整条流跑了多久」。后者会把正常的长响应误杀掉。
pub fn with_idle_watchdog<S>(
    inner: S,
    idle: Duration,
    clock: Arc<dyn Clock>,
) -> impl Stream<Item = ProviderEvent> + Send
where
    S: Stream<Item = ProviderEvent> + Send + 'static,
{
    let idle_ms = idle.as_millis() as u64;

    stream! {
        futures::pin_mut!(inner);

        loop {
            let tick = clock.sleep_ms(idle_ms);

            tokio::select! {
                // biased 让「有数据」优先于「超时」。没有它的话，数据和超时
                // 同时就绪时 select 会随机挑一个 —— 表现为偶发的假超时，
                // 而且只在负载高的时候出现。
                biased;

                item = inner.next() => {
                    match item {
                        Some(ev) => {
                            // 错误事件之后流就该结束了，别再等下一个
                            let is_terminal = matches!(ev, ProviderEvent::Error(_));
                            yield ev;
                            if is_terminal {
                                break;
                            }
                        }
                        None => break,
                    }
                }

                _ = tick => {
                    // 证据采集：是否值得实现"超时后降级非流式"（ARCHITECTURE
                    // §11.3 标注未实现）取决于这条真实触发的频率。日志攒一阵，
                    // 频率可观再排期，别为没证实的场景先付一套非流式解析。
                    tracing::warn!(idle_secs = idle.as_secs(), "流式静默超时，结束本条流");
                    yield ProviderEvent::Error(ProviderError::Transport {
                        message: format!(
                            "流静默超过 {} 秒。连接还在，但服务端不发数据了 —— \
                             通常是中间代理或网关的问题。",
                            idle.as_secs()
                        ),
                    });
                    break;
                }
            }
        }
    }
}

/// 生产用的时钟。
///
/// 测试里用 `tokio::time::pause()` 控制它 —— tokio 的时间轮在暂停后会
/// 自动推进到下一个 timer，所以 90 秒的超时能在几微秒内测完。
/// 这比自己造一个可快进的 Clock 更可靠：它连 `select!` 的调度语义
/// 一起模拟了，而手写的 mock 只能模拟「时间到了」。
pub struct TokioClock;

#[async_trait::async_trait]
impl Clock for TokioClock {
    // 豁免理由：这就是 Clock 的生产实现本身，它必须碰真实时间。
    // 内核侧代码一律通过注入的 Clock 使用它，不直接调这两个 API。
    #[allow(clippy::disallowed_methods)]
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// 本地时区偏移。每次现查不缓存 —— 夏令时切换和用户改时区都该
    /// 下一轮就生效，而 `localtime_r` 本来就只是一次结构体填充的开销。
    ///
    /// `localtime_r` 是唯一同时覆盖 macOS/Linux 的标准做法；为这一个数
    /// 引整套 chrono 不划算（同 riot-tools date.rs 的取舍）。Windows 走
    /// trait 默认值 0：时钟行会诚实标注 UTC，不会把 UTC 假装成本地时刻。
    #[cfg(unix)]
    fn tz_offset_minutes(&self) -> i32 {
        let secs = (self.now_ms() / 1000) as libc::time_t;
        // SAFETY: tm 是本线程栈上的出参，zeroed 的 tm 对 localtime_r 合法
        // （它只写不读）；localtime_r 是线程安全变体。
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
            return 0;
        }
        (tm.tm_gmtoff / 60) as i32
    }

    #[allow(clippy::disallowed_methods)]
    async fn sleep_ms(&self, ms: u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
}

#[cfg(test)]
// 豁免理由：这些测试验证的就是「时间流逝」本身，被测对象是看门狗的计时。
// 跑在 `start_paused = true` 下，tokio 的时间轮会自动推进到下一个 timer，
// 所以 90 秒的超时在几微秒内测完 —— 既不慢，也不依赖真实挂钟。
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use riot_protocol::event::StreamDelta;
    use riot_protocol::id::MessageId;

    fn delta(text: &str) -> ProviderEvent {
        ProviderEvent::Delta(StreamDelta::Text {
            message_id: MessageId::from_raw("m1"),
            text: text.into(),
        })
    }

    fn is_idle_error(ev: &ProviderEvent) -> bool {
        matches!(
            ev,
            ProviderEvent::Error(ProviderError::Transport { message }) if message.contains("静默")
        )
    }

    #[tokio::test(start_paused = true)]
    async fn 正常流不受影响() {
        let inner = futures::stream::iter(vec![delta("a"), delta("b")]);
        let s = with_idle_watchdog(inner, DEFAULT_IDLE, Arc::new(TokioClock));
        let out: Vec<_> = s.collect().await;

        assert_eq!(out.len(), 2);
        assert!(!out.iter().any(is_idle_error));
    }

    #[tokio::test(start_paused = true)]
    async fn 建连后静默会被抓到() {
        // 这是 HTTP timeout 完全覆盖不到的场景：连接活着，但没数据。
        // 没有看门狗的话，agent 会永远转圈。
        let inner = futures::stream::pending::<ProviderEvent>();
        let s = with_idle_watchdog(inner, Duration::from_secs(90), Arc::new(TokioClock));
        let out: Vec<_> = s.collect().await;

        assert_eq!(out.len(), 1);
        assert!(is_idle_error(&out[0]), "{:?}", out[0]);
    }

    #[tokio::test(start_paused = true)]
    async fn 计时器在每个事件后重置() {
        // 看的是「两个事件之间隔了多久」，不是「整条流跑了多久」。
        // 判错的话，正常的长响应会被误杀。
        let idle = Duration::from_secs(10);

        let inner = stream! {
            for i in 0..5 {
                // 每次都比阈值短，但累计远超阈值
                tokio::time::sleep(Duration::from_secs(8)).await;
                yield delta(&format!("chunk{i}"));
            }
        };

        let s = with_idle_watchdog(inner, idle, Arc::new(TokioClock));
        let out: Vec<_> = s.collect().await;

        assert_eq!(out.len(), 5, "累计 40 秒 > 阈值 10 秒，但每一段都没超");
        assert!(!out.iter().any(is_idle_error));
    }

    #[tokio::test(start_paused = true)]
    async fn 中途静默会被抓到() {
        let idle = Duration::from_secs(10);

        let inner = stream! {
            yield delta("开头");
            tokio::time::sleep(Duration::from_secs(60)).await;
            yield delta("这条永远到不了");
        };

        let s = with_idle_watchdog(inner, idle, Arc::new(TokioClock));
        let out: Vec<_> = s.collect().await;

        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], ProviderEvent::Delta(_)));
        assert!(is_idle_error(&out[1]));
    }

    #[tokio::test(start_paused = true)]
    async fn 错误之后不再等待() {
        // 错误是终止事件。继续等下一个只会白白多耗一个 idle 周期，
        // 用户要多等 90 秒才看到那个本该立刻显示的错误。
        let inner = stream! {
            yield ProviderEvent::Error(ProviderError::Auth { message: "401".into() });
            futures::future::pending::<()>().await;
            yield delta("不可达");
        };

        let s = with_idle_watchdog(inner, Duration::from_secs(90), Arc::new(TokioClock));
        let out: Vec<_> = s.collect().await;

        assert_eq!(out.len(), 1, "错误后应立即结束，不再空等一个 idle 周期");
        assert!(matches!(
            out[0],
            ProviderEvent::Error(ProviderError::Auth { .. })
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn 空流立即结束() {
        let inner = futures::stream::empty::<ProviderEvent>();
        let s = with_idle_watchdog(inner, DEFAULT_IDLE, Arc::new(TokioClock));
        let out: Vec<_> = s.collect().await;
        assert!(out.is_empty(), "空流不是静默，不该报超时");
    }
}
