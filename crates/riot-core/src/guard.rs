//! 给事件流套上 [`StreamGuard`]。
//!
//! 为什么需要一层包装：`StreamGuard` 的 `Drop` 会断言「Done 出现过」，
//! 但**消费者提前 drop 流是完全合法的**（用户关了窗口、UI 切走了会话）。
//! 把 guard 直接放进 `stream!` 块里，这两种情况在 Drop 时长得一模一样，
//! 于是关窗口就会误报。
//!
//! 这里的做法是记住内层流有没有 poll 出过 `None`。只有「自然结束了但没发
//! Done」才是 bug，提前 drop 就 disarm 掉。

use std::pin::Pin;
use std::task::{Context, Poll};

use futures_core::Stream;
use pin_project_lite::pin_project;
use riot_protocol::event::AgentEvent;

use crate::invariants::StreamGuard;

pin_project! {
    pub struct Guarded<S> {
        #[pin]
        inner: S,
        guard: StreamGuard,
        exhausted: bool,
    }

    impl<S> PinnedDrop for Guarded<S> {
        fn drop(this: Pin<&mut Self>) {
            let this = this.project();
            // 没走到流末尾就被 drop：消费者主动放弃，不是内核的错。
            if !*this.exhausted {
                this.guard.disarm();
            }
        }
    }
}

/// 包装一个事件流，让「忘了发 Done」在测试里立刻 panic。
pub fn guarded<S: Stream<Item = AgentEvent>>(inner: S) -> Guarded<S> {
    Guarded {
        inner,
        guard: StreamGuard::new(),
        exhausted: false,
    }
}

impl<S: Stream<Item = AgentEvent>> Stream for Guarded<S> {
    type Item = AgentEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<AgentEvent>> {
        let this = self.project();
        match this.inner.poll_next(cx) {
            Poll::Ready(Some(ev)) => {
                this.guard.observe(&ev);
                Poll::Ready(Some(ev))
            }
            Poll::Ready(None) => {
                *this.exhausted = true;
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use riot_protocol::event::TerminalReason;

    // 这三个测试都靠「跑完不 panic」或「按预期 panic」来判定。
    // debug 构建下 invariant! 直接 panic 而不是记进 violations 表，
    // 所以断言 take_violations().is_empty() 是没有意义的 —— 它永远成立。

    fn ev_start() -> AgentEvent {
        AgentEvent::RequestStart {
            turn: 0,
            model: "m".into(),
            after: None,
        }
    }

    #[tokio::test]
    async fn 正常结束的流不报警() {
        let s = guarded(futures::stream::iter(vec![
            ev_start(),
            AgentEvent::Done {
                reason: TerminalReason::Completed,
            },
        ]));
        let events: Vec<_> = s.collect().await;
        assert_eq!(events.len(), 2);
    }

    #[tokio::test]
    async fn 消费者提前放弃不算违规() {
        // 必须 Box::pin 而不是 pin_mut! —— 后者把值钉在栈上，`drop(s)` 丢掉的
        // 只是 Pin 引用，Guarded 本体要到函数结束才析构，断言就跑在了它前面。
        // 这个测试最初就是那么写的，clippy 的 drop_non_drop 才把它揪出来。
        let mut s = Box::pin(guarded(futures::stream::iter(vec![ev_start()])));

        // 只取一个就走人 —— 相当于用户关了窗口
        let _ = s.next().await;
        drop(s);
    }

    // debug 行为的说明书：release 里 invariant! 记录不炸，编译出去。
    // 同 invariants.rs 那批 should_panic 的处理。
    #[cfg(debug_assertions)]
    #[tokio::test]
    #[should_panic(expected = "从未发出 Done")]
    async fn 自然结束却没有_done_要报警() {
        // 与上一个测试的唯一区别：这里把流读到了底。
        // 读到底 = 内核认为自己干完了，那就必须有 Done。
        let s = guarded(futures::stream::iter(vec![ev_start()]));
        let _: Vec<_> = s.collect().await;
    }
}
