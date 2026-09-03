//! LLM 结构化总结：压缩阶梯的最重一档。
//!
//! # 设计（对照 Claude Code 的 services/compact）
//!
//! - **九节式总结提示词**：主要意图 / 技术概念 / 文件与代码 / 错误与修复 /
//!   问题解决 / 全部用户原话 / 待办 / 当前工作 / 下一步。九节里最关键的是
//!   「全部用户原话」—— 意图漂移是压缩最大的风险，原话是锚。
//! - **analysis / summary 分离**：先让模型在 `<analysis>` 里自查遗漏，再产出
//!   `<summary>`。入库前剥掉 analysis —— 它是脚手架，不是产物。
//! - **同形状请求吃前缀缓存**：能拿到主循环请求形状（[`RequestShape`]）时，
//!   总结请求带同样的 system 和 tools、消息原样不动 —— provider 的前缀缓存
//!   按 tools → system → messages 逐字节匹配，同形状的话 ~100k token 的历史
//!   几乎全走 cache_read（价格约 1/10，prefill 快一个量级）。禁工具靠提示词
//!   前后夹击 + 失败防线（模型真调了工具 → 没有 summary → 报失败，不写坏
//!   任何东西）。拿不到形状的调用方（手动 /compact）退回瘦身路径：图片换
//!   占位、工具块转文本、不带 tools。
//! - **续接包装**：总结以合成 user 消息回到历史，措辞要求模型"像中断从未
//!   发生过一样继续"，不要复述、不要再次确认。
//!
//! # 职责边界
//!
//! 这里只有**纯函数**和一次 provider 调用的封装。什么时候压、压完的历史
//! 归谁、工作集怎么重注入 —— 反应式路径归 [`crate::compactor::Layered`]，
//! 主动式路径归宿主（历史的持久所有权在那边）。

use std::sync::Arc;

use futures::StreamExt;
use riot_protocol::message::{
    AssistantContent, Attachment, Message, MessageMeta, ToolResultContent, UserContent,
};
use riot_protocol::provider::{Provider, ProviderEvent, ProviderRequest, ThinkingConfig, ToolSpec};
use tokio_util::sync::CancellationToken;

/// 主循环请求的形状：system 和 tools 原样一份。
///
/// 总结请求带上它，前缀（tools → system → messages）就和主循环上一次请求
/// 逐字节一致，provider 的 prompt cache 直接命中 —— 压缩的输入恰恰是主循环
/// 刚发过的那 ~100k token。形状差一点（精简 system、去掉 tools、改写消息）
/// 都会让这部分每次全量重算：时间多十几秒，费用差约 10 倍。
///
/// `[约束]` 两个字段必须**原样**取自本轮主循环的请求，不能"看起来差不多"。
/// 缓存是字节级前缀匹配，差一个字节，之后全部 miss 且没有任何报错 ——
/// 表现只是"压缩怎么又变慢了"。
///
/// 开着 thinking 的会话，总结请求（thinking off）与主循环的 thinking 配置
/// 不同，messages 层缓存照样 miss（Anthropic 把 thinking 配置渲染进 prompt）。
/// 这是已接受的退化：不比不带形状更差，且总结任务本就不该开思考。
#[derive(Debug, Clone)]
pub struct RequestShape {
    pub system: String,
    pub tools: Vec<ToolSpec>,
}

/// 总结输出的预算。CC 用 20k；对齐它 —— 长会话的九节总结真能写到
/// 上万 token，砍太狠丢的是"文件与代码段"那节的完整片段。撞上这个
/// 预算时 provider 会报 OutputLimit，总结按失败处理而不是静默截尾
/// （九节顺序输出，截掉的恰是用户原话/待办/下一步那几节）。
const SUMMARY_MAX_OUTPUT_TOKENS: u32 = 20_000;

