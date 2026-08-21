//! 环境快照的渲染、指纹与自我状态档位。纯函数，设计见 docs/ENV_DESIGN.md。
//!
//! # 措辞是行为的一部分
//!
//! 和 [`crate::git::describe`] 同一条经验：注入文本在诱导模型做什么/别做
//! 什么。这里的硬要求有两条 ——
//!
//! - 未共享终端那行必须指路（请用户在面板上共享），否则模型只会记下一个
//!   数字，下次需要时又去猜；
//! - 告警必须以"无关就忽略，不必评论"收尾。感知的成本不在 token，在模型
//!   被无关信息勾走 —— 这句是防分心的行为护栏，不是客气话。
//!
//! # 渲染结果同时就是指纹
//!
//! `[约束]` 差分判定比较的是渲染后的文本（[`render`] 的返回值），不是
//! 结构体哈希。拿结构体做指纹的话，渲染措辞改了而结构没变，升级前后的
//! 会话会各说各话 —— 文本相同 = 模型看到的相同，这才是"没变化"的定义。

use riot_protocol::env::{EnvAlert, EnvSnapshot};
use riot_protocol::terminal::TerminalInfo;

/// 快照 → 注入给模型的那段文字。空环境也有话说（"没有你能看的终端"）——
/// 从有到无也是变化，调用方靠首轮 + [`EnvSnapshot::is_quiet`] 决定跳过。
pub fn render(snap: &EnvSnapshot) -> String {
    let mut s = String::from("环境快照（终端面板与内置浏览器的现状）\n");

    if snap.mine.is_empty() && snap.shared.is_empty() {
        s.push_str("终端面板里没有你能看的终端。\n");
    }
    if !snap.mine.is_empty() {
        s.push_str("你起的服务：\n");
        for t in &snap.mine {
            s.push_str(&format!("  {}\n", term_line(t)));
        }
    }
    if !snap.shared.is_empty() {
        s.push_str("用户共享给你的终端（能读不能停）：\n");
        for t in &snap.shared {
            s.push_str(&format!("  {}\n", term_line(t)));
        }
    }
    if snap.unshared_count > 0 {
        s.push_str(&format!(
            "用户另有 {} 个未共享的终端；内容你看不到，需要就请他在终端面板上点「共享给 agent」。\n",
            snap.unshared_count
        ));
    }
    if let Some(b) = &snap.browser {
        let title = if b.title.is_empty() {
            String::new()
        } else {
            format!("（{}）", b.title)
        };
        s.push_str(&format!(
            "浏览器面板开着 {}{title}，共 {} 个标签页。\n",
            b.url, b.tabs
        ));
    }
    // 尾行说清差分语义的另一半：快照本身声明时效，别让模型拿三轮前的
    // 快照当现状（git.rs 那条"不必再跑"的教训反过来用）。
    s.push_str("以上是本轮开始时的采样；之后没有新快照就表示这些没变。");
    s
}

fn term_line(t: &TerminalInfo) -> String {
    let state = if t.running { "在跑" } else { "已退出" };
    match &t.command {
        Some(cmd) => format!("[{}] {} — {cmd} — {state}", t.id, t.title),
        None => format!("[{}] {} — {state}", t.id, t.title),
    }
}

/// 一条告警 → system-reminder 文本。
pub fn alert_text(a: &EnvAlert) -> String {
    format!(
        "终端 [{}]（{}）的输出里出现了异常：\n{}\n\
         与当前任务相关就用 TerminalOutput(id={}) 看完整输出；无关就忽略，不必评论。",
        a.terminal_id, a.title, a.excerpt, a.terminal_id
    )
}

/// 自我状态档位。只在向上越档时说一次 —— 每轮报数是噪音，
/// 档位的语义是"疲劳感"，不是仪表盘。
pub const BANDS: [u32; 3] = [50, 70, 85];

