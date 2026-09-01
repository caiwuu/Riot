//! 带上限的按行读取。
//!
//! MCP 服务器是第三方进程，它往管道里写什么完全不受我们控制。裸的
//! `read_until(b'\n', …)` 会一直读到换行为止 —— 一条永不换行的输出流
//! （投毒的包、死循环里打日志的 bug）就把宿主的内存吃光了，而表象是
//! "整个应用越来越慢然后被系统杀掉"，没人会想到是某个 MCP 服务器。
//!
//! `[约束]` 超限的行整条丢弃，不做截断保留。半条 JSON-RPC 帧解析出来
//! 只会是垃圾，而带着"这行被截过"的字节去 route 会让日志里出现一条
//! 看起来像真实帧的假帧。

use tokio::io::{AsyncBufRead, AsyncBufReadExt as _};

pub(crate) enum ReadLine {
    /// `buf` 里是一行（含行尾的 `\n`，最后一行可能没有）。
    Line,
    /// 这一行超过上限，已经读到行尾并丢弃。连接照常。
    TooLong,
    /// 流结束或读失败。两者对调用方是同一件事：这条连接没有下文了。
    Eof,
}

/// 读一行，最多攒 `max` 字节。
///
/// 超限之后**继续读到换行**再返回，这样下一次调用是从一个干净的行首
/// 开始的 —— 只丢一行，不是把后面的帧全部错位解析。
pub(crate) async fn read_line_capped<R>(reader: &mut R, buf: &mut Vec<u8>, max: usize) -> ReadLine
where
    R: AsyncBufRead + Unpin,
{
    buf.clear();
    let mut dropped = false;

    loop {
        let (consumed, at_newline) = {
            let chunk = match reader.fill_buf().await {
                Ok(c) => c,
                Err(_) => return ReadLine::Eof,
            };
            if chunk.is_empty() {
                // EOF。手上还有半行就先交出去，下一次调用返回 Eof。
                return if dropped {
                    ReadLine::TooLong
                } else if buf.is_empty() {
                    ReadLine::Eof
                } else {
                    ReadLine::Line
                };
            }

            let (take, at_newline) = match chunk.iter().position(|b| *b == b'\n') {
                Some(i) => (&chunk[..=i], true),
                None => (chunk, false),
            };
            if !dropped {
                if buf.len() + take.len() > max {
                    // 越界的瞬间就把攒的放掉，别让"已经超了"这件事
                    // 本身继续占着内存。
                    dropped = true;
                    buf.clear();
                    buf.shrink_to_fit();
                } else {
                    buf.extend_from_slice(take);
                }
            }
            (take.len(), at_newline)
        };
        reader.consume(consumed);

        if at_newline {
            return if dropped {
                ReadLine::TooLong
            } else {
                ReadLine::Line
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::BufReader;

    use super::*;

    async fn collect(input: &[u8], max: usize) -> Vec<Result<String, ()>> {
        let mut reader = BufReader::new(input);
        let mut buf = Vec::new();
        let mut out = Vec::new();
        loop {
            match read_line_capped(&mut reader, &mut buf, max).await {
                ReadLine::Eof => return out,
                ReadLine::TooLong => out.push(Err(())),
                ReadLine::Line => out.push(Ok(String::from_utf8_lossy(&buf).into_owned())),
            }
        }
    }

    #[tokio::test]
    async fn 正常的行原样读出来() {
        let got = collect(b"a\nbb\nccc", 1024).await;
        assert_eq!(
            got,
            vec![Ok("a\n".into()), Ok("bb\n".into()), Ok("ccc".into())],
            "末尾没有换行的那行也要交出来"
        );
    }

    #[tokio::test]
    async fn 永不换行的输出不会吃光内存() {
        // 恶意或故障的 MCP 服务器只要一直写不带换行的字节就够了。
        // 裸 read_until 会一路攒到 OOM。
        let mut input = vec![b'x'; 100_000];
        input.extend_from_slice(b"\n{\"ok\":1}\n");

        let got = collect(&input, 1024).await;
        assert_eq!(
            got,
            vec![Err(()), Ok("{\"ok\":1}\n".into())],
            "超限的只丢那一行，后面的帧照常解析 —— 否则一行坏数据就废掉整条连接"
        );
    }

    #[tokio::test]
    async fn 超限行没有换行结尾时也不卡住() {
        let got = collect(&vec![b'x'; 5000], 1024).await;
        assert_eq!(got, vec![Err(())]);
    }

    #[tokio::test]
    async fn 恰好等于上限的行仍然收下() {
        // 上限是"能收多少"，不是"从多少开始拒"。差一个字节的判断错误
        // 会让贴着上限的合法帧随机失败，那种 bug 极难复现。
        let line = format!("{}\n", "x".repeat(9));
        let got = collect(line.as_bytes(), 10).await;
        assert_eq!(got, vec![Ok(line)]);
    }
}
