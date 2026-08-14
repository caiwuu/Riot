//! LLM 结构化总结：压缩阶梯的最重一档。
//!
//! # 设计（对照 Claude Code 的 services/compact）
//!
//! - **九节式总结提示词**：主要意图 / 技术概念 / 文件与代码 / 错误与修复 /
//!   问题解决 / 全部用户原话 / 待办 / 当前工作 / 下一步。九节里最关键的是
//!   「全部用户原话」—— 意图漂移是压缩最大的风险，原话是锚。
//! - **analysis / summary 分离**：先让模型在 `<analysis>` 里自查遗漏，再产出
//!   `<summary>`。入库前剥掉 analysis —— 它是脚手架，不是产物。
//! - **禁工具前后夹击**：总结请求不带 tools，提示词开头结尾都写明只输出
//!   文本。总结模型调工具等于烧掉它唯一的一轮。
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
    Attachment, Message, MessageMeta, ToolResultContent, UserContent,
};
use riot_protocol::provider::{Provider, ProviderEvent, ProviderRequest, ThinkingConfig};
use tokio_util::sync::CancellationToken;

/// 总结输出的预算。CC 用 20k；对齐它 —— 长会话的九节总结真能写到
/// 上万 token，砍太狠丢的是"文件与代码段"那节的完整片段。
const SUMMARY_MAX_OUTPUT_TOKENS: u32 = 16_384;

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
/// 失败返回人话（进日志/熔断计数用）。取消时返回 Err —— 调用方本来
/// 就在被取消的路径上，任何返回值都不会被用。
pub async fn summarize_history(
    provider: &Arc<dyn Provider>,
    model: &str,
    messages: &[Message],
    cancel: CancellationToken,
) -> Result<String, String> {
    let mut request_messages = strip_for_summary(messages);
    request_messages.push(Message::User {
        id: riot_protocol::id::MessageId::from_raw("msg_compact_prompt"),
        content: vec![UserContent::Text { text: COMPACT_PROMPT.into() }],
        meta: MessageMeta { synthetic: true, ..Default::default() },
    });

    let request = ProviderRequest {
        model: model.to_owned(),
        messages: request_messages,
        system: SUMMARY_SYSTEM.into(),
        // 不带工具：总结模型调工具等于烧掉它唯一的一轮。
        tools: Vec::new(),
        max_output_tokens: Some(SUMMARY_MAX_OUTPUT_TOKENS),
        thinking: ThinkingConfig::Off,
    };

    let mut stream = provider.stream(request, cancel);
    let mut text = String::new();
    while let Some(ev) = stream.next().await {
        match ev {
            ProviderEvent::Message(Message::Assistant { content, .. }) => {
                for c in content {
                    if let riot_protocol::message::AssistantContent::Text { text: t } = c {
                        text.push_str(&t);
                    }
                }
            }
            ProviderEvent::Error(e) => return Err(format!("总结请求失败：{e}")),
            _ => {}
        }
    }

    let summary = extract_summary(&text);
    if summary.trim().is_empty() {
        return Err("总结模型没有产出 <summary> 内容".into());
    }
    Ok(summary)
}

/// 把总结包成续接消息（合成 user 消息），作为压缩后历史的开头。
///
/// `memory` 是重新注入的记忆附件（压缩把带着记忆的首条消息一起吞了，
/// 不重注的话项目约定就此消失）；`restored` 是工作集文件。
pub fn continuation_message(
    summary: &str,
    memory: Vec<Attachment>,
    restored: Vec<Attachment>,
    id: riot_protocol::id::MessageId,
) -> Message {
    let mut content: Vec<UserContent> =
        memory.into_iter().map(UserContent::Attachment).collect();
    content.push(UserContent::Text {
        text: format!(
            "本会话由一段更早的对话延续而来，先前内容已压缩。以下是前文的完整总结：\n\n{summary}\n\n\
             直接接着做，不要复述总结、不要向用户再次确认、不要说「我将继续」—— \
             像中断从未发生过一样，接上手头的任务。",
        ),
    });
    content.extend(restored.into_iter().map(UserContent::Attachment));
    Message::User {
        id,
        content,
        meta: MessageMeta { synthetic: true, ..Default::default() },
    }
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
/// 已清理的工具结果保持原样。
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
                        UserContent::ToolResult { tool_use_id, content, is_error } => {
                            UserContent::ToolResult {
                                tool_use_id: tool_use_id.clone(),
                                content: match content {
                                    ToolResultContent::Image { .. }
                                    | ToolResultContent::DescribedImage { .. } => {
                                        ToolResultContent::text("[图片结果，总结时已省略]")
                                    }
                                    other => other.clone(),
                                },
                                is_error: *is_error,
                            }
                        }
                        other => other.clone(),
                    })
                    .collect(),
                meta: meta.clone(),
            },
            other => other.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use riot_protocol::id::MessageId;

    #[test]
    fn 剥掉_analysis_取出_summary() {
        let raw = "<analysis>我想想……这里有三段对话</analysis>\n<summary>1. 主要请求：修 bug</summary>";
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
            vec![Attachment::Memory { path: "/p/AGENTS.md".into(), content: "约定".into() }],
            vec![Attachment::RestoredFile { path: "/p/a.rs".into(), content: "code".into() }],
            MessageId::from_raw("m1"),
        );
        let Message::User { content, meta, .. } = &m else { panic!("是 user") };
        assert!(meta.synthetic, "合成消息要打标，UI 靠它区分于用户亲口说的话");
        assert!(
            matches!(&content[0], UserContent::Attachment(Attachment::Memory { .. })),
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
                UserContent::Text { text: "看图".into() },
            ],
            meta: MessageMeta::default(),
        }];
        let stripped = strip_for_summary(&msgs);
        let Message::User { content, .. } = &stripped[0] else { panic!() };
        assert!(
            !format!("{content:?}").contains("AAAA"),
            "base64 图片进总结请求是纯浪费 —— 总结是文本任务"
        );
        assert!(matches!(&content[1], UserContent::Text { text } if text == "看图"));
    }
}