/// 用量百分比落在哪一档。没到 50% 是 0（不说话）。
pub fn usage_band(pct: u32) -> u32 {
    BANDS
        .iter()
        .rev()
        .find(|b| pct >= **b)
        .copied()
        .unwrap_or(0)
}

/// 越档时注入的那一行。
pub fn band_line(pct: u32) -> String {
    format!(
        "上下文已用约 {}%（满 100% 会自动压缩历史）。压缩会吞掉旧的工具结果 —— \
         重要结论尽早写进回复正文。",
        pct.min(100)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use riot_protocol::env::BrowserGlance;

    fn term(
        id: u32,
        title: &str,
        command: Option<&str>,
        running: bool,
        shared: bool,
    ) -> TerminalInfo {
        TerminalInfo {
            id,
            title: title.into(),
            command: command.map(str::to_owned),
            running,
            shared,
        }
    }

    #[test]
    fn 渲染分组列出可见终端_未共享只有数量和指路() {
        let snap = EnvSnapshot {
            mine: vec![
                term(3, "dev server", Some("pnpm dev"), true, false),
                term(5, "测试", Some("cargo test"), false, false),
            ],
            shared: vec![term(7, "终端", None, true, true)],
            unshared_count: 2,
            browser: Some(BrowserGlance {
                url: "http://localhost:5173".into(),
                title: "Riot".into(),
                tabs: 3,
            }),
            alerts: vec![],
        };
        let out = render(&snap);
        assert!(out.contains("[3] dev server — pnpm dev — 在跑"), "{out}");
        assert!(out.contains("[5] 测试 — cargo test — 已退出"), "{out}");
        assert!(out.contains("能读不能停"), "共享终端要说清权限边界：{out}");
        assert!(out.contains("另有 2 个未共享"), "{out}");
        assert!(
            out.contains("共享给 agent"),
            "要指路怎么共享，不然模型只记住一个数字：{out}"
        );
        assert!(
            out.contains("http://localhost:5173（Riot），共 3 个标签页"),
            "{out}"
        );
        assert!(
            out.contains("没有新快照就表示这些没变"),
            "差分语义要在快照里自declare：{out}"
        );
    }

    /// 从有到无也是变化：全关掉之后的快照要能说出"没有了"，
    /// 不能渲染成空串（那会被当成"没变化"）。
    #[test]
    fn 空环境渲染成没有而不是空串() {
        let snap = EnvSnapshot {
            mine: vec![],
            shared: vec![],
            unshared_count: 0,
            browser: None,
            alerts: vec![],
        };
        let out = render(&snap);
        assert!(out.contains("没有你能看的终端"), "{out}");
    }

    #[test]
    fn 告警以防分心护栏收尾() {
        let a = EnvAlert {
            terminal_id: 3,
            title: "dev server".into(),
            excerpt: "Error: EADDRINUSE :::5173".into(),
        };
        let out = alert_text(&a);
        assert!(out.contains("EADDRINUSE"), "{out}");
        assert!(out.contains("TerminalOutput(id=3)"), "要指路读全文：{out}");
        assert!(out.ends_with("无关就忽略，不必评论。"), "护栏不能少：{out}");
    }

    #[test]
    fn 档位单调且只认三档() {
        assert_eq!(usage_band(0), 0);
        assert_eq!(usage_band(49), 0);
        assert_eq!(usage_band(50), 50);
        assert_eq!(usage_band(69), 50);
        assert_eq!(usage_band(70), 70);
        assert_eq!(usage_band(84), 70);
        assert_eq!(usage_band(85), 85);
        assert_eq!(usage_band(200), 85);
    }

    #[test]
    fn 档位行显示封顶且带失忆预警() {
        let line = band_line(140);
        assert!(line.contains("100%"), "显示要封顶：{line}");
        assert!(
            line.contains("写进回复"),
            "失忆预警是这行存在的一半理由：{line}"
        );
    }
}
