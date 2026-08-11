//! 响应体 → Markdown。
//!
//! 两件事：把字节按正确的字符集解码成字符串，再把 HTML 转成 Markdown。
//!
//! # 为什么要管字符集
//!
//! Claude Code 直接 `buffer.toString('utf-8')`。对英文站没问题，但国内不少
//! 站点仍然是 GBK/GB18030，按 UTF-8 解出来整页都是替换字符 —— 而且不报错，
//! 模型会认认真真地去总结一堆乱码。

use std::sync::OnceLock;

use htmd::HtmlToMarkdown;

/// 送进模型（或辅助模型）的正文上限，按字符计。
///
/// 超过这个长度的网页，多出来的部分基本是评论区和推荐位。截断比让请求
/// 因为超长被拒要好。
pub const MAX_CONTENT_CHARS: usize = 100_000;

/// 转换器是无状态的，但构造时要建一批规则对象，复用一个就够。
fn converter() -> &'static HtmlToMarkdown {
    static C: OnceLock<HtmlToMarkdown> = OnceLock::new();
    C.get_or_init(|| {
        HtmlToMarkdown::builder()
            // 这些标签的内容对"读懂这个页面"没有贡献，但能占掉大半篇幅。
            // 导航和页脚尤其糟糕：每个页面都带一份，抓十个页面就重复十遍。
            .skip_tags(vec![
                "script", "style", "noscript", "nav", "footer", "header", "aside", "form", "svg",
                "iframe", "canvas", "template",
            ])
            .build()
    })
}

/// 按 content-type 和 HTML 内的 meta 声明解码响应体。
pub fn decode_body(bytes: &[u8], content_type: &str) -> String {
    let enc = charset_from_content_type(content_type)
        .or_else(|| charset_from_meta(bytes))
        .and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);

    let (text, _, _) = enc.decode(bytes);
    text.into_owned()
}

/// 从 `text/html; charset=gbk` 里取出 `gbk`。
fn charset_from_content_type(ct: &str) -> Option<String> {
    let lower = ct.to_ascii_lowercase();
    let idx = lower.find("charset=")?;
    let rest = &lower[idx + "charset=".len()..];
    let val = rest
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('"')
        .trim_matches('\'');
    (!val.is_empty()).then(|| val.to_owned())
}

/// 响应头没说的时候，从 `<meta charset=...>` 里猜。
///
/// 只扫前 2KB：charset 声明必须出现在 `<head>` 靠前的位置才对浏览器有效，
/// 往后扫既慢又容易把正文里出现的 "charset=" 当真。
fn charset_from_meta(bytes: &[u8]) -> Option<String> {
    let head = &bytes[..bytes.len().min(2048)];
    // meta 声明本身一定是 ASCII，用 lossy 解不会丢信息。
    let text = String::from_utf8_lossy(head).to_ascii_lowercase();

    if let Some(i) = text.find("charset=") {
        let rest = &text[i + "charset=".len()..];
        let val: String = rest
            .trim_start_matches(['"', '\''])
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if !val.is_empty() {
            return Some(val);
        }
    }
    None
}

/// HTML → Markdown。转换失败时退回原文。
///
/// 失败退回而不是报错：拿到一堆标签总比拿不到内容强，模型有能力从带标签的
/// 文本里读出信息。
pub fn html_to_markdown(html: &str) -> String {
    match converter().convert(html) {
        Ok(md) => collapse_blank_lines(&md),
        Err(e) => {
            tracing::warn!(error = %e, "HTML 转 Markdown 失败，退回原文");
            html.to_owned()
        }
    }
}

/// 三个以上连续换行压成两个。
///
/// 跳过一堆标签之后会留下大片空行，那些空行照样占 token。
fn collapse_blank_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut newlines = 0usize;
    for ch in s.chars() {
        if ch == '\n' {
            newlines += 1;
            if newlines <= 2 {
                out.push(ch);
            }
        } else {
            newlines = 0;
            out.push(ch);
        }
    }
    out.trim().to_owned()
}