/// 总结提示词。九节结构和 CC 逐节对应，措辞按中文对话习惯改写。
const COMPACT_PROMPT: &str = "\
重要：只输出文本，不要调用任何工具。你需要的全部上下文都已经在上面的对话里。\n\n\
你的任务：为到目前为止的对话写一份**详尽**的总结，重点保住用户的明确要求和你已经做过的动作。\
这份总结将替代完整历史供后续继续工作使用 —— 漏掉的信息就永远丢了。\n\n\
先在 <analysis> 标签里按时间顺序过一遍对话，自查每一段的：用户意图、你的做法、关键决策、\
具体细节（文件名、完整代码片段、函数签名、文件修改）、踩过的错和修法、用户的反馈（尤其是纠正你的话）。\n\n\
然后在 <summary> 标签里输出以下九节：\n\
1. 主要请求与意图：用户的每一个明确要求，写细。\n\
2. 关键技术概念：涉及的技术、框架、约定。\n\
3. 文件与代码段：看过/改过/新建的文件，为什么重要，关键代码片段要完整摘录（最近改动优先）。\n\
4. 错误与修复：踩过的每个错、怎么修的、用户对此的反馈。\n\
5. 问题解决：已解决的问题和仍在排查的思路。\n\
6. 全部用户原话：列出**所有**非工具结果的用户消息原文 —— 这是防止意图漂移的锚，一条都不能少。\n\
7. 待办事项：明确被要求、还没完成的事。\n\
8. 当前工作：总结前的那一刻正在做什么，文件和代码要具体。\n\
9. 下一步（可选）：与最近工作直接相关的下一步。必须和用户最近的明确要求一致；\
   如果上一件事已经收尾，没有新指示就不要发明下一步。引用最近对话的原话来说明接续点。\n\n\
提醒：只输出 <analysis> 和 <summary> 两个块，不要调用工具。";

/// 总结请求的 system prompt。刻意简短 —— 总结靠上面的用户消息驱动，
/// system 只定角色。
const SUMMARY_SYSTEM: &str = "你是负责精确总结对话的助手。你只输出文本，从不调用工具。";

/// 调 LLM 总结一段历史。返回剥好的总结正文。
///
/// `shape` 是本轮主循环请求的形状（见 [`RequestShape`]）：给了就发同形状
/// 请求吃前缀缓存 —— 消息**原样**（图片、工具块都不动，动一个字节缓存就
/// 断在那里）；没给（手动 /compact 拿不到轮次装配）退回瘦身路径。
///
/// 失败返回人话（进日志/熔断计数用）。取消时返回 Err —— 调用方本来
/// 就在被取消的路径上，任何返回值都不会被用。
pub async fn summarize_history(
    provider: &Arc<dyn Provider>,
    model: &str,
    messages: &[Message],
    shape: Option<&RequestShape>,
    cancel: CancellationToken,
) -> Result<String, String> {
    let mut request_messages = match shape {
        Some(_) => messages
            .iter()
            .filter(|m| m.goes_to_model())
            .cloned()
            .collect(),
        None => strip_for_summary(messages),
    };
    request_messages.push(Message::User {
        id: riot_protocol::id::MessageId::from_raw("msg_compact_prompt"),
        content: vec![UserContent::Text {
            text: COMPACT_PROMPT.into(),
        }],
        meta: MessageMeta {
            synthetic: true,
            ..Default::default()
        },
    });

    let request = ProviderRequest {
        model: model.to_owned(),
        messages: request_messages,
        system: match shape {
            Some(s) => s.system.clone(),
            None => SUMMARY_SYSTEM.into(),
        },
        // 同形状路径带主循环的 tools —— 不是为了让它调（提示词前后夹击
        // 禁着），是为了前缀缓存：tools 是缓存层级的第一层，抽掉它整个
        // 请求从第 0 字节开始 miss。刻意不设 tool_choice 之类的硬禁 ——
        // Anthropic 对 tool_choice 变化的处理是把 messages 层缓存作废，
        // 等于为了防一个低概率事件放弃全部收益。模型真调了工具：没有
        // summary → 下面报失败，有防线。
        tools: match shape {
            Some(s) => s.tools.clone(),
            None => Vec::new(),
        },
        max_output_tokens: Some(SUMMARY_MAX_OUTPUT_TOKENS),
        thinking: ThinkingConfig::Off,
    };

    let mut stream = provider.stream(request, cancel);
    let mut text = String::new();
    let mut called_tools = false;
    while let Some(ev) = stream.next().await {
        match ev {
            ProviderEvent::Message(Message::Assistant { content, .. }) => {
                for c in content {
                    match c {
                        AssistantContent::Text { text: t } => text.push_str(&t),
                        AssistantContent::ToolUse { .. } => called_tools = true,
                        AssistantContent::Thinking { .. } => {}
                    }
                }
            }
            ProviderEvent::Error(e) => return Err(format!("总结请求失败：{e}")),
            _ => {}
        }
    }

    let summary = extract_summary(&text);
    if summary.trim().is_empty() {
        // 分开报：两种失败的处置不同 ——"调了工具"该查提示词/换模型，
        // "没产出"多半是截断或空响应。
        if called_tools {
            return Err("总结模型调用了工具而不是输出总结".into());
        }
        return Err("总结模型没有产出 <summary> 内容".into());
    }
    Ok(summary)
}

