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
use riot_protocol::message::{Attachment, Message, UserContent};
use riot_protocol::terminal::TerminalInfo;

/// 快照渲染的首行。差分指纹和水合恢复（[`last_snapshot_text`]）都靠它
/// 从 `Attachment::Environment` 里认出"这是环境快照"—— git 快照和
/// 轮首状态行走的是同一种附件。
pub const SNAPSHOT_HEADER: &str =
    "Environment snapshot (current state of the terminal panel and the built-in browser)";

/// 快照 → 注入给模型的那段文字。空环境也有话说（"没有你能看的终端"）——
/// 从有到无也是变化，调用方靠首轮 + [`EnvSnapshot::is_quiet`] 决定跳过。
pub fn render(snap: &EnvSnapshot) -> String {
    let mut s = format!("{SNAPSHOT_HEADER}\n");

    if snap.mine.is_empty() && snap.shared.is_empty() {
        s.push_str("No terminal in the panel is visible to you.\n");
    }
    if !snap.mine.is_empty() {
        s.push_str("Processes you started:\n");
        for t in &snap.mine {
            s.push_str(&format!("  {}\n", term_line(t)));
        }
    }
    if !snap.shared.is_empty() {
        s.push_str("Terminals the user shared with you (readable, not stoppable):\n");
        for t in &snap.shared {
            s.push_str(&format!("  {}\n", term_line(t)));
        }
    }
    if snap.unshared_count > 0 {
        s.push_str(&format!(
            "The user has {} other terminal(s) that are not shared; you cannot see their \
             contents. If you need one, ask them to click 「共享给 agent」 on it in the terminal \
             panel.\n",
            snap.unshared_count
        ));
    }
    if let Some(b) = &snap.browser {
        let title = if b.title.is_empty() {
            String::new()
        } else {
            format!(" ({})", b.title)
        };
        s.push_str(&format!(
            "The browser panel is open at {}{title}, with {} tab(s).\n",
            b.url, b.tabs
        ));
    }
    // 尾行说清差分语义的另一半：快照本身声明时效，别让模型拿三轮前的
    // 快照当现状（git.rs 那条"不必再跑"的教训反过来用）。
    s.push_str(
        "The above was sampled at the start of this turn; until a new snapshot arrives, it still \
         holds.",
    );
    s
}

fn term_line(t: &TerminalInfo) -> String {
    let state = if t.running { "running" } else { "exited" };
    match &t.command {
        Some(cmd) => format!("[{}] {} — {cmd} — {state}", t.id, t.title),
        None => format!("[{}] {} — {state}", t.id, t.title),
    }
}

/// 一条告警 → system-reminder 文本。
pub fn alert_text(a: &EnvAlert) -> String {
    format!(
        "Something went wrong in the output of terminal [{}] ({}):\n{}\n\
         If it bears on the current task, read the full output with TerminalOutput(id={}); if it \
         does not, ignore it and do NOT comment on it.",
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
        "About {}% of the context is used (history is compacted automatically at 100%). \
         Compaction swallows old tool results, so write conclusions that matter into the body of \
         a reply while you still have them.",
        pct.min(100)
    )
}

/// 每轮注入的时钟行，如 `Now: 2026-08-31 (Monday) 16:37, UTC+8.`
///
/// wire 格式里消息不带时间戳（meta 不进请求），模型对「这轮离上轮隔了
/// 多久」零感知 —— 上午到下午的五个小时在它眼里是紧挨着的两行字，
/// 真实翻过车：拿上午的浏览器快照回答下午的提问。这一行是它唯一的钟。
///
/// 精确时刻不进 system prompt（变一个字整个前缀缓存作废，见 prompt.rs
/// 顶部的缓存约束）；轮首注入只出现在**新**消息里，前缀原封不动，
/// 每轮十几个 token 买断整类"时间盲"错误。
pub fn clock_line(epoch_ms: u64, tz_offset_minutes: i32) -> String {
    let shifted = epoch_ms.saturating_add_signed(i64::from(tz_offset_minutes) * 60_000);
    let (y, mo, d) = riot_tools::tools::web::date::ymd_utc(shifted);
    const WEEKDAYS: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    // 1970-01-01 是周四：天序号 +4 再模 7，正好落在 0=周日 的表上。
    let weekday = WEEKDAYS[((shifted / 86_400_000 + 4) % 7) as usize];
    let minutes_of_day = (shifted / 60_000) % (24 * 60);
    format!(
        "Now: {y}-{mo:02}-{d:02} ({weekday}) {:02}:{:02}, {}.",
        minutes_of_day / 60,
        minutes_of_day % 60,
        tz_label(tz_offset_minutes)
    )
}