/// 按字符数截断，并明确告诉模型截断了。
///
/// `[约束]` 必须留下截断标记。没有标记的话，模型会把半截文档当成完整文档，
/// 然后信心十足地告诉用户"文档里没有提到这个配置项"。
pub fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_owned();
    }
    let head: String = s.chars().take(max_chars).collect();
    format!("{head}\n\n[内容过长已截断，仅显示前 {max_chars} 个字符]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn 基本的_html_转换() {
        let md = html_to_markdown("<h1>标题</h1><p>一段<strong>正文</strong>。</p>");
        assert!(md.contains("# 标题"), "实际输出：{md}");
        assert!(md.contains("**正文**"), "实际输出：{md}");
    }

    #[test]
    fn 跳过脚本与样式() {
        // 不跳的话，一个现代网页里 script 的体积经常比正文大一个数量级
        let md = html_to_markdown(
            "<p>正文</p><script>var evil='应当消失'</script><style>.a{color:red}</style>",
        );
        assert!(md.contains("正文"));
        assert!(!md.contains("应当消失"), "script 内容漏出来了：{md}");
        assert!(!md.contains("color:red"), "style 内容漏出来了：{md}");
    }

    #[test]
    fn 跳过导航与页脚() {
        let md = html_to_markdown("<nav>导航项</nav><p>正文</p><footer>版权信息</footer>");
        assert!(md.contains("正文"));
        assert!(!md.contains("导航项"), "{md}");
        assert!(!md.contains("版权信息"), "{md}");
    }

    #[test]
    fn 保留链接() {
        // 抓回来的页面要能让模型给出可点的来源
        let md = html_to_markdown(r#"<a href="https://docs.rs">文档</a>"#);
        assert!(md.contains("https://docs.rs"), "链接丢了：{md}");
    }

    #[test]
    fn 从响应头取字符集() {
        // GBK 的"中文"两个字
        let gbk = [0xD6u8, 0xD0, 0xCE, 0xC4];
        let s = decode_body(&gbk, "text/html; charset=gbk");
        assert_eq!(s, "中文");
    }

    #[test]
    fn 响应头没说时从_meta_取() {
        let mut html = br#"<html><head><meta charset="gbk"></head><body>"#.to_vec();
        html.extend_from_slice(&[0xD6, 0xD0, 0xCE, 0xC4]);
        html.extend_from_slice(b"</body></html>");

        let s = decode_body(&html, "text/html");
        assert!(s.contains("中文"), "实际解出：{s}");
    }

    #[test]
    fn 默认按_utf8_解() {
        assert_eq!(decode_body("中文".as_bytes(), ""), "中文");
        assert_eq!(decode_body("中文".as_bytes(), "text/html"), "中文");
    }

    #[test]
    fn 不认识的字符集退回_utf8() {
        // 站点乱写 charset 是常态，不能因此整个抓取失败
        assert_eq!(decode_body("中文".as_bytes(), "text/html; charset=x-nope"), "中文");
    }

    #[test]
    fn 压掉多余空行() {
        assert_eq!(collapse_blank_lines("a\n\n\n\n\nb"), "a\n\nb");
        assert_eq!(collapse_blank_lines("\n\n a \n\n"), "a");
    }

    #[test]
    fn 截断带标记() {
        // 没有标记的话模型会把半截文档当完整文档用
        let out = truncate("abcdefghij", 4);
        assert!(out.starts_with("abcd"));
        assert!(out.contains("截断"), "必须告诉模型内容被截断了：{out}");
    }

    #[test]
    fn 不超长就不动() {
        assert_eq!(truncate("abc", 10), "abc");
        assert_eq!(truncate("abc", 3), "abc");
    }

    #[test]
    fn 截断按字符不按字节() {
        // 按字节切会切在 UTF-8 字符中间，轻则乱码重则 panic
        let out = truncate("中文中文中文", 2);
        assert!(out.starts_with("中文"), "实际：{out}");
        assert!(!out.starts_with("中文中"), "实际：{out}");
    }
}
