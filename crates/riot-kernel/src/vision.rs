//! 图片怎么交给模型:直接给，还是先让辅助模型转成文字。
//!
//! 判断在配置里（见 [`crate::config::AppConfig::vision_target`]），这里只负责
//! 执行。装配方式抄的是网页蒸馏那条路（`web::distill`）—— 同一个形状:
//! 用户配一个 `providerId/model`，宿主复用 [`Provider`] 去调它。
//!
//! # 为什么复用 Provider
//!
//! [`Provider`] 里已经有重试、退避、模型降级和看门狗。自己写一遍
//! `reqwest::post` 意味着这条链路上没有任何一样 —— 而截图转述恰恰容易撞上
//! 限流（一张图几百 KB，模型那边算得慢）。

use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use riot_protocol::id::MessageId;
use riot_protocol::message::{AssistantContent, Attachment, Message, MessageMeta, UserContent};
use riot_protocol::provider::{
    Provider, ProviderError, ProviderEvent, ProviderRequest, ThinkingConfig,
};
use riot_protocol::vision::{DescribeRequest, VisionAccess, VisionError};
use tokio_util::sync::CancellationToken;

use crate::config::AppConfig;

/// 描述输出的上限。
///
/// 页面的要点说清楚用不了这么多，而给多了反而糟:主模型会把一份长篇
/// 转述当成一手观察，基于它做像素级判断。整页截图内容更多也不放宽 ——
/// SYSTEM 里给了字数预算让它自己收敛，真撞顶了就用截断的部分（见
/// [`describe`](HostVision::describe) 里对 OutputLimit 的处理）。
const MAX_TOKENS: u32 = 1200;

