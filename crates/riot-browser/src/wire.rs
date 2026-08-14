//! 把协议事件写到 stdout。
//!
//! 类型本身在 [`riot_protocol::browser`] —— 那份定义两个进程共享。这里只放
//! 本进程专属的 IO:协议层是叶子 crate，不该知道 stdout 的存在。

use riot_protocol::browser::Event;

/// 写一行 NDJSON 到 stdout。
///
/// `[约束]` 这是**唯一**允许往 stdout 写东西的地方。混进任何一行别的内容
/// 都会把主应用那边的解析冲坏，而报错指向的是消息本身，不是真正的源头。
/// 所以本 crate 一律用 `eprintln!` 打日志。
///
/// 序列化失败只往 stderr 抱怨，不 panic:浏览器进程死掉的代价是用户正在看的
/// 页面消失，而这里的失败通常只影响一条消息。
pub fn emit(event: &Event) {
    match serde_json::to_string(event) {
        Ok(line) => {
            use std::io::Write as _;
            let mut out = std::io::stdout().lock();
            if writeln!(out, "{line}").is_err() {
                // 管道断了 = 主应用没了。继续渲染没有意义。
                std::process::exit(0);
            }
            let _ = out.flush();
        }
        Err(e) => eprintln!("[wire] 序列化事件失败: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use riot_protocol::browser::Event;

    #[test]
    fn 事件序列化成单行() {
        // NDJSON 的前提是一条消息一行。多行会把流切错位，而且错位之后
        // 后面每一条都解析失败，看起来像协议整个坏了。
        let line = serde_json::to_string(&Event::Frame {
            tab: 1,
            seq: 7,
            width: 1280,
            height: 800,
        })
        .expect("序列化");
        assert!(!line.contains('\n'), "事件不能跨行: {line}");
        assert!(line.contains("\"event\":\"frame\""));
    }
}