/// 把总结包成续接消息（合成 user 消息），作为压缩后历史的开头。
///
/// `memory` 是重新注入的记忆附件（压缩把带着记忆的首条消息一起吞了，
/// 不重注的话项目约定就此消失）；`restored` 是工作集文件。
///
/// `archive` 是被压掉的原文落成的文件（[`crate::archive`]）。给了就在
/// 续接消息里指路：总结是有损的，模型要报错原文、具体路径、用户某句话
/// 的时候，有地方可查比靠猜好。宿主写不出文件时传 None，措辞退回
/// "只有总结"。
pub fn continuation_message(
    summary: &str,
    memory: Vec<Attachment>,
    restored: Vec<Attachment>,
    archive: Option<&std::path::Path>,
    id: riot_protocol::id::MessageId,
) -> Message {
    let mut content: Vec<UserContent> = memory.into_iter().map(UserContent::Attachment).collect();
    let archive_note = match archive {
        Some(p) => format!(
            "\n\n被压缩的对话**原文**保存在 `{}`（一条消息一个 `## [序号] 角色` 小节，\
             工具结果只留开头）。总结里没有、但你需要的细节 —— 报错原文、具体路径、\
             命令输出、用户某句话的准确措辞 —— 用 Grep 搜关键词或 Read 指定行区间去查，\
             不要靠猜、也不要整份读进来。",
            p.display()
        ),
        None => String::new(),
    };
    content.push(UserContent::Text {
        text: format!(
            "本会话由一段更早的对话延续而来，先前内容已压缩。以下是前文的完整总结：\n\n{summary}\
             {archive_note}\n\n\
             直接接着做，不要复述总结、不要向用户再次确认、不要说「我将继续」—— \
             像中断从未发生过一样，接上手头的任务。",
        ),
    });
    content.extend(restored.into_iter().map(UserContent::Attachment));
    Message::User {
        id,
        content,
        meta: MessageMeta {
            synthetic: true,
            ..Default::default()
        },
    }
}

/// 压缩时原样保留的尾巴最多多大。
///
/// 总结负责"前面发生过什么"，尾巴负责"刚刚在做什么"—— 最近一轮的原话
/// 和工具结果是模型接续时最常回看的东西，让它从总结里重新拼太亏。
/// 20k 约是默认阈值（300k）的 7%：够装下一轮普通对话，装不下一轮跑了
/// 几十个工具的长任务（那种尾巴留着等于没压）。
pub const MAX_TAIL_TOKENS: u32 = 20_000;

/// 压缩的切分点：`messages[..split]` 送去总结，`messages[split..]` 原样保留。
///
/// 只在**用户提问**处切（[`Message::is_user_prompt`]）：一轮问答从提问
/// 开始、到下一条提问前结束，工具调用和结果总在轮内，所以在这里切不会
/// 拆开配对。取最后一条提问；它引出的那一轮超过 `max_tail_tokens` 就不
/// 留尾巴（返回 `len`），首条提问也不算（那等于什么都不压）。
///
/// `[约束]` 切分是纯函数、只看消息本身。后台预压缩和开工时的换入必须算
/// 出同一个切点，否则总结的是一段、留下的是另一段 —— 中间要么重复要么
/// 缺一截，两者都没有报错。
pub fn split_point(
    messages: &[Message],
    count_tokens: impl Fn(&[Message]) -> u32,
    max_tail_tokens: u32,
) -> usize {
    let Some(last_prompt) = messages.iter().rposition(Message::is_user_prompt) else {
        return messages.len();
    };
    if last_prompt == 0 || count_tokens(&messages[last_prompt..]) > max_tail_tokens {
        return messages.len();
    }
    last_prompt
}