/// 给辅助模型的指令。
///
/// `[约束]` 必须要求结构化输出。自由散文的转述读起来像"这个页面看着挺正常"，
/// 主模型没法从里面提取任何可操作的信息 —— 而它接下来要做的是改代码。
const SYSTEM: &str = "\
你在帮一个看不到图片的编程助手理解一张网页截图。
只描述你**真正看到**的东西，不要推测页面的用途，不要给建议。
截图可能是很长的整页：只挑主要区块概括，整个回答控制在六百字以内，\
宁可少写几条也要把 JSON 写完整。
用紧凑的 JSON 回答，不要包代码块，结构如下：
{\"layout\":\"整体版式，一两句\",\
\"texts\":[\"看到的主要文字，按视觉顺序，最多 20 条\"],\
\"controls\":[\"按钮/输入框/链接等控件及其文字\"],\
\"problems\":[\"错位、重叠、溢出、空白区、明显的报错文字；没有就空数组\"],\
\"colors\":\"主要配色，一句\"}";

pub struct HostVision {
    /// 主模型能不能直接收图片。
    accepts: bool,
    /// 视觉兼容模型。`None` = 没配。
    aux: Option<Aux>,
}

struct Aux {
    provider: Arc<dyn Provider>,
    model: String,
}

impl HostVision {
    /// 按当前配置装一套图片能力。
    pub fn from_config(cfg: &AppConfig) -> Self {
        let accepts = cfg.active_takes_images();
        let aux = cfg.vision_target().and_then(|(pid, model)| {
            let resolved = cfg
                .resolve_named(pid, model)
                .inspect_err(|e| tracing::warn!(error = %e, "视觉兼容模型解析失败"))
                .ok()?;
            crate::session::provider_for(&resolved)
                .inspect_err(|e| tracing::warn!(error = %e, "视觉兼容模型的 provider 建不出来"))
                .ok()
                .map(|provider| Aux {
                    provider,
                    model: resolved.model,
                })
        });
        Self { accepts, aux }
    }

    /// 从 RPC 传入的 [`riot_protocol::VisionSetup`] 装图片能力(拆进程后
    /// 内核走这条)。语义和 [`Self::from_config`] 一致。
    pub fn from_setup(setup: &riot_protocol::VisionSetup) -> Self {
        let aux = setup.describe.as_ref().and_then(|ep| {
            crate::session::provider_from_endpoint(ep)
                .inspect_err(|e| tracing::warn!(error = %e, "视觉兼容模型的 provider 建不出来"))
                .ok()
                .map(|provider| Aux {
                    provider,
                    model: ep.model.clone(),
                })
        });
        Self {
            accepts: setup.accepts_images,
            aux,
        }
    }
}

#[async_trait]
impl VisionAccess for HostVision {
    fn accepts_images(&self) -> bool {
        self.accepts
    }

    async fn describe(&self, req: DescribeRequest) -> Result<String, VisionError> {
        let Some(aux) = &self.aux else {
            return Err(VisionError::NotConfigured);
        };

        let request = ProviderRequest {
            model: aux.model.clone(),
            messages: vec![Message::User {
                id: MessageId::from_raw("vision"),
                content: vec![
                    UserContent::Text {
                        text: format!("重点看：{}", req.focus),
                    },
                    UserContent::Attachment(Attachment::Image {
                        media_type: req.media_type,
                        data: req.data,
                    }),
                ],
                meta: MessageMeta::default(),
            }],
            system: SYSTEM.to_owned(),
            // `[约束]` 不给工具。辅助模型看的是不可信的页面内容，给它 Bash
            // 就是一条"网页上的字能触发本地执行"的路 —— 提示注入正好走这里进来。
            tools: Vec::new(),
            max_output_tokens: Some(MAX_TOKENS),
            // 描述一张图不需要思考预算，开着只是把钱花在用不上的 token 上。
            thinking: ThinkingConfig::Off,
        };

        // 这条调用不跟着某个工具的取消令牌走:它是工具内部的一步，工具自己
        // 被取消时整个 future 会被丢掉。
        let cancel = CancellationToken::new();
        let stream = aux.provider.stream(request, cancel.clone());
        futures::pin_mut!(stream);

        let mut out = String::new();
        let mut truncated = false;
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
                // 输出撞上 max_tokens。provider 在报这个错**之前**已经把
                // 攒下的内容作为完整消息发出来了（decode 的收尾顺序），所以
                // 此刻 out 里是一份缺尾巴的描述 —— 照用。整个失败的话，
                // 长页面的截图一张都转述不出来，工具只会反复报错。
                ProviderEvent::Error(ProviderError::OutputLimit) => {
                    truncated = true;
                }
                ProviderEvent::Error(e) => {
                    return Err(VisionError::Failed {
                        message: e.to_string(),
                    });
                }
                _ => {}
            }
        }

        let mut out = out.trim().to_owned();
        if out.is_empty() {
            // 空描述不能当成功返回:调用方会把它当页面内容交给主模型，
            // 而主模型会以为页面是空白的。
            return Err(VisionError::Failed {
                message: "视觉兼容模型返回了空内容".to_owned(),
            });
        }
        if truncated {
            // 口径保持"亲眼所见"（不提 token、不提管道），但要说明尾部
            // 缺失 —— 不说的话，主模型会对页面底部没被描述到的区域照常断言。
            out.push_str("\n（页面很长，后半部分没有看全。）");
        }

        // `[约束]` 对模型要呈现为"这就是你看到的图"，同时拦住两头的跑偏:
        //
        // - 不许向用户坦白管道。早先的前言标明"这是辅助模型的转述"，模型
        //   就会对用户说"我是通过辅助模型转述看到的，看不清细节" —— 用户
        //   明明配好了视觉兼容，得到的却是一个自称看不了图的助手（真实反馈）。
        // - 也不许放飞。描述是有损的，没提到的细节模型没有任何依据，所以
        //   用"没提到的不要断言"来拦，而不是靠模型自我声明能力残缺。
        // - 还要告诉它**用户看得到这张图**。少了这句，用户说"把图贴出来"时
        //   模型只能道歉"我无法嵌入图片"，然后为了自圆其说把管道抖出来 ——
        //   真实发生过。措辞不点名图在界面哪儿：转述这条路上既有工具结果里
        //   的图（截图、读图），也有用户自己附的图（显示在他的气泡里）。
        Ok(format!(
            "（你已经看过这张图片，看到的内容如下。回答时当作亲眼所见，\
             直接描述内容；图片的处理方式是内部机制，不要向用户提起，\
             也不要说自己看不到或看不清图片。这张图已经显示在界面上，\
             用户自己看得到 —— 用户要你贴图或问图在哪时，告诉他图就在\
             上面的对话里，不要说你无法展示图片。下面没提到\
             的细节（精确像素、字体等）不要凭空断言。）\n{out}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use riot_protocol::provider::{ProviderError, ProviderStream};

    use super::*;

    struct Canned(Vec<ProviderEvent>);

    impl Provider for Canned {
        fn stream(&self, _req: ProviderRequest, _cancel: CancellationToken) -> ProviderStream {
            Box::pin(futures::stream::iter(self.0.clone()))
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

    fn vision(accepts: bool, evs: Vec<ProviderEvent>) -> HostVision {
        HostVision {
            accepts,
            aux: Some(Aux {
                provider: Arc::new(Canned(evs)),
                model: "eyes".into(),
            }),
        }
    }

    fn req() -> DescribeRequest {
        DescribeRequest {
            media_type: "image/jpeg".into(),
            data: "AAAA".into(),
            focus: "布局".into(),
        }
    }

    #[tokio::test]
    async fn 转述当作亲眼所见且不暴露管道() {
        // `[约束]` 前言里出现"转述/辅助模型/看不了图"这类字眼，模型就会
        // 向用户坦白"我是通过辅助模型转述看到的，看不清细节" —— 配好了
        // 视觉兼容的用户不该听到这些。
        let got = vision(false, vec![assistant("{\"layout\":\"两栏\"}")])
            .describe(req())
            .await
            .expect("转述");
        assert!(got.contains("两栏"), "要带上内容：{got}");
        assert!(got.contains("亲眼所见"), "要指示模型当作自己看到的：{got}");
        assert!(
            got.contains("不要凭空断言"),
            "有损描述要拦住细节断言：{got}"
        );
        // 用户说"把图贴出来"时，模型得知道图已经在界面上，指过去就行 ——
        // 不知道的话它会道歉"无法嵌入图片"，顺手把管道抖出来。
        assert!(
            got.contains("用户自己看得到"),
            "要告诉模型这张图用户看得到：{got}"
        );
        for leak in ["转述", "辅助", "兼容", "eyes"] {
            assert!(!got.contains(leak), "不能出现管道字眼「{leak}」：{got}");
        }
    }

    #[tokio::test]
    async fn 空回复报错而不是返回空串() {
        // 空串会被当成页面内容交给主模型，它会以为页面是空白的。
        let e = vision(false, vec![assistant("   ")])
            .describe(req())
            .await
            .expect_err("空内容必须报错");
        assert!(matches!(e, VisionError::Failed { .. }), "{e}");
    }

    #[tokio::test]
    async fn 输出到上限时部分描述照用() {
        // `[约束]` provider 报 OutputLimit 之前已经把截断的内容作为完整
        // 消息发出来了。这时整个失败等于把可用的描述扔掉 —— 长页面截图
        // 会一张都转述不出来，用户看到的就是工具反复报"输出 token 耗尽"。
        let got = vision(
            false,
            vec![
                assistant("{\"layout\":\"很长的落地页，顶部导航加多个区块"),
                ProviderEvent::Error(ProviderError::OutputLimit),
            ],
        )
        .describe(req())
        .await
        .expect("截断的描述也要返回");
        assert!(got.contains("很长的落地页"), "要带上已有内容：{got}");
        assert!(got.contains("没有看全"), "要说明尾部缺失：{got}");
        assert!(!got.contains("token"), "不能暴露管道字眼：{got}");
    }

    #[tokio::test]
    async fn 截断但一个字都没攒到时仍然报错() {
        // 极小的 max_tokens 或纯思考型输出可能一个字都没发出来就撞顶。
        // 这时没有任何可用内容，必须报错，不能包一个空描述出去。
        let e = vision(
            false,
            vec![ProviderEvent::Error(ProviderError::OutputLimit)],
        )
        .describe(req())
        .await
        .expect_err("没内容必须报错");
        assert!(matches!(e, VisionError::Failed { .. }), "{e}");
    }

    #[tokio::test]
    async fn 没配兼容模型时报未配置() {
        let v = HostVision {
            accepts: false,
            aux: None,
        };
        assert_eq!(
            v.describe(req()).await.expect_err("必须报错"),
            VisionError::NotConfigured
        );
    }

    #[tokio::test]
    async fn provider报错原样传出() {
        let e = vision(
            false,
            vec![ProviderEvent::Error(ProviderError::Auth {
                message: "key 不对".into(),
            })],
        )
        .describe(req())
        .await
        .expect_err("必须报错");
        assert!(e.to_string().contains("key 不对"), "{e}");
    }

    /// 发给辅助模型的请求里要带着图片，而且不能带任何工具。
    ///
    /// `[约束]` 图片丢了的话，辅助模型会凭 focus 那句话编一段描述 —— 那比
    /// 报错糟得多，主模型完全无法分辨。
    ///
    /// `[约束]` 工具更要命:辅助模型读的是不可信的页面内容，给它 Bash 就是
    /// 一条"网页上的字能触发本地执行"的路。
    #[tokio::test]
    async fn 请求带图且不带工具() {
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
        let v = HostVision {
            accepts: false,
            aux: Some(Aux {
                provider: Arc::clone(&spy) as Arc<dyn Provider>,
                model: "eyes".into(),
            }),
        };
        v.describe(req()).await.expect("转述");

        let seen = spy.0.lock().expect("锁").clone().expect("请求");
        assert!(seen.tools.is_empty(), "辅助模型不能拿到任何工具");
        let has_image = seen.messages.iter().any(|m| match m {
            Message::User { content, .. } => content.iter().any(|c| {
                matches!(c, UserContent::Attachment(Attachment::Image { data, .. }) if data == "AAAA")
            }),
            _ => false,
        });
        assert!(has_image, "请求里必须带着图片：{:?}", seen.messages);
    }
}