/// `UTC+8` / `UTC-7` / `UTC+5:30` / `UTC`。偏移必须写出来 ——
/// 拿不到时区时（Clock 默认值 0）渲染的是 UTC 时刻，不标注的话
/// 就是把 UTC 假装成本地时间，比不注入还糟。
fn tz_label(offset_minutes: i32) -> String {
    if offset_minutes == 0 {
        return "UTC".to_owned();
    }
    let sign = if offset_minutes < 0 { '-' } else { '+' };
    let abs = offset_minutes.unsigned_abs();
    if abs.is_multiple_of(60) {
        format!("UTC{sign}{}", abs / 60)
    } else {
        format!("UTC{sign}{}:{:02}", abs / 60, abs % 60)
    }
}

/// 间隔警示的阈值。半小时以内是同一工作流里的正常停顿，报出来是噪音；
/// 超过就值得把「外部状态可能变了」说出口 —— 真实翻车的形态是隔了半天，
/// 阈值放低到"出去吃了顿饭"的量级，宁可多提醒一句。
const GAP_NOTICE_MS: u64 = 30 * 60 * 1000;

/// 距上一条消息隔了多久的警示行。不到阈值返回 None。
///
/// 时钟行给的是绝对时刻，模型理论上能自己对着历史算间隔 —— 但"能算"
/// 和"会注意到"是两回事：大间隔配一句显式提示，把注意力直接引到
/// 「先重新核实」上。
pub fn gap_line(gap_ms: u64) -> Option<String> {
    if gap_ms < GAP_NOTICE_MS {
        return None;
    }
    let mins = gap_ms / 60_000;
    let human = if mins < 60 {
        format!("{mins} minutes")
    } else if mins < 48 * 60 {
        format!("{} hours", mins / 60)
    } else {
        format!("{} days", mins / (24 * 60))
    };
    Some(format!(
        "About {human} have passed since the previous message. Terminals, the browser, and files \
         may all have changed in the meantime — snapshots and conclusions in the history describe \
         that earlier moment only, so re-verify anything about the current state before you rely \
         on it."
    ))
}

/// 采样失败时替代沉默的提醒。
///
/// 契约是「没有新快照就是环境没变」—— 探针一断，沉默会被这条契约反向
/// 背书成"一切照旧"。宣告作废之后调用方必须清掉指纹：一来连续断供
/// 只唠叨这一次，二来恢复采样的那一轮差分对 None 必真，全量重发。
pub const STALE_NOTICE: &str = "Environment sampling failed this turn: treat the terminal and browser state from every \
     earlier snapshot as unknown rather than unchanged, and re-check with a tool anything you are \
     about to rely on.";

/// 历史里模型最后看到的快照全文。水合时恢复差分指纹用。
///
/// 重启后指纹从 None 起步的话，恰逢环境变空会命中"首轮安静跳过"，
/// transcript 里的旧快照就被「没有新快照 = 没变」反向背书成现状
/// （真实翻过车：上午开的页面被当成下午还开着）。恢复成"模型最后
/// 看到的那份"，从有到空的差分就能正确触发。
///
/// 匹配靠 [`SNAPSHOT_HEADER`] 开头行；老版本把档位线拼在快照同一条
/// 附件里，恢复出来的指纹和纯渲染文本差一行，代价只是下一轮多发一份
/// 全量快照，随后自愈。
pub fn last_snapshot_text(msgs: &[Message]) -> Option<String> {
    msgs.iter().rev().find_map(|m| match m {
        Message::User { content, .. } => content.iter().rev().find_map(|c| match c {
            UserContent::Attachment(Attachment::Environment { text })
                if text.starts_with(SNAPSHOT_HEADER)
                    || text.starts_with(LEGACY_SNAPSHOT_HEADER) =>
            {
                Some(text.clone())
            }
            _ => None,
        }),
        _ => None,
    })
}