/// 剥出 `<summary>` 正文；没有闭合标签时取开标签之后的全部（流被截断
/// 的常见形态 —— 内容仍然可用，别因为缺一个标签丢掉整份总结）。
/// 两个标签都没有时原样返回（有些模型不吐标签直接给正文）。
pub fn extract_summary(raw: &str) -> String {
    let after_analysis = match raw.find("</analysis>") {
        Some(i) => &raw[i + "</analysis>".len()..],
        None => raw,
    };
    let body = match after_analysis.find("<summary>") {
        Some(i) => {
            let s = &after_analysis[i + "<summary>".len()..];
            match s.find("</summary>") {
                Some(j) => &s[..j],
                None => s,
            }
        }
        None => {
            // 没有 summary 标签但有 analysis：analysis 是脚手架不是产物，
            // 只剩它的话等于没总结出来。
            if raw.contains("<analysis>") && !raw.contains("<summary>") {
                return String::new();
            }
            after_analysis
        }
    };
    body.trim().to_owned()
}

/// 为总结请求瘦身：图片换占位符（视觉内容进不了文本总结，白占请求体），
/// tool_use / tool_result 块降级成纯文本，思考块丢弃。
///
/// **只用于拿不到 [`RequestShape`] 的退回路径**（手动 /compact）。同形状
/// 路径的消息必须原样发 —— 前缀缓存按字节匹配，这里的每一处改写在那条
/// 路上都是缓存杀手。
///
/// `[约束]` 工具块必须**转成文字而不是保留**。这条路径的请求不带 `tools`，而
/// Anthropic 对"消息里有 tool_use/tool_result 块但请求没定义 tools"直接 400
/// （"Requests which include `tool_use` or `tool_result` blocks must define
/// tools"）—— 带工具调用的会话（几乎所有真实会话）总结必失败，压缩阶梯的
/// 重档等于不存在。OpenAI 主线容忍，但严格校验的兼容后端一样会拒。
/// 转成文字：块类型消失，信息留下。
///
/// 思考块直接丢：它是脚手架不是事实，而且 Anthropic 的签名与模型绑定、
/// 要求原样回传 —— 总结模型和会话模型可能不是同一个（INV-9 同源）。
fn strip_for_summary(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .filter(|m| m.goes_to_model())
        .map(|m| match m {
            Message::User { id, content, meta } => Message::User {
                id: id.clone(),
                content: content
                    .iter()
                    .map(|c| match c {
                        UserContent::Attachment(Attachment::Image { .. }) => {
                            UserContent::Attachment(Attachment::SystemReminder {
                                text: "[此处原本是一张图片，总结时已省略]".into(),
                            })
                        }
                        // 视觉兼容那张图：转述留着（模型本来读的就是它，
                        // 丢掉等于让总结忘掉用户附过什么），base64 去掉。
                        UserContent::Attachment(Attachment::DescribedImage { text, .. }) => {
                            UserContent::Attachment(Attachment::SystemReminder {
                                text: text.clone(),
                            })
                        }
                        UserContent::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => UserContent::Text {
                            text: format!(
                                "[工具调用 {} 的结果{}]\n{}",
                                tool_use_id.as_str(),
                                if *is_error { "（失败）" } else { "" },
                                tool_result_text(content),
                            ),
                        },
                        other => other.clone(),
                    })
                    .collect(),
                meta: meta.clone(),
            },
            Message::Assistant {
                id,
                content,
                usage,
                meta,
            } => Message::Assistant {
                id: id.clone(),
                content: content
                    .iter()
                    .filter_map(|c| match c {
                        AssistantContent::Text { .. } => Some(c.clone()),
                        AssistantContent::ToolUse { id, name, input } => {
                            Some(AssistantContent::Text {
                                text: format!(
                                    "[调用工具 {name}（{}），参数：{}]",
                                    id.as_str(),
                                    serde_json::to_string(input).unwrap_or_else(|_| "{}".into()),
                                ),
                            })
                        }
                        AssistantContent::Thinking { .. } => None,
                    })
                    .collect(),
                usage: *usage,
                meta: meta.clone(),
            },
            other => other.clone(),
        })
        .collect()
}

