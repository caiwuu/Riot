//! SSE 帧解析。
//!
//! 职责只有一件事：把**任意切分**的字节流还原成一个个 SSE 事件。
//! 它不认识 Anthropic 的任何字段 —— 那是 `anthropic::decode` 的事。
//!
//! # 为什么自己写
//!
//! 不是不信任现成的库，是这一层的失败模式很特殊：网络和中间代理会制造出
//! 规范里没写的脏状态（半行、CRLF 混用、注释行、多余空行），而这些在本地
//! 开发时一次都碰不到。自己写至少能把每种脏状态都摆进测试里。
//!
//! # O(n²) 陷阱
//!
//! 每来一个 chunk 就从缓冲区开头找分隔符，是 O(n²)。一次大的工具参数
//! （几十 KB，切成几百个 chunk）就能让这一层吃掉可观的 CPU。
//! `scanned` 字段记住上次扫到哪，新数据只扫一遍。
//!
//! # 两道大小闸
//!
//! 缓冲区只在遇到空行时才 drain，所以「一直发、不发空行」就是一条通往
//! OOM 的直路，而且看起来完全像一次正常的长回答。上限超了要**报错终止**，
//! 不能静默截断：截断会把半截 JSON 交给解码器，模型收到的是一个语法坏掉
//! 的工具参数 —— 那比一条明确的错误难查得多。
//!
//! 见 ARCHITECTURE.md §11.3

/// 单个事件（两个空行之间）的上限。
///
/// 正常帧是几百字节；把它放到 8 MiB 是为了容下那些"整条响应塞进一帧"
/// 的网关（不流式的中转常这么干），同时仍然是个硬闸。
const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// 一条流累计能推进来多少字节。
///
/// `[取舍]` 帧上限拦不住"一直发合法小帧"——那种流每帧都能被消费，
/// 涨的是解码器里累积的文本，同样通往 OOM，而 idle 看门狗看的是
/// 事件间隔，数据不断就永远不触发。
///
/// 128 MiB 对真实回答有约十倍余量：Anthropic 每个 token 一帧、每帧
/// 一百多字节，128k token 的满额输出也就十几 MB。
const MAX_STREAM_BYTES: usize = 128 * 1024 * 1024;

/// 流被判定为异常，必须终止。
///
/// 是错误而不是"截断后继续"：见模块文档。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SseError {
    #[error("单个 SSE 事件超过 {limit} 字节还没结束，中止响应")]
    FrameTooLarge { limit: usize },
    #[error("响应流累计超过 {limit} 字节还没结束，中止响应")]
    StreamTooLarge { limit: usize },
}

/// 一个 SSE 事件。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SseEvent {
    /// `event:` 行。Anthropic 每个事件都带，但规范里它是可选的。
    pub event: Option<String>,
    /// `data:` 行拼起来的内容。多行 data 用 `\n` 连接。
    pub data: String,
}

impl SseEvent {
    fn is_empty(&self) -> bool {
        self.event.is_none() && self.data.is_empty()
    }
}

