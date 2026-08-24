//! 用户输入 → 消息内容的组装：图片（直发/转述/超限拦截）、正文、
//! `@` 引用展开、hook 补充上下文，以及界面占位用的轻量版本。
//!
//! 从 session.rs 拆出来的独立职责：这里只做「把 [`TurnInput`] 变成
//! `Vec<UserContent>`」这一件事，不碰会话状态。会话侧在
//! `Session::mention_ctx` 里备好 [`MentionCtx`]，其余都发生在这里。
//!
//! [`TurnInput`]: crate::session::TurnInput

use riot_protocol::message::{Attachment, UserContent};

use crate::session::TurnInput;

/// 用户随消息附上的一张图。
///
/// 只走内容不走路径:图片可能压根没有路径（从剪贴板粘的截图），而有路径的
/// 那些也要读成 base64 才能进请求 —— 统一成内容，下游少一条分支。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageInput {
    /// MIME 类型，如 `image/png`。
    pub media_type: String,
    /// base64 编码的图片数据。
    pub data: String,
}

/// 读回来的一张图。字段名和 [`ImageInput`] 对齐 —— 前端读完直接原样发回来。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageOutput {
    pub media_type: String,
    pub data: String,
    /// 文件名。界面上给附件条做标签用。
    pub name: String,
}

/// 磁盘上的图片文件读进来的上限（原始字节）。
///
/// base64 之后会涨三分之一，所以这个数要比单图上限小一截。
const MAX_IMAGE_FILE: u64 = 3_500_000;

/// 读一个图片文件。
///
/// `[约束]` 类型按**扩展名**判断，而且只认这几种。不认的一律拒绝 ——
/// 把一个 PDF 当 image/png 发出去，服务方要么 400、要么解出一张坏图，
/// 而报错完全不会指向"类型判错了"。
pub async fn read_image(path: &str) -> Result<ImageOutput, String> {
    let p = std::path::Path::new(path);
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let media_type = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        other => {
            return Err(format!(
                "不支持 .{other} —— 只能附 png / jpg / gif / webp。\
                 其它文件可以用「附加文件」，模型会自己去读。"
            ));
        }
    };

    // 豁免理由：这是宿主层，读的是用户亲手选的那个文件，注入 FileSystem
    // 抽象在这里没有意义（见 clippy.toml 的说明）。
    #[allow(clippy::disallowed_methods)]
    let meta = tokio::fs::metadata(p)
        .await
        .map_err(|e| format!("读不到 {path}：{e}"))?;
    if meta.len() > MAX_IMAGE_FILE {
        return Err(format!(
            "这张图有 {} MB，太大了（上限约 {} MB）。裁剪或缩小之后再附。",
            meta.len() / 1_000_000,
            MAX_IMAGE_FILE / 1_000_000,
        ));
    }

    #[allow(clippy::disallowed_methods)]
    let bytes = tokio::fs::read(p)
        .await
        .map_err(|e| format!("读不到 {path}：{e}"))?;

    use base64::Engine as _;
    Ok(ImageOutput {
        media_type: media_type.to_owned(),
        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
        name: p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("image")
            .to_owned(),
    })
}

/// 单张图的上限（base64 后的长度）。
///
/// 各家服务方对单张图有自己的限制（Anthropic 是 5MB），超了是一个 400。
/// 在这里拦住，用户能立刻知道是哪张图太大 —— 而模型那边报回来的错只会说
/// "请求无效"。前端会先按长边缩一遍，走到这条的多半是超大截图。
const MAX_IMAGE_B64: usize = 5_000_000;

