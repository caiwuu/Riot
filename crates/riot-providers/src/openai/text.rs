//! Chat Completions 和 Responses 共用的文字渲染。
//!
//! 附件和工具结果的措辞必须两边一致 —— 模型换形态不该看到两种注入格式。

use riot_protocol::message::{Attachment, ToolResultContent};

use crate::anthropic::request::SystemSection;

/// 拼出最终发给模型的 system / instructions 文本。
///
/// 分段边界只对 Anthropic 的分块缓存有意义；OpenAI 两条路都是一条
/// 字符串装完，把两段原序接回去。顺序不能动，服务端的自动前缀缓存
/// 靠稳定前缀命中。不切的话那个标记会原样念给模型听。
pub(super) fn assemble_system(system: &[SystemSection], req_system: &str) -> String {
    let system_text = system
        .iter()
        .map(|s| s.text.as_str())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    let (stable, project) = crate::anthropic::request::split_request_system(req_system);
    let req_system = if project.is_empty() {
        stable.to_owned()
    } else {
        format!("{stable}\n\n{project}")
    };

    match (system_text.is_empty(), req_system.is_empty()) {
        (true, true) => String::new(),
        (true, false) => req_system,
        (false, true) => system_text,
        (false, false) => format!("{system_text}\n\n{req_system}"),
    }
}

pub(super) fn data_url(media_type: &str, data: &str) -> String {
    format!("data:{media_type};base64,{data}")
}

pub(super) fn render_result(content: &ToolResultContent, is_error: bool) -> String {
    let body = match content {
        ToolResultContent::Text { text } => text.clone(),
        ToolResultContent::Spilled {
            path,
            preview,
            total_bytes,
        } => format!(
            "Result too large ({total_bytes} bytes); written to {}. First part:\n{preview}",
            path.display()
        ),
        ToolResultContent::Cleared => "[result cleared to save context]".to_owned(),
        // 图片本身跟在这条工具结果后面的那条 user 消息里。这里留一句话
        // 是因为结果不能为空 —— 空结果会让一部分模型误判任务结束。
        ToolResultContent::Image { media_type, .. } => {
            format!("(the {media_type} image is in the next message)")
        }
        ToolResultContent::DescribedImage { text, .. } => text.clone(),
        ToolResultContent::MarkedImage {
            media_type, text, ..
        } => {
            format!(
                "{text}\n(the image accompanying this result is in the next message, {media_type})"
            )
        }
    };

    // 空的 tool 结果会让部分模型误判任务结束。见 ARCHITECTURE.md §6.7
    let body = if body.trim().is_empty() {
        "(completed with no output)".to_owned()
    } else {
        body
    };

    if is_error {
        format!("Error: {body}")
    } else {
        body
    }
}

/// 附件转成给模型的文字。和 Anthropic 那条路（`convert_attachment`）保持
/// 同一套措辞 —— 模型换协议不该看到两种不同的注入格式。
pub(super) fn render_attachment(a: &Attachment) -> Option<String> {
    Some(match a {
        Attachment::Memory { path, content } => format!(
            "<system-reminder>\nProject memory {}:\n{content}\n</system-reminder>",
            path.display()
        ),
        Attachment::RestoredFile { path, content } => format!(
            "<system-reminder>\nYou read {} before compaction:\n{content}\n</system-reminder>",
            path.display()
        ),
        Attachment::UserFile { path, content } => format!(
            "<system-reminder>\nThe user referenced {} in their message; its contents \
             follow:\n{content}\n</system-reminder>",
            path.display()
        ),
        Attachment::Environment { text } | Attachment::SystemReminder { text } => {
            format!("<system-reminder>\n{text}\n</system-reminder>")
        }
        Attachment::DescribedImage { text, .. } => {
            format!("<system-reminder>\n{text}\n</system-reminder>")
        }
        Attachment::Image { .. } => return None,
    })
}
