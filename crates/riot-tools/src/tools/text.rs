//! 文本文件的解码与编码。
//!
//! 这个模块存在的唯一理由是**读回来的东西写回去要一模一样**。
//!
//! Read 和 Edit 组合起来是一条完整的读-改-写链路,链路上任何一处丢失
//! 信息,表现都是"文件被静默改坏":
//!
//! - lossy 解码 + 全量写回 → 非 UTF-8 字节变成 `U+FFFD`,原始内容永久丢失;
//! - 归一化 CRLF 但写回 LF → 整个文件每一行都进 diff,真正的改动被淹没;
//! - 丢掉 BOM → 某些 Windows 工具链不再认这个文件。
//!
//! 这三种都不报错。所以这里的原则是:**信息保不住就拒绝,不要猜。**

/// 判定为二进制之前扫描的字节数。
///
/// 只扫开头:整文件扫描对大文件太贵,而二进制文件的 NUL 几乎总在前面
/// (文件头、magic number)。真正的文本文件前 8KB 没有 NUL 基本就没有。
const BINARY_SNIFF_BYTES: usize = 8192;

const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextFile {
    /// 解码后的内容。换行风格是 [`Newline::Lf`] 或 [`Newline::Crlf`] 时
    /// 已归一化成 `\n`;[`Newline::Mixed`] 时保持原样。
    pub content: String,
    pub newline: Newline,
    pub bom: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Newline {
    Lf,
    Crlf,
    /// 文件里两种换行都有。
    ///
    /// 这时**不做归一化** —— 归一化之后无法还原,写回去会把没碰过的行
    /// 也一起改掉。代价是 Edit 的 `old_string` 要带准确的换行符,
    /// 但混合换行本来就罕见,而"改一行结果整个文件进 diff"更糟。
    Mixed,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("这是二进制文件（{reason}），不能作为文本读取")]
    Binary { reason: &'static str },
}

pub fn decode(bytes: &[u8]) -> Result<TextFile, DecodeError> {
    let (bom, body) = match bytes.strip_prefix(BOM) {
        Some(rest) => (true, rest),
        None => (false, bytes),
    };

    // NUL 字节。文本文件里不该有,而它是二进制最可靠的信号。
    if body[..body.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        return Err(DecodeError::Binary {
            reason: "包含 NUL 字节",
        });
    }

    // `[约束]` 这里用 from_utf8 而不是 from_utf8_lossy。
    //
    // lossy 会把无效字节替换成 U+FFFD,读出来看着正常,一旦被 Edit
    // 全量写回,原始字节就永久丢了 —— 而且整个过程不报任何错。
    let text = std::str::from_utf8(body).map_err(|_| DecodeError::Binary {
        reason: "不是有效的 UTF-8",
    })?;

    let newline = detect_newline(text);
    let content = match newline {
        Newline::Crlf => text.replace("\r\n", "\n"),
        Newline::Lf | Newline::Mixed => text.to_owned(),
    };

    Ok(TextFile {
        content,
        newline,
        bom,
    })
}

/// 把内容按原文件的风格写回字节。
///
/// `[约束]` 必须是 [`decode`] 的逆操作:`encode(decode(x)) == x`。
/// 有 roundtrip 测试守着这一条。
pub fn encode(content: &str, newline: Newline, bom: bool) -> Vec<u8> {
    let body = match newline {
        Newline::Crlf => content.replace('\n', "\r\n"),
        Newline::Lf | Newline::Mixed => content.to_owned(),
    };

    let mut out = Vec::with_capacity(body.len() + if bom { 3 } else { 0 });
    if bom {
        out.extend_from_slice(BOM);
    }
    out.extend_from_slice(body.as_bytes());
    out
}

fn detect_newline(text: &str) -> Newline {
    let crlf = text.matches("\r\n").count();
    if crlf == 0 {
        return Newline::Lf;
    }

    // 独立的 \n（不属于任何 \r\n）
    let lf_total = text.matches('\n').count();
    if lf_total > crlf {
        Newline::Mixed
    } else {
        Newline::Crlf
    }
}

/// 给模型看的带行号格式。
///
/// `[约束]` 行号和内容之间用 tab 分隔,行号右对齐 6 位。
///
/// 格式必须稳定:模型会照着行号提 Edit 请求,也会在回复里引用行号。
/// 换格式等于让所有历史对话里的行号引用失效。
pub fn with_line_numbers(content: &str, start_line: usize) -> String {
    let mut out = String::with_capacity(content.len() + content.lines().count() * 8);
    for (i, line) in content.lines().enumerate() {
        out.push_str(&format!("{:>6}\t{}\n", start_line + i, line));
    }
    out
}

/// 按行切片。`offset` 从 0 开始,`limit` 是行数。
pub fn slice_lines(content: &str, offset: usize, limit: usize) -> String {
    content
        .lines()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn line_count(content: &str) -> usize {
    content.lines().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn 纯_lf_文件() {
        let f = decode(b"a\nb\nc").expect("是文本");
        assert_eq!(f.newline, Newline::Lf);
        assert_eq!(f.content, "a\nb\nc");
        assert!(!f.bom);
    }

    #[test]
    fn crlf_归一化后写回仍是_crlf() {
        // 不还原的话，改一行会让整个文件每一行都进 diff，
        // 真正的改动被淹没在几百行换行符变更里
        let f = decode(b"a\r\nb\r\nc").expect("是文本");
        assert_eq!(f.newline, Newline::Crlf);
        assert_eq!(f.content, "a\nb\nc", "对上层统一成 \\n");

        let back = encode(&f.content, f.newline, f.bom);
        assert_eq!(back, b"a\r\nb\r\nc");
    }

    #[test]
    fn 混合换行不归一化() {
        // 归一化之后无法还原，写回去会把没碰过的行也改掉
        let f = decode(b"a\r\nb\nc\nd").expect("是文本");
        assert_eq!(f.newline, Newline::Mixed);
        assert_eq!(f.content, "a\r\nb\nc\nd", "保持原样");
        assert_eq!(encode(&f.content, f.newline, f.bom), b"a\r\nb\nc\nd");
    }

    #[test]
    fn bom_被保留() {
        // 丢掉 BOM 之后某些 Windows 工具链不再认这个文件
        let raw = b"\xEF\xBB\xBFhello";
        let f = decode(raw).expect("是文本");
        assert!(f.bom);
        assert_eq!(f.content, "hello", "BOM 不进内容");
        assert_eq!(encode(&f.content, f.newline, f.bom), raw);
    }

    #[test]
    fn roundtrip_对各种组合都成立() {
        // encode 必须是 decode 的逆操作 —— 这是整个模块存在的理由
        for raw in [
            &b"plain"[..],
            b"a\nb\n",
            b"a\r\nb\r\n",
            b"\xEF\xBB\xBFa\r\nb\r\n",
            b"a\r\nb\nc",
            b"",
            b"\n",
            b"no trailing newline",
            "中文\n内容\n".as_bytes(),
            "emoji 😀\r\n".as_bytes(),
        ] {
            let f = decode(raw).expect("是文本");
            assert_eq!(
                encode(&f.content, f.newline, f.bom),
                raw,
                "roundtrip 失败：{raw:?}"
            );
        }
    }

    #[test]
    fn 非_utf8_被拒绝而不是_lossy() {
        // lossy 解码 + 全量写回 = 原始字节永久丢失，且全程不报错
        let latin1 = b"caf\xE9 au lait";
        assert!(matches!(
            decode(latin1),
            Err(DecodeError::Binary {
                reason: "不是有效的 UTF-8"
            })
        ));
    }

    #[test]
    fn nul_字节判定为二进制() {
        assert!(matches!(
            decode(b"\x7FELF\x02\x01\x01\0"),
            Err(DecodeError::Binary { .. })
        ));
    }

    #[test]
    fn 只嗅探开头的字节() {
        // 整文件扫描对大文件太贵。前 8KB 干净就当文本处理，
        // 后面真有 NUL 的话 UTF-8 校验会兜住（NUL 本身是合法 UTF-8，
        // 所以这里确实会放过 —— 代价是超大二进制文件可能被当文本读，
        // 但它们几乎总在开头就有 NUL）
        let mut big = vec![b'a'; BINARY_SNIFF_BYTES + 10];
        big[BINARY_SNIFF_BYTES + 5] = 0;
        assert!(decode(&big).is_ok());
    }

    #[test]
    fn 空文件是合法文本() {
        let f = decode(b"").expect("空文件是文本");
        assert_eq!(f.content, "");
        assert_eq!(f.newline, Newline::Lf);
    }

    #[test]
    fn 行号格式稳定() {
        // 模型会照着行号提 Edit 请求，换格式等于让历史引用全部失效
        assert_eq!(with_line_numbers("a\nb", 1), "     1\ta\n     2\tb\n");
        assert_eq!(with_line_numbers("x", 100), "   100\tx\n");
    }

    #[test]
    fn 行号从指定偏移开始() {
        assert_eq!(with_line_numbers("c\nd", 3), "     3\tc\n     4\td\n");
    }

    #[test]
    fn 按行切片() {
        assert_eq!(slice_lines("a\nb\nc\nd", 1, 2), "b\nc");
        assert_eq!(slice_lines("a\nb", 5, 2), "", "越界返回空");
    }
}
