//! 图片怎么交给模型。
//!
//! # 为什么需要这一层
//!
//! 截图这类工具产出的是图片，而"模型能不能收图片"取决于用户接的是哪个服务方
//! —— 而且**不接图片是常态**:多数国内的对话模型（DeepSeek 那一类）是纯文本的。
//!
//! 没有这一层的后果是真实发生过的:图片在 provider 那一层被替换成一句
//! "当前模型不支持图片"，模型读到之后不会放弃，它会自己想办法 —— 去 shell 里
//! `screencapture` 截整个屏幕、用 osascript 找窗口，然后拿着一张截错的图
//! 言之凿凿地分析。看起来像模型笨，根因是能力边界没有被表达出来。
//!
//! # 视觉兼容
//!
//! `[取舍]` 纯文本模型配一个"视觉兼容模型":用一个支持图片的辅助模型把图片
//! 转成结构化文字，再把文字交给主模型。
//!
//! 转述必然有损 —— 辅助模型看到的和描述出来的之间隔着一次压缩。但可选项只有
//! 三个:什么都不给（主模型瞎猜或者去截屏）、报错（那这个工具在半数配置下
//! 等于不存在）、或者给一份有损但可用的描述。第三个最不坏。
//!
//! `[约束]` 转述对主模型要呈现为"你看到的图片内容"，并且明确要求它不向
//! 用户暴露这条管道。早先的做法是标明"这是辅助模型的转述"，结果模型会对
//! 用户坦白"我是通过辅助模型转述看到的，看不清细节" —— 用户明明配好了
//! 视觉兼容，得到的却是一个自称看不了图的助手。有损的部分靠"没提到的
//! 细节不要凭空断言"来拦，而不是靠模型自我声明能力残缺。

use async_trait::async_trait;

/// 请辅助模型描述一张图。
#[derive(Debug, Clone)]
pub struct DescribeRequest {
    /// 图片的 MIME 类型，如 `image/jpeg`。
    pub media_type: String,
    /// base64 编码的图片数据。
    pub data: String,
    /// 想让它重点看什么。空 = 通用描述。
    ///
    /// 调用方（工具）比这一层清楚"为什么要看这张图" —— 是验证布局还是找
    /// 某个按钮，描述的侧重完全不同。
    pub focus: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VisionError {
    /// 没配视觉兼容模型。这不是故障，是"这个功能没开"。
    #[error(
        "当前模型不接受图片，也没有配视觉兼容模型。\
         去设置里给这个服务方打开「支持图片」，或者配一个视觉兼容模型。"
    )]
    NotConfigured,
    #[error("视觉兼容模型调用失败：{message}")]
    Failed { message: String },
    #[error("已取消")]
    Cancelled,
}

/// 图片能力。宿主实现，工具层通过 `ToolContext` 拿到。
#[async_trait]
pub trait VisionAccess: Send + Sync {
    /// 当前对话模型能不能直接收图片。
    ///
    /// `true` 时工具应当原样返回图片内容块 —— 那条路没有信息损失。
    fn accepts_images(&self) -> bool;

    /// 让视觉兼容模型把图片转成文字。
    ///
    /// 返回的文字会**代替**图片交给主模型，所以它必须自带使用指示（由实现
    /// 负责）:当作亲眼所见来回答、不向用户暴露转述管道、没提到的细节不要
    /// 凭空断言。
    async fn describe(&self, req: DescribeRequest) -> Result<String, VisionError>;
}

/// 没有任何图片能力。
///
/// `[约束]` 默认必须是它。装配漏了的表现应该是工具明确说"配一下"，
/// 而不是悄悄按"模型能看图"处理 —— 那会让图片被 provider 层吞掉。
pub struct NoVision;

#[async_trait]
impl VisionAccess for NoVision {
    fn accepts_images(&self) -> bool {
        false
    }

    async fn describe(&self, _req: DescribeRequest) -> Result<String, VisionError> {
        Err(VisionError::NotConfigured)
    }
}