#[derive(Debug, Default)]
pub struct SseParser {
    buf: String,
    /// 已经扫描过分隔符的字节数。避免每次 push 都从头扫一遍。
    scanned: usize,
    /// 上一个 chunk 末尾那个没收完的 UTF-8 字符。
    pending: Vec<u8>,
    /// 这条流累计吃进了多少字节。
    total: usize,
    /// 撞过上限。撞了之后一直报同一个错，不再攒新数据 ——
    /// 调用方漏判返回值时，至少内存不会继续涨。
    failed: Option<SseError>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// 吃进一段**原始字节**，吐出这段数据凑齐的所有完整事件。
    ///
    /// `[约束]` 参数必须是 `&[u8]` 而不是 `&str`。TCP 分片不认字符边界，
    /// 一个中文字符被切成两个 chunk 是常态而非异常 —— 中文回复里几乎每次
    /// 请求都会发生。
    ///
    /// 早先这里收的是 `&str`，把重组责任推给了 transport 层。那是错的：
    /// 每个 HTTP 客户端实现都得重做一遍，漏掉的那个会产生 `看��了` 这种
    /// 乱码，而且它不报错、不崩溃，只是内容悄悄坏掉。
    ///
    /// 返回 `Err` 表示流已经不可信（见两道大小闸），调用方必须终止它。
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>, SseError> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }

        self.total = self.total.saturating_add(chunk.len());
        if self.total > MAX_STREAM_BYTES {
            return Err(self.fail(SseError::StreamTooLarge {
                limit: MAX_STREAM_BYTES,
            }));
        }

        let text = self.decode_utf8(chunk);
        let events = self.push_str(&text);

        // drain 之后 buf 里剩的就是当前这个还没收完的帧。
        if self.buf.len() > MAX_FRAME_BYTES {
            return Err(self.fail(SseError::FrameTooLarge {
                limit: MAX_FRAME_BYTES,
            }));
        }
        Ok(events)
    }

    /// 记下失败并把攒着的数据放掉。
    fn fail(&mut self, e: SseError) -> SseError {
        self.buf.clear();
        self.buf.shrink_to_fit();
        self.scanned = 0;
        self.pending.clear();
        self.failed = Some(e.clone());
        e
    }

    /// 把字节流还原成文本，把切断的字符攒到下一次。
    fn decode_utf8(&mut self, bytes: &[u8]) -> String {
        let input: &[u8] = if self.pending.is_empty() {
            bytes
        } else {
            self.pending.extend_from_slice(bytes);
            &self.pending
        };

        let mut out = String::new();
        let mut consumed = 0;

        loop {
            match std::str::from_utf8(&input[consumed..]) {
                Ok(s) => {
                    out.push_str(s);
                    consumed = input.len();
                    break;
                }
                Err(e) => {
                    let valid_end = consumed + e.valid_up_to();
                    // valid_up_to 之前一定是合法的
                    out.push_str(std::str::from_utf8(&input[consumed..valid_end]).unwrap_or(""));

                    match e.error_len() {
                        // 真正的非法序列。用替换字符顶掉并继续 ——
                        // 一个坏字节不该让整条响应作废。
                        Some(bad) => {
                            out.push('\u{FFFD}');
                            consumed = valid_end + bad;
                        }
                        // 只是被切断了，等下一个 chunk
                        None => {
                            consumed = valid_end;
                            break;
                        }
                    }
                }
            }
        }

        let leftover = input[consumed..].to_vec();
        self.pending = leftover;
        out
    }

    fn push_str(&mut self, chunk: &str) -> Vec<SseEvent> {
        self.buf.push_str(chunk);
        let mut out = Vec::new();

        // 从上次扫到的位置往后找事件分隔符（空行）。
        // 回退 3 个字节，覆盖分隔符本身被 chunk 切断的情况（如 "\r\n" | "\r\n"）。
        let mut search_from = self.scanned.saturating_sub(3);
        let mut consumed = 0usize;

        while let Some((sep_at, sep_len)) = find_separator(&self.buf, search_from) {
            let raw = &self.buf[consumed..sep_at];
            let ev = parse_frame(raw);
            if !ev.is_empty() {
                out.push(ev);
            }
            consumed = sep_at + sep_len;
            search_from = consumed;
        }

        if consumed > 0 {
            self.buf.drain(..consumed);
            self.scanned = self.buf.len();
        } else {
            self.scanned = self.buf.len();
        }

        out
    }

    /// 流结束时调用，处理最后一个没有以空行结尾的事件。
    ///
    /// 规范上不完整的帧应该丢弃，但真实网关经常在最后一帧后直接断开。
    /// 丢掉它意味着丢掉 `message_stop` —— 那会让上层以为流被截断了。
    pub fn finish(&mut self) -> Option<SseEvent> {
        // 流结束时还挂着半个字符，说明连接在字符中间断了。
        // 用替换字符补上，别静默吞掉 —— 内容坏了要看得见。
        if !self.pending.is_empty() {
            self.buf.push('\u{FFFD}');
            self.pending.clear();
        }

        if self.buf.trim().is_empty() {
            self.buf.clear();
            self.scanned = 0;
            return None;
        }
        let ev = parse_frame(&self.buf);
        self.buf.clear();
        self.scanned = 0;
        (!ev.is_empty()).then_some(ev)
    }
}