/// 把用户这一轮的输入拼成消息内容。
///
/// `[约束]` 图片排在文字前面。两家服务方的文档都建议这个顺序，实测差别在
/// "先看图再读问题"和"读完问题回头找图"之间 —— 后者更容易答偏。
///
/// `[约束]` 模型收不了图片时必须**在这里**转成文字，不能把图片原样塞进历史。
/// 塞进去的话，OpenAI 那条路会把它发成一条模型看不懂的 image_url（400），
/// Anthropic 那条路会被服务方拒 —— 而两种失败都发生在用户已经按下发送之后。
pub(crate) async fn user_content(
    input: TurnInput,
    vision: &dyn riot_protocol::vision::VisionAccess,
    mentions: MentionCtx<'_>,
) -> Vec<UserContent> {
    let mut content = Vec::with_capacity(input.images.len() + 1);

    // `[约束]` 转述和各类说明用 SystemReminder 附件，不用 Text。
    //
    // 这些话是宿主替模型补的上下文，不是用户说的:混进 Text 的话，
    // 前端重建历史时会把整段转述当成用户气泡显示出来（实时路径看不到
    // 这个问题 —— 乐观回显只显示用户真正打的字，切回会话才暴露）。
    // 模型侧则两条路都读得到，SystemReminder 还多了"这是带外提示"的语义。
    for (i, img) in input.images.into_iter().enumerate() {
        if img.data.len() > MAX_IMAGE_B64 {
            content.push(UserContent::Attachment(Attachment::SystemReminder {
                text: format!(
                    "用户附了第 {} 张图，但它有 {} KB，超过单张上限，没有发给你。\
                     可以请用户裁剪或缩小之后再发。",
                    i + 1,
                    img.data.len() / 1024,
                ),
            }));
            continue;
        }

        if vision.accepts_images() {
            content.push(UserContent::Attachment(Attachment::Image {
                media_type: img.media_type,
                data: img.data,
            }));
            continue;
        }

        // 走视觉兼容。失败也要留一句话 —— 静默丢掉的话，用户明明附了图，
        // 模型却完全不知道有这回事，然后答得像用户什么都没给。
        //
        // 转述进 DescribedImage 而不是 SystemReminder：模型那边两者一样（都
        // 只读文字，provider 不发图），但前者把图片本体留在了消息里，界面
        // 切回会话时还能把用户发过的图画出来。
        let described = vision
            .describe(riot_protocol::vision::DescribeRequest {
                media_type: img.media_type.clone(),
                data: img.data.clone(),
                focus: "用户附上这张图是想让你看懂它的内容:上面的文字、界面元素、\
                        数据、以及任何看起来是报错的地方"
                    .to_owned(),
            })
            .await;
        content.push(UserContent::Attachment(Attachment::DescribedImage {
            media_type: img.media_type,
            data: img.data,
            text: match described {
                Ok(desc) => format!("用户附的第 {} 张图：\n{desc}", i + 1),
                Err(e) => format!("用户附了第 {} 张图，但没能转成文字：{e}", i + 1),
            },
        }));
    }

    let text_for_mentions = input.text.clone();
    content.push(UserContent::Text {
        text: prompt_text(&input.text),
    });
    // `@路径` 引用：用户点名的文件连内容一起带上，排在正文之后
    //（先读问题再看材料 —— 和图片相反，图片是"看着图听问题"）。
    // 两路来源：正文里手打的 @，和界面上选中的块。
    let refs = crate::mentions::merge(
        crate::mentions::parse(&text_for_mentions, mentions.cwd),
        crate::mentions::from_paths(&input.refs, mentions.cwd),
    );
    if !refs.is_empty() {
        tracing::info!(count = refs.len(), "展开 @ 文件引用");
        content.extend(
            crate::mentions::expand(&refs, mentions.file_state)
                .into_iter()
                .map(UserContent::Attachment),
        );
    }

    // UserPromptSubmit hook 的补充上下文排在最后 —— 它是对这条消息的
    // 注解，不是消息本身。
    for ctx in input.extra_context {
        content.push(UserContent::Attachment(Attachment::SystemReminder {
            text: format!("UserPromptSubmit hook 的补充上下文：\n{ctx}"),
        }));
    }
    content
}

/// 用户这条消息的正文。
///
/// 空文本也要留个位置:用户可能只丢了一张图什么都没说，而空的 user 消息
/// 会被一部分服务方拒。
fn prompt_text(text: &str) -> String {
    if text.trim().is_empty() {
        "看这张图。".to_owned()
    } else {
        text.to_owned()
    }
}

/// 用户这一轮输入的**占位形态**：只有他真正打的字、附的图和点名的文件。
///
/// [`user_content`] 要调模型（图片转述）、读磁盘（`@` 展开）才能定稿，
/// 期间界面得先有东西可画（见 `Session::pending_user`）。缺的是那些
/// 本来就是补给模型的上下文 —— 用户自己在气泡里看到的就是这些。
///
/// `[约束]` 图片一律进 `DescribedImage`。这份内容按设计不会发给模型
/// （定稿版会整条顶掉它），但万一漏出去，这个变体只会渲染成一段文字 ——
/// 而 `Image` 会让收不了图的模型 400，正是 [`user_content`] 要避免的那个。
///
/// `[约束]` `@` 引用块也要占位。展开要读盘，切会话再回来如果只有正文、
/// 没有 `UserFile`，前端曾经只能画出一串 `@路径` 纯文字（实时路径靠乐观
/// 回显还看得见）。内容先空着 —— 定稿版会整条顶掉。
pub(crate) fn pending_user_content(input: &TurnInput) -> Vec<UserContent> {
    let mut content: Vec<UserContent> = input
        .images
        .iter()
        .enumerate()
        // 超限的图定稿时也不会带上，占位这里同样跳过 —— 它进不了历史，
        // 却要跟着每次拉历史在 RPC 上来回搬几 MB。
        .filter(|(_, img)| img.data.len() <= MAX_IMAGE_B64)
        .map(|(i, img)| {
            UserContent::Attachment(Attachment::DescribedImage {
                media_type: img.media_type.clone(),
                data: img.data.clone(),
                text: format!("用户附的第 {} 张图，还在读。", i + 1),
            })
        })
        .collect();
    content.push(UserContent::Text {
        text: prompt_text(&input.text),
    });
    for p in &input.refs {
        if p.trim().is_empty() {
            continue;
        }
        content.push(UserContent::Attachment(Attachment::UserFile {
            path: std::path::PathBuf::from(p),
            content: String::new(),
        }));
    }
    content
}