/// 英文化之前的快照首行。跨版本会话的历史里还是这一行，不认它的话指纹
/// 从 None 起步，恰逢环境变空就命中"首轮安静跳过"，旧快照被「没有新快照
/// = 没变」反向背书成现状 —— 正是上面那段注释记的那次翻车。
const LEGACY_SNAPSHOT_HEADER: &str = "环境快照（终端面板与内置浏览器的现状）";

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
        assert!(out.contains("[3] dev server — pnpm dev — running"), "{out}");
        assert!(out.contains("[5] 测试 — cargo test — exited"), "{out}");
        assert!(
            out.contains("readable, not stoppable"),
            "共享终端要说清权限边界：{out}"
        );
        assert!(
            out.contains("2 other terminal(s) that are not shared"),
            "{out}"
        );
        // 按钮名是界面上的字面量，必须原样给出 —— 翻译过去用户在面板上找不到。
        assert!(
            out.contains("共享给 agent"),
            "要指路怎么共享，不然模型只记住一个数字：{out}"
        );
        assert!(
            out.contains("http://localhost:5173 (Riot), with 3 tab(s)"),
            "{out}"
        );
        assert!(
            out.contains("until a new snapshot arrives, it still holds"),
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
        assert!(
            out.contains("No terminal in the panel is visible to you"),
            "{out}"
        );
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
        assert!(
            out.ends_with("ignore it and do NOT comment on it."),
            "护栏不能少：{out}"
        );
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
            line.contains("write conclusions that matter into the body of a reply"),
            "失忆预警是这行存在的一半理由：{line}"
        );
    }

    /// 2026-08-31T08:37Z。东八区应渲染成 16:37 周一 —— 日期、星期、
    /// 时刻、时区标注一个都不能少，这行是模型唯一的钟。
    const MONDAY_0837Z: u64 = 1_788_165_420_000;

    #[test]
    fn 时钟行_按时区渲染日期星期与时刻() {
        assert_eq!(
            clock_line(MONDAY_0837Z, 480),
            "Now: 2026-08-31 (Monday) 16:37, UTC+8."
        );
        // 拿不到时区就诚实标 UTC，不许把 UTC 假装成本地时间。
        assert_eq!(
            clock_line(MONDAY_0837Z, 0),
            "Now: 2026-08-31 (Monday) 08:37, UTC."
        );
        // 西向偏移与半小时时区（印度 UTC+5:30）。
        assert_eq!(
            clock_line(MONDAY_0837Z, -420),
            "Now: 2026-08-31 (Monday) 01:37, UTC-7."
        );
        assert_eq!(
            clock_line(MONDAY_0837Z, 330),
            "Now: 2026-08-31 (Monday) 14:07, UTC+5:30."
        );
    }

    /// 偏移跨过午夜时，日期和星期要跟着进位 —— 只挪时刻不挪日期的话，
    /// 东八区每天头八个小时的日期都是错的。
    #[test]
    fn 时钟行_偏移跨午夜要进位日期() {
        // 2026-08-31T20:00Z → 东八区 09-01 04:00，周二。
        assert_eq!(
            clock_line(1_788_206_400_000, 480),
            "Now: 2026-09-01 (Tuesday) 04:00, UTC+8."
        );
    }

    /// 半小时以内不说话；超过按人话报间隔并要求重新核实。
    #[test]
    fn 间隔警示_有阈值且单位分档() {
        const MIN: u64 = 60_000;
        assert_eq!(gap_line(29 * MIN), None, "半小时内是正常停顿");
        let half_hour = gap_line(30 * MIN).expect("到阈值该说");
        assert!(half_hour.contains("About 30 minutes"), "{half_hour}");
        assert!(
            half_hour.contains("re-verify"),
            "警示要指路行动：{half_hour}"
        );

        let hours = gap_line(5 * 60 * MIN).expect("有值");
        assert!(hours.contains("About 5 hours"), "{hours}");
        let days = gap_line(3 * 24 * 60 * MIN).expect("有值");
        assert!(days.contains("About 3 days"), "{days}");
    }

    /// 水合恢复指纹：取历史里**最后**一份快照，git 快照和轮首状态行
    /// 虽然同为 Environment 附件，但不以快照头开场，不能被认错。
    #[test]
    fn 恢复指纹_取最后一份快照_不认错同类附件() {
        use riot_protocol::id::MessageId;
        use riot_protocol::message::{MessageMeta, UserContent};

        let env_msg = |id: &str, text: &str| Message::User {
            id: MessageId::from_raw(id),
            content: vec![UserContent::Attachment(Attachment::Environment {
                text: text.into(),
            })],
            meta: MessageMeta::default(),
        };
        let old_snap = format!("{SNAPSHOT_HEADER}\n浏览器面板开着 https://linux.do……");
        let new_snap = format!("{SNAPSHOT_HEADER}\n终端面板里没有你能看的终端。");
        let msgs = vec![
            env_msg("m1", "Git 仓库\n当前分支：main"),
            env_msg("m2", &old_snap),
            env_msg("m3", "现在是 2026-08-31（周一）16:37，UTC+8。"),
            env_msg("m4", &new_snap),
        ];
        assert_eq!(
            last_snapshot_text(&msgs).as_deref(),
            Some(new_snap.as_str())
        );

        assert_eq!(
            last_snapshot_text(&msgs[..1]),
            None,
            "历史里从没有过快照就该是 None —— 首轮安静跳过靠它成立"
        );
    }
}