/// tool_result 内容的模型可见文字形态。措辞和 wire 层各家的转换同一个口径，
/// 总结读到的和会话模型当时读到的尽量一致。
fn tool_result_text(c: &ToolResultContent) -> String {
    match c {
        ToolResultContent::Text { text } => text.clone(),
        ToolResultContent::Spilled {
            path,
            preview,
            total_bytes,
        } => format!(
            "结果过大（{total_bytes} 字节），已写入 {}。\n预览：\n{preview}",
            path.display()
        ),
        ToolResultContent::Cleared => "[结果已清理以节省上下文]".into(),
        ToolResultContent::Image { .. } => "[图片结果，总结时已省略]".into(),
        // 转述是模型本来就在读的内容，保留；图片本体进不了文本总结。
        ToolResultContent::DescribedImage { text, .. } => text.clone(),
        ToolResultContent::MarkedImage { text, .. } => {
            format!("{text}\n[图片本体总结时已省略]")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use riot_protocol::id::MessageId;

    #[test]
    fn 剥掉_analysis_取出_summary() {
        let raw =
            "<analysis>我想想……这里有三段对话</analysis>\n<summary>1. 主要请求：修 bug</summary>";
        assert_eq!(extract_summary(raw), "1. 主要请求：修 bug");
    }

    #[test]
    fn 缺闭合标签时取开标签之后的全部() {
        // 流被截断是常见形态。内容仍然可用，别因为缺一个标签丢掉整份总结。
        let raw = "<analysis>x</analysis><summary>总结正文被截断在这";
        assert_eq!(extract_summary(raw), "总结正文被截断在这");
    }

    #[test]
    fn 没有任何标签时原样返回() {
        assert_eq!(extract_summary("  直接给正文的模型  "), "直接给正文的模型");
    }

    #[test]
    fn 只有_analysis_没有_summary_算失败() {
        // analysis 是脚手架不是产物，只剩它等于没总结出来 ——
        // 拿它当总结会把模型的自言自语灌进后续所有轮次。
        assert_eq!(extract_summary("<analysis>只想了没总结</analysis>"), "");
    }

    #[test]
    fn 续接消息带记忆和工作集_文本居中() {
        let m = continuation_message(
            "九节总结",
            vec![Attachment::Memory {
                path: "/p/AGENTS.md".into(),
                content: "约定".into(),
            }],
            vec![Attachment::RestoredFile {
                path: "/p/a.rs".into(),
                content: "code".into(),
            }],
            None,
            MessageId::from_raw("m1"),
        );
        let Message::User { content, meta, .. } = &m else {
            panic!("是 user")
        };
        assert!(
            meta.synthetic,
            "合成消息要打标，UI 靠它区分于用户亲口说的话"
        );
        assert!(
            matches!(
                &content[0],
                UserContent::Attachment(Attachment::Memory { .. })
            ),
            "记忆在最前 —— 压缩把带着记忆的首条消息吞了，必须重注"
        );
        assert!(matches!(&content[1], UserContent::Text { text } if text.contains("九节总结")));
        assert!(
            matches!(&content[1], UserContent::Text { text } if text.contains("像中断从未发生过")),
            "续接指令必须在：没有它模型会先复述一遍总结再开工"
        );
        assert!(matches!(
            content.last(),
            Some(UserContent::Attachment(Attachment::RestoredFile { .. }))
        ));
        assert!(
            !matches!(&content[1], UserContent::Text { text } if text.contains("原文")),
            "没有归档文件就别提它，否则模型会去找一个不存在的路径"
        );
    }

    #[test]
    fn 续接消息给出归档路径和用法() {
        let m = continuation_message(
            "总结",
            Vec::new(),
            Vec::new(),
            Some(std::path::Path::new("/art/s1/history.md")),
            MessageId::from_raw("m1"),
        );
        let Message::User { content, .. } = &m else {
            panic!("是 user")
        };
        let UserContent::Text { text } = &content[0] else {
            panic!("文本")
        };
        assert!(text.contains("/art/s1/history.md"), "{text}");
        assert!(
            text.contains("Grep") && text.contains("不要靠猜"),
            "要教模型怎么用：搜关键词，别整份读：{text}"
        );
    }

    /// 切分只在用户提问处，且尾巴有预算。
    #[test]
    fn 切分点落在最后一条提问_超预算则不留尾巴() {
        use riot_protocol::id::ToolUseId;
        let prompt = |id: &str, t: &str| Message::User {
            id: MessageId::from_raw(id),
            content: vec![UserContent::Text { text: t.into() }],
            meta: MessageMeta::default(),
        };
        let result = |id: &str, t: &str| Message::User {
            id: MessageId::from_raw(id),
            content: vec![UserContent::ToolResult {
                tool_use_id: ToolUseId::from_raw("t"),
                content: ToolResultContent::text(t),
                is_error: false,
            }],
            meta: MessageMeta::default(),
        };
        let reply = |id: &str| Message::Assistant {
            id: MessageId::from_raw(id),
            content: vec![AssistantContent::Text { text: "好".into() }],
            usage: None,
            meta: MessageMeta::default(),
        };
        // 每条按 10 token 算。
        let count = |ms: &[Message]| (ms.len() * 10) as u32;

        let msgs = vec![
            prompt("u1", "第一问"),
            reply("a1"),
            prompt("u2", "第二问"),
            reply("a2"),
            result("r2", "工具结果"),
            reply("a3"),
        ];
        assert_eq!(
            split_point(&msgs, count, 100),
            2,
            "从最后一条提问（u2）起留尾巴，工具结果不算提问"
        );
        assert_eq!(
            split_point(&msgs, count, 30),
            msgs.len(),
            "尾巴 4 条 = 40 token 超预算，整段都压"
        );
        assert_eq!(
            split_point(&msgs[..2], count, 100),
            2,
            "只有首条提问时不切 —— 那等于什么都不压"
        );
        assert_eq!(split_point(&[], count, 100), 0);
    }

    #[test]
    fn 总结请求里图片换成占位符() {
        let msgs = vec![Message::User {
            id: MessageId::from_raw("m1"),
            content: vec![
                UserContent::Attachment(Attachment::Image {
                    media_type: "image/png".into(),
                    data: "AAAA".repeat(1000),
                }),
                UserContent::Text {
                    text: "看图".into(),
                },
            ],
            meta: MessageMeta::default(),
        }];
        let stripped = strip_for_summary(&msgs);
        let Message::User { content, .. } = &stripped[0] else {
            panic!()
        };
        assert!(
            !format!("{content:?}").contains("AAAA"),
            "base64 图片进总结请求是纯浪费 —— 总结是文本任务"
        );
        assert!(matches!(&content[1], UserContent::Text { text } if text == "看图"));
    }

    /// `[约束]` 总结请求不带 `tools`，而 Anthropic 对"有 tool_use/tool_result
    /// 块但没定义 tools"的请求直接 400。块必须消失，信息必须留下。
    #[test]
    fn 总结请求里工具块降级为纯文本() {
        use riot_protocol::id::ToolUseId;

        let msgs = vec![
            Message::Assistant {
                id: MessageId::from_raw("a1"),
                content: vec![AssistantContent::ToolUse {
                    id: ToolUseId::from_raw("t1"),
                    name: "read_file".into(),
                    input: serde_json::json!({"path": "/p/a.rs"}),
                }],
                usage: None,
                meta: MessageMeta::default(),
            },
            Message::User {
                id: MessageId::from_raw("m1"),
                content: vec![UserContent::ToolResult {
                    tool_use_id: ToolUseId::from_raw("t1"),
                    content: ToolResultContent::text("fn main() {}"),
                    is_error: false,
                }],
                meta: MessageMeta::default(),
            },
        ];

        let stripped = strip_for_summary(&msgs);
        let debug = format!("{stripped:?}");
        assert!(
            !debug.contains("ToolUse") && !debug.contains("ToolResult"),
            "工具块必须全部降级成文本：{debug}"
        );
        assert!(
            debug.contains("read_file") && debug.contains("/p/a.rs"),
            "调了什么工具、什么参数要留在文字里：{debug}"
        );
        assert!(debug.contains("fn main"), "工具结果的内容要留在文字里");
    }

    /// 同形状 = 吃前缀缓存的前提。消息改一个字节、system/tools 差一点，
    /// ~100k token 的历史就从那里开始全量重算，且没有任何报错。
    #[tokio::test]
    async fn 同形状路径原样发_退回路径才改写() {
        use crate::testing::ScriptedProvider;
        use riot_protocol::id::ToolUseId;
        use riot_protocol::provider::ProviderEvent;

        let history = vec![
            Message::Assistant {
                id: MessageId::from_raw("a1"),
                content: vec![AssistantContent::ToolUse {
                    id: ToolUseId::from_raw("t1"),
                    name: "read_file".into(),
                    input: serde_json::json!({"path": "/p/a.rs"}),
                }],
                usage: None,
                meta: MessageMeta::default(),
            },
            Message::User {
                id: MessageId::from_raw("m1"),
                content: vec![UserContent::ToolResult {
                    tool_use_id: ToolUseId::from_raw("t1"),
                    content: ToolResultContent::text("fn main() {}"),
                    is_error: false,
                }],
                meta: MessageMeta::default(),
            },
        ];
        let ok = || {
            vec![ProviderEvent::Message(Message::Assistant {
                id: MessageId::from_raw("s"),
                content: vec![AssistantContent::Text {
                    text: "<summary>1. 总结</summary>".into(),
                }],
                usage: None,
                meta: MessageMeta::default(),
            })]
        };
        let provider = std::sync::Arc::new(ScriptedProvider::new(vec![ok(), ok()]));
        let arc: std::sync::Arc<dyn Provider> = std::sync::Arc::clone(&provider) as _;

        let shape = RequestShape {
            system: "主循环的完整 system".into(),
            tools: vec![ToolSpec {
                name: "read_file".into(),
                description: "读文件".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }],
        };
        summarize_history(
            &arc,
            "m",
            &history,
            Some(&shape),
            CancellationToken::new(),
        )
        .await
        .expect("同形状路径成功");
        summarize_history(&arc, "m", &history, None, CancellationToken::new())
            .await
            .expect("退回路径成功");

        let reqs = provider.requests();
        // 同形状：system/tools 透传，消息里的工具块原样保留。
        assert_eq!(reqs[0].system, "主循环的完整 system");
        assert_eq!(reqs[0].tools.len(), 1, "tools 是缓存层级第一层，必须带");
        let shaped = format!("{:?}", reqs[0].messages);
        assert!(
            shaped.contains("ToolUse") && shaped.contains("ToolResult"),
            "同形状路径不许改写消息：{shaped}"
        );
        // 退回：精简 system、无 tools、工具块转文本。
        assert_eq!(reqs[1].system, SUMMARY_SYSTEM);
        assert!(reqs[1].tools.is_empty());
        let stripped = format!("{:?}", reqs[1].messages);
        assert!(
            !stripped.contains("ToolUse") && !stripped.contains("ToolResult"),
            "退回路径必须转文本（无 tools 的请求带工具块，Anthropic 400）：{stripped}"
        );
    }

    #[test]
    fn 总结请求里思考块被丢弃() {
        // 思考是脚手架不是事实；Anthropic 的签名与模型绑定，总结模型
        // 可能不是会话模型，带上必 400（INV-9 同源）。
        let msgs = vec![Message::Assistant {
            id: MessageId::from_raw("a1"),
            content: vec![
                AssistantContent::Thinking {
                    text: "内心戏".into(),
                    signature: Some("sig".into()),
                },
                AssistantContent::Text {
                    text: "回答正文".into(),
                },
            ],
            usage: None,
            meta: MessageMeta::default(),
        }];

        let stripped = strip_for_summary(&msgs);
        let debug = format!("{stripped:?}");
        assert!(!debug.contains("内心戏"), "思考不进总结请求：{debug}");
        assert!(debug.contains("回答正文"), "正文要留下");
    }
}