/// `@` 引用展开要用的东西：解析相对路径的基准 + 工作集登记。
///
/// `file_state` 为 None 时不登记（测试）。登记之后模型能直接 Edit
/// 引用过的文件，不用先 Read 一遍。
#[derive(Clone, Copy)]
pub(crate) struct MentionCtx<'a> {
    pub(crate) cwd: &'a std::path::Path,
    pub(crate) file_state: Option<&'a dyn riot_protocol::tool::FileStateCache>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 不解析 @ 引用的上下文（图片相关的用例不碰文件）。cwd 指一个
    /// 不存在的目录，万一测试文本里出现 @ 也读不到东西。
    fn no_mentions() -> MentionCtx<'static> {
        MentionCtx {
            cwd: std::path::Path::new("/nonexistent-mentions"),
            file_state: None,
        }
    }

    /// 图片能看的模型:图片原样进消息，而且排在文字前面。
    #[tokio::test]
    async fn 能看图时图片在文字前面() {
        struct Direct;
        #[async_trait::async_trait]
        impl riot_protocol::vision::VisionAccess for Direct {
            fn accepts_images(&self) -> bool {
                true
            }
            async fn describe(
                &self,
                _r: riot_protocol::vision::DescribeRequest,
            ) -> Result<String, riot_protocol::vision::VisionError> {
                panic!("能看图就不该来转述")
            }
        }

        let content = user_content(
            TurnInput {
                text: "这里为什么错位".into(),
                images: vec![ImageInput {
                    media_type: "image/png".into(),
                    data: "AAAA".into(),
                }],
                ..Default::default()
            },
            &Direct,
            no_mentions(),
        )
        .await;

        assert!(
            matches!(
                content.first(),
                Some(UserContent::Attachment(Attachment::Image { data, .. })) if data == "AAAA"
            ),
            "图片该排在最前：{content:?}"
        );
        assert!(matches!(
            content.last(),
            Some(UserContent::Text { text }) if text == "这里为什么错位"
        ));
    }

    /// 看不了图的模型:图片转成文字，**不能**把图片当 `Image` 留在消息里。
    ///
    /// `[约束]` 留在里面的话，OpenAI 那条路会发出一条模型看不懂的 image_url，
    /// Anthropic 那条会被服务方拒 —— 而两种失败都发生在用户已经点了发送之后。
    ///
    /// `[约束]` 但图片本体要留在 `DescribedImage` 里给界面。丢掉的话，用户
    /// 切走再切回来自己发过的图就没了（实时路径靠乐观回显看不出这个问题）。
    #[tokio::test]
    async fn 看不了图时转成文字() {
        struct Compat;
        #[async_trait::async_trait]
        impl riot_protocol::vision::VisionAccess for Compat {
            fn accepts_images(&self) -> bool {
                false
            }
            async fn describe(
                &self,
                _r: riot_protocol::vision::DescribeRequest,
            ) -> Result<String, riot_protocol::vision::VisionError> {
                Ok("图里是一个两栏布局".into())
            }
        }

        let content = user_content(
            TurnInput {
                text: String::new(),
                images: vec![ImageInput {
                    media_type: "image/png".into(),
                    data: "AAAA".into(),
                }],
                ..Default::default()
            },
            &Compat,
            no_mentions(),
        )
        .await;

        assert!(
            !content
                .iter()
                .any(|c| matches!(c, UserContent::Attachment(Attachment::Image { .. }))),
            "不能把图片当 Image 留在消息里：{content:?}"
        );
        // 转述进附件而不是 Text:它是宿主补的上下文，不是用户说的话。混进
        // Text 的话，前端重建历史时会把整段转述当成用户气泡显示出来。
        // 用 DescribedImage 而不是 SystemReminder:图片本体得跟着留下来。
        assert!(
            content.iter().any(|c| matches!(
                c,
                UserContent::Attachment(Attachment::DescribedImage { text, data, .. })
                    if text.contains("两栏") && data == "AAAA"
            )),
            "转述和图片本体要一起以 DescribedImage 附件带上：{content:?}"
        );
        // 只丢了图什么都没说时，也得有一句话 —— 空 user 消息会被一部分
        // 服务方拒。
        assert!(
            content.iter().any(|c| matches!(
                c, UserContent::Text { text } if text.contains("看这张图")
            )),
            "空文本要补一句：{content:?}"
        );
    }

    /// 超大图不发出去，但要告诉模型"有这么回事"。
    #[tokio::test]
    async fn 超大图被拦下并留一句说明() {
        struct Direct;
        #[async_trait::async_trait]
        impl riot_protocol::vision::VisionAccess for Direct {
            fn accepts_images(&self) -> bool {
                true
            }
            async fn describe(
                &self,
                _r: riot_protocol::vision::DescribeRequest,
            ) -> Result<String, riot_protocol::vision::VisionError> {
                unreachable!()
            }
        }

        let content = user_content(
            TurnInput {
                text: "看图".into(),
                images: vec![ImageInput {
                    media_type: "image/png".into(),
                    data: "x".repeat(MAX_IMAGE_B64 + 1),
                }],
                ..Default::default()
            },
            &Direct,
            no_mentions(),
        )
        .await;

        assert!(
            !content
                .iter()
                .any(|c| matches!(c, UserContent::Attachment(Attachment::Image { .. }))),
            "超限的图不该发出去"
        );
        assert!(
            content.iter().any(|c| matches!(
                c,
                UserContent::Attachment(Attachment::SystemReminder { text })
                    if text.contains("超过单张上限")
            )),
            "要留一句说明，否则模型以为用户什么都没给：{content:?}"
        );
    }

    /// 只认几种图片扩展名。
    ///
    /// `[约束]` 把 PDF 当 image/png 发出去，服务方要么 400、要么解出一张
    /// 坏图，而报错完全不指向"类型判错了"。
    #[tokio::test]
    async fn 不认识的扩展名直接拒() {
        let e = read_image("/tmp/whatever.pdf").await.expect_err("该拒");
        assert!(e.contains("png"), "报错要说清能附什么：{e}");
        // 文件压根不存在也是这个结论 —— 扩展名先判，省一次磁盘访问。
        assert!(!e.contains("读不到"), "不该先去读盘：{e}");
    }

    /// 占位内容 = 用户真正打的字 + 他附的图 + 点名的文件。
    #[tokio::test]
    async fn 占位带上正文和图片但跳过超限的图() {
        let content = pending_user_content(&TurnInput {
            text: "这里为什么错位".into(),
            images: vec![
                ImageInput {
                    media_type: "image/png".into(),
                    data: "AAAA".into(),
                },
                ImageInput {
                    media_type: "image/png".into(),
                    data: "x".repeat(MAX_IMAGE_B64 + 1),
                },
            ],
            ..Default::default()
        });

        // `[约束]` 图片进 DescribedImage 而不是 Image：占位按设计不发给
        // 模型，但万一漏出去，Image 会让收不了图的模型 400。
        let imgs: Vec<&str> = content
            .iter()
            .filter_map(|c| match c {
                UserContent::Attachment(Attachment::DescribedImage { data, .. }) => {
                    Some(data.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(imgs, ["AAAA"], "超限的图不该跟着占位来回搬：{content:?}");
        assert!(
            content.iter().any(|c| matches!(
                c, UserContent::Text { text } if text == "这里为什么错位"
            )),
            "正文要原样带上：{content:?}"
        );
    }

    #[tokio::test]
    async fn 占位带上引用文件以免切会话丢块() {
        let content = pending_user_content(&TurnInput {
            text: "读下内容".into(),
            refs: vec!["/tmp/a.xlsx".into(), "  ".into()],
            ..Default::default()
        });
        let paths: Vec<String> = content
            .iter()
            .filter_map(|c| match c {
                UserContent::Attachment(Attachment::UserFile { path, content }) => {
                    assert!(content.is_empty(), "占位不读盘，内容该空：{content}");
                    Some(path.to_string_lossy().into_owned())
                }
                _ => None,
            })
            .collect();
        assert_eq!(paths, ["/tmp/a.xlsx"], "空路径不该占一条：{content:?}");
    }

    /// 只丢了一张图什么都没说时，占位的正文和定稿版保持一致。
    /// 两边对不上的话，前端按正文去重的那步会把气泡显示成两条。
    #[tokio::test]
    async fn 空文本的占位和定稿用同一句话() {
        let input = TurnInput {
            text: "  ".into(),
            images: vec![ImageInput {
                media_type: "image/png".into(),
                data: "AAAA".into(),
            }],
            ..Default::default()
        };
        let pending = pending_user_content(&input);
        let final_content =
            user_content(input, &riot_protocol::vision::NoVision, no_mentions()).await;

        let text_of = |c: &[UserContent]| {
            c.iter()
                .find_map(|x| match x {
                    UserContent::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .expect("总要有一句正文")
        };
        assert_eq!(text_of(&pending), text_of(&final_content));
    }
}