/// 找事件分隔符：连续两个换行。返回 (位置, 分隔符长度)。
///
/// 要同时认 `\n\n`、`\r\n\r\n` 和混用的 `\n\r\n`。混用不是假想 ——
/// 代理重写响应时经常只规范化一半。
fn find_separator(s: &str, from: usize) -> Option<(usize, usize)> {
    let bytes = s.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        if bytes[i] != b'\n' {
            i += 1;
            continue;
        }
        // 位置 i 是一个 \n，看它后面跟的是不是另一个换行
        let after = i + 1;
        if after < bytes.len() {
            match bytes[after] {
                b'\n' => return Some((i, 2)),
                b'\r' if after + 1 < bytes.len() && bytes[after + 1] == b'\n' => {
                    return Some((i, 3));
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

fn parse_frame(raw: &str) -> SseEvent {
    let mut ev = SseEvent::default();
    for line in raw.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);

        // 以冒号开头是注释。网关常用 `: keep-alive` 保活，
        // 当成数据处理会往 JSON 解析器里塞垃圾。
        if line.is_empty() || line.starts_with(':') {
            continue;
        }

        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            // 规范：没有冒号的行，整行是字段名，值为空
            None => (line, ""),
        };

        match field {
            "event" => ev.event = Some(value.to_owned()),
            "data" => {
                if !ev.data.is_empty() {
                    ev.data.push('\n');
                }
                ev.data.push_str(value);
            }
            // id / retry 我们用不上。忽略而不是报错 —— 未知字段是规范允许的。
            _ => {}
        }
    }
    ev
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn ev(event: &str, data: &str) -> SseEvent {
        SseEvent {
            event: Some(event.into()),
            data: data.into(),
        }
    }

    /// 正常输入喂进去，顺手断言没撞上大小闸。
    fn feed(p: &mut SseParser, bytes: &[u8]) -> Vec<SseEvent> {
        p.push(bytes).expect("正常输入不该触发大小闸")
    }

    #[test]
    fn 基本分帧() {
        let mut p = SseParser::new();
        let out = feed(
            &mut p,
            b"event: ping\ndata: {}\n\nevent: done\ndata: []\n\n",
        );
        assert_eq!(out, vec![ev("ping", "{}"), ev("done", "[]")]);
    }

    #[test]
    fn 逐字节喂也能还原() {
        // 真实网络下 chunk 边界完全不可控，这是最基本的健壮性要求。
        let input = "event: a\ndata: 1\n\nevent: b\ndata: 2\n\n";
        let mut p = SseParser::new();
        let mut out = Vec::new();
        for ch in input.chars() {
            out.extend(feed(&mut p, ch.to_string().as_bytes()));
        }
        assert_eq!(out, vec![ev("a", "1"), ev("b", "2")]);
    }

    #[test]
    fn 分隔符被切断也认得() {
        let mut p = SseParser::new();
        assert!(feed(&mut p, b"event: a\ndata: 1\r").is_empty());
        assert!(feed(&mut p, b"\n\r").is_empty());
        let out = feed(&mut p, b"\n");
        assert_eq!(out, vec![ev("a", "1")], "CRLF 分隔符跨了三个 chunk");
    }

    #[test]
    fn 混用换行符() {
        // 代理重写响应时经常只规范化一半
        let mut p = SseParser::new();
        let out = feed(&mut p, b"event: a\r\ndata: 1\n\r\n");
        assert_eq!(out, vec![ev("a", "1")]);
    }

    #[test]
    fn 注释行被忽略() {
        let mut p = SseParser::new();
        let out = feed(&mut p, b": keep-alive\n\nevent: a\ndata: 1\n\n");
        assert_eq!(
            out,
            vec![ev("a", "1")],
            "保活注释当成数据会往 JSON 解析器里塞垃圾"
        );
    }

    #[test]
    fn 多行_data_用换行连接() {
        let mut p = SseParser::new();
        let out = feed(&mut p, b"data: line1\ndata: line2\n\n");
        assert_eq!(out[0].data, "line1\nline2");
    }

    #[test]
    fn 没有空格的冒号也认() {
        let mut p = SseParser::new();
        let out = feed(&mut p, b"event:a\ndata:1\n\n");
        assert_eq!(out, vec![ev("a", "1")]);
    }

    #[test]
    fn 末尾没有空行时_finish_捞回来() {
        // 真实网关经常在最后一帧后直接断开。丢掉它 = 丢掉 message_stop，
        // 上层会以为流被截断。
        let mut p = SseParser::new();
        assert!(feed(&mut p, b"event: message_stop\ndata: {}").is_empty());
        assert_eq!(p.finish(), Some(ev("message_stop", "{}")));
    }

    #[test]
    fn finish_对干净的流返回_none() {
        let mut p = SseParser::new();
        feed(&mut p, b"event: a\ndata: 1\n\n");
        assert_eq!(p.finish(), None);
    }

    #[test]
    fn 多字节字符被切断也能还原() {
        // TCP 分片不认字符边界。中文回复里这几乎每次请求都会发生。
        // 早先这里收 &str，把重组推给 transport，结果是 `看��了` 这种
        // 乱码 —— 不报错、不崩溃，内容悄悄坏掉。
        let input = "data: 你好世界\n\n".as_bytes();
        let mut p = SseParser::new();
        let mut out = Vec::new();
        for b in input {
            out.extend(feed(&mut p, &[*b]));
        }
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].data, "你好世界");
    }

    #[test]
    fn 字符跨_chunk_边界() {
        let full = "data: 完\n\n".as_bytes();
        // "完" 是 3 字节，在它中间切开
        let cut = 6 + 1;
        let mut p = SseParser::new();
        assert!(feed(&mut p, &full[..cut]).is_empty());
        let out = feed(&mut p, &full[cut..]);
        assert_eq!(out[0].data, "完");
    }

    #[test]
    fn 非法字节不作废整条响应() {
        let mut p = SseParser::new();
        let mut bytes = b"data: ab".to_vec();
        bytes.push(0xFF); // 非法
        bytes.extend_from_slice("cd\n\n".as_bytes());

        let out = feed(&mut p, &bytes);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].data, "ab\u{FFFD}cd", "一个坏字节不该让整条响应作废");
    }

    #[test]
    fn 结尾挂着半个字符时不静默吞掉() {
        let mut p = SseParser::new();
        feed(&mut p, "data: 完".as_bytes());
        // 只喂 "完" 的前两个字节
        let mut p2 = SseParser::new();
        let partial = "data: 完".as_bytes();
        feed(&mut p2, &partial[..partial.len() - 1]);
        let ev = p2.finish().expect("要吐出来");
        assert!(
            ev.data.contains('\u{FFFD}'),
            "内容坏了要看得见：{:?}",
            ev.data
        );
    }

    #[test]
    fn 扫描不重复_大流不退化() {
        // 直接断言算法性质：处理完 N 个 chunk 后，缓冲区不该还留着已消费的数据。
        // 这比测耗时稳定 —— 耗时断言在 CI 上会随机失败。
        let mut p = SseParser::new();
        for i in 0..1000 {
            feed(&mut p, format!("event: e\ndata: {i}\n\n").as_bytes());
            assert!(
                p.buf.is_empty(),
                "完整帧消费后缓冲区应该清空，否则后续扫描是 O(n²)"
            );
        }
    }

    #[test]
    fn 超长单帧只扫描一次尾部() {
        // 一个 50KB 的 tool_use 参数被切成 500 个 chunk 的情形。
        let mut p = SseParser::new();
        feed(&mut p, b"event: x\ndata: ");
        for _ in 0..500 {
            let out = feed(&mut p, "a".repeat(100).as_bytes());
            assert!(out.is_empty());
            // scanned 必须跟着 buf 走，否则下一次 push 会从头重扫
            assert_eq!(p.scanned, p.buf.len());
        }
        let out = feed(&mut p, b"\n\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].data.len(), 50_000);
    }

    #[test]
    fn 不发空行的流被帧上限拦住() {
        // 服务端只要一直发不含空行的数据，缓冲区就永远不 drain ——
        // 内存无限涨到 OOM，而这期间看起来完全像一次正常的长回答。
        let mut p = SseParser::new();
        feed(&mut p, b"data: ");

        let mb = "x".repeat(1024 * 1024);
        let err = loop {
            match p.push(mb.as_bytes()) {
                Ok(events) => assert!(events.is_empty(), "没有空行就不该有事件"),
                Err(e) => break e,
            }
            assert!(
                p.buf.len() <= MAX_FRAME_BYTES,
                "缓冲区已经越过上限还没报错：{} 字节",
                p.buf.len()
            );
        };
        assert_eq!(
            err,
            SseError::FrameTooLarge {
                limit: MAX_FRAME_BYTES
            }
        );
        assert!(p.buf.is_empty(), "报错之后要把攒的数据放掉");
        assert_eq!(
            p.push(b"data: 1\n\n"),
            Err(SseError::FrameTooLarge {
                limit: MAX_FRAME_BYTES
            }),
            "撞过闸之后不能又开始攒数据 —— 调用方漏判返回值时这是最后一道防线"
        );
    }

    #[test]
    fn 无限长的合法流被总量上限拦住() {
        // 帧上限拦不住"一直发合法小帧"：每帧都被消费掉，涨的是解码器
        // 里累积的文本，而 idle 看门狗看的是事件间隔，数据不断就不触发。
        let mut p = SseParser::new();
        let frame = format!("data: {}\n\n", "y".repeat(1024 * 1024 - 10));
        let err = loop {
            if let Err(e) = p.push(frame.as_bytes()) {
                break e;
            }
            assert!(
                p.total <= MAX_STREAM_BYTES,
                "总量已经越过上限还没报错：{} 字节",
                p.total
            );
        };
        assert_eq!(
            err,
            SseError::StreamTooLarge {
                limit: MAX_STREAM_BYTES
            }
        );
    }

    #[test]
    fn 一帧装下整条响应的网关不被误伤() {
        // 不流式的中转会把整条响应塞进一个 data 帧。这类帧可以到几 MB，
        // 上限压太低会把它们全判成攻击。
        let mut p = SseParser::new();
        let body = "z".repeat(4 * 1024 * 1024);
        let out = feed(&mut p, format!("data: {body}\n\n").as_bytes());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].data.len(), body.len());
    }
}
