//! 用辅助模型把网页正文压缩成回答问题需要的那几百字。
//!
//! # 为什么值得单独走一次模型调用
//!
//! 一个技术文档页转成 Markdown 之后常有五万字以上，里面九成是导航、
//! 侧边栏、版本切换器、footer。原样塞进主对话有两个后果：上下文很快
//! 被撑到要压缩，以及模型在噪音里找信息的准确率明显下降。
//!
//! 花一次便宜模型的调用换回几百字，主循环的上下文能多撑十几轮。
//!
//! # 为什么复用 Provider 而不是自己发请求
//!
//! [`Provider`] 里已经有重试、退避、模型降级和看门狗。自己写一遍
//! `reqwest::post` 意味着这条链路上没有任何一样 —— 而蒸馏调用恰恰
//! 最容易撞上限流（每抓一个网页就是一次）。

use std::sync::Arc;

use futures::StreamExt;
use riot_protocol::id::MessageId;
use riot_protocol::message::{AssistantContent, Message, MessageMeta, UserContent};
use riot_protocol::provider::{Provider, ProviderEvent, ProviderRequest, ThinkingConfig};
use riot_protocol::web::{DistillRequest, WebError};
use tokio_util::sync::CancellationToken;

/// 蒸馏输出的默认上限。
///
/// 摘要长到一千 token 以上就失去意义了 —— 那已经接近直接读原文的成本。
const DEFAULT_MAX_TOKENS: u32 = 1024;

pub struct Distiller {
    provider: Arc<dyn Provider>,
    model: String,
}

impl Distiller {
    pub fn new(provider: Arc<dyn Provider>, model: String) -> Self {
        Self { provider, model }
    }

    pub async fn run(
        &self,
        req: DistillRequest,
        cancel: &CancellationToken,
    ) -> Result<String, WebError> {
        let request = ProviderRequest {
            model: self.model.clone(),
            messages: vec![Message::User {
                id: MessageId::from_raw("distill"),
                content: vec![UserContent::Text { text: req.user }],
                meta: MessageMeta::default(),
            }],
            system: req.system,
            // `[约束]` 不给工具。辅助模型拿到 Bash 就是一条"网页内容能
            // 触发本地执行"的路径 —— 提示注入正好走这里进来。
            tools: Vec::new(),
            max_output_tokens: Some(req.max_output_tokens.unwrap_or(DEFAULT_MAX_TOKENS)),
            // 摘要不需要思考预算，开着只是把钱花在用不上的 token 上。
            thinking: ThinkingConfig::Off,
        };

        let stream = self.provider.stream(request, cancel.clone());
        futures::pin_mut!(stream);

        let mut out = String::new();
        while let Some(ev) = stream.next().await {
            match ev {
                // 只收完整消息，不拼 Delta —— 两个都收会让文本重复一遍。
                ProviderEvent::Message(Message::Assistant { content, .. }) => {
                    for c in content {
                        if let AssistantContent::Text { text } = c {
                            out.push_str(&text);
                        }
                    }
                }
                ProviderEvent::Error(e) => {
                    return Err(WebError::Transport {
                        message: e.to_string(),
                    });
                }
                _ => {}
            }
        }

        if cancel.is_cancelled() {
            return Err(WebError::Cancelled);
        }

        let out = out.trim().to_owned();
        if out.is_empty() {
            // 空摘要不能当成功返回。调用方会拿它替换掉正文，那样用户看到
            // 的是"抓取成功但什么都没有"。报错让调用方降级回截断原文。
            return Err(WebError::Transport {
                message: "辅助模型返回了空内容".to_owned(),
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use riot_protocol::provider::ProviderStream;

    struct Canned(Vec<ProviderEvent>);

    impl Provider for Canned {
        fn stream(&self, _req: ProviderRequest, _cancel: CancellationToken) -> ProviderStream {
            let evs = self.0.clone();
            Box::pin(futures::stream::iter(evs))
        }
        fn count_tokens(&self, _m: &[Message]) -> u32 {
            0
        }
    }

    fn assistant(text: &str) -> ProviderEvent {
        ProviderEvent::Message(Message::Assistant {
            id: MessageId::from_raw("m"),
            content: vec![AssistantContent::Text { text: text.into() }],
            usage: None,
            meta: MessageMeta::default(),
        })
    }

    fn distiller(evs: Vec<ProviderEvent>) -> Distiller {
        Distiller::new(Arc::new(Canned(evs)), "aux".into())
    }

    fn req() -> DistillRequest {
        DistillRequest {
            system: "s".into(),
            user: "u".into(),
            max_output_tokens: None,
        }
    }

    #[tokio::test]
    async fn 取回助手文本() {
        let got = distiller(vec![assistant("  摘要正文  ")])
            .run(req(), &CancellationToken::new())
            .await
            .expect("蒸馏");
        assert_eq!(got, "摘要正文");
    }

    #[tokio::test]
    async fn 空回复报错而不是返回空串() {
        // 空串会被调用方拿去替换正文，用户看到"抓取成功但页面是空的"。
        // 报错才能让调用方降级回截断原文。
        let e = distiller(vec![assistant("   ")])
            .run(req(), &CancellationToken::new())
            .await
            .expect_err("空内容必须报错");
        assert!(matches!(e, WebError::Transport { .. }));
    }

    #[tokio::test]
    async fn provider报错原样传出() {
        let e = distiller(vec![ProviderEvent::Error(
            riot_protocol::provider::ProviderError::Auth {
                message: "key 不对".into(),
            },
        )])
        .run(req(), &CancellationToken::new())
        .await
        .expect_err("必须报错");
        assert!(e.to_string().contains("key 不对"), "{e}");
    }

    #[tokio::test]
    async fn 已取消时报取消() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let e = distiller(vec![assistant("x")])
            .run(req(), &cancel)
            .await
            .expect_err("必须报错");
        assert_eq!(e, WebError::Cancelled);
    }

    #[test]
    fn 蒸馏请求不带任何工具() {
        // 辅助模型读的是不可信的网页正文。给它工具等于给提示注入
        // 一条通向本地执行的路 —— 这条断言是那道门的锁。
        struct Spy(std::sync::Mutex<Option<ProviderRequest>>);
        impl Provider for Spy {
            fn stream(&self, req: ProviderRequest, _c: CancellationToken) -> ProviderStream {
                *self.0.lock().expect("锁") = Some(req);
                Box::pin(futures::stream::iter(vec![assistant("ok")]))
            }
            fn count_tokens(&self, _m: &[Message]) -> u32 {
                0
            }
        }

        let spy = Arc::new(Spy(std::sync::Mutex::new(None)));
        let d = Distiller::new(Arc::clone(&spy) as Arc<dyn Provider>, "aux".into());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        rt.block_on(d.run(req(), &CancellationToken::new())).expect("蒸馏");

        let seen = spy.0.lock().expect("锁").clone().expect("请求");
        assert!(seen.tools.is_empty(), "辅助模型不能拿到任何工具");
        assert_eq!(seen.thinking, ThinkingConfig::Off);
    }
}
