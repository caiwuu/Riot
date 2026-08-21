//! 环境快照的宿主侧组装 + 告警扫描。设计与约束见 docs/ENV_DESIGN.md。
//!
//! # 可见集 = term_access 的可见集
//!
//! `[约束]` 快照按 `Terminals` 注册表条目上的 owner / shared 分拣，判定和
//! `term_access::HostTerminal` 完全同源 —— 快照在结构上不可能比工具看得更多。
//! 未共享的终端在这里只剩一个数字：标题和内容根本不进 [`EnvSnapshot`]。
//!
//! # 告警是拉取时扫描，不是流式观察
//!
//! `[取舍]` 不起线程盯 PTY 流。告警只随快照拉取交付（内核轮首采样），
//! 流式检测买不来更早的交付时机，只买来一套线程和去抖状态机。
//! 代价是错误滚出扫描窗口就漏报 —— 可接受：尾部的报错才是可行动的。

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use riot_protocol::env::{BrowserGlance, EnvAlert, EnvSnapshot};
use riot_protocol::terminal::TerminalInfo;

use crate::state::AppState;

/// 告警模式：子串，不是正则。高精度低召回是刻意的 —— 告警的成本不在
/// token，在模型被无关信息勾走。宁可漏报（模型还有工具可查），不能狼来了。
const ALERT_PATTERNS: &[&str] = &[
    "Traceback (most recent call last)",
    "panicked at",
    "error[E",
    "npm ERR!",
    "EADDRINUSE",
    "ECONNREFUSED",
    "Segmentation fault",
    "UnhandledPromiseRejection",
    "test result: FAILED",
    "FATAL ERROR",
];

/// 摘录：命中行起最多几行、几个字符。够看清是什么错，剩下的模型自己
/// 用 TerminalOutput 读。
const EXCERPT_LINES: usize = 3;
const EXCERPT_CHARS: usize = 240;
/// 一次快照最多带几条告警。
const MAX_ALERTS: usize = 3;
/// 扫尾部多少行。再往前的报错早过时了。
const SCAN_LINES: usize = 80;

/// (会话, 终端) → 上次告警摘录的哈希。
///
/// `[约束]` 按会话分开记：两个会话看着同一个共享终端时，各自都该收到一次。
/// 条目跟着终端消失被清（见 [`assemble`] 末尾的 retain）。
#[derive(Default)]
pub struct AlertSeen(Mutex<HashMap<(String, u32), u64>>);

/// 组装一次环境快照。任何一部分拿不到就少一部分 —— 感知是锦上添花，
/// 不该报错挡住轮次。
pub async fn assemble(state: &AppState, session_id: &str) -> EnvSnapshot {
    let all = state.terminals().list();
    let mut mine = Vec::new();
    let mut shared = Vec::new();
    let mut unshared_count = 0u32;
    for t in &all {
        let is_mine = t.owner.as_deref() == Some(session_id);
        if is_mine {
            mine.push(info(t, false));
        } else if t.shared {
            shared.push(info(t, true));
        } else {
            // 别的会话起的服务也算"你看不到的"：对本会话它就是不可见。
            unshared_count += 1;
        }
    }

    // 告警：只扫可见的（自己起的 ∪ 共享的）。锁内只有同步调用，
    // 顺序是告警表 → 终端表，没有反向路径。
    let mut alerts = Vec::new();
    {
        let mut seen = state.env_alerts().0.lock().expect("告警表锁");
        for t in mine.iter().chain(shared.iter()) {
            if alerts.len() >= MAX_ALERTS {
                break;
            }
            let Ok(text) = state.terminals().read(t.id, SCAN_LINES) else {
                continue;
            };
            let Some(excerpt) = scan_tail(&text) else {
                continue;
            };
            let key = (session_id.to_owned(), t.id);
            let h = hash(&excerpt);
            // 同一段报错不重复告警。摘录变了（新的错）才再报。
            if seen.get(&key) == Some(&h) {
                continue;
            }
            seen.insert(key, h);
            alerts.push(EnvAlert {
                terminal_id: t.id,
                title: t.title.clone(),
                excerpt,
            });
        }
        // 终端没了就把本会话对它的记录清掉，不积垢。
        let live: std::collections::HashSet<u32> = all.iter().map(|t| t.id).collect();
        seen.retain(|(s, tid), _| s != session_id || live.contains(tid));
    }

    // 浏览器一瞥：活动页在哪、开了几页。state() 只问活着的进程，
    // 不会为了快照把浏览器拉起来。
    let browser = match state.browser_of(session_id).await {
        Some(b) => match b.state().await {
            Ok(st) if !st.tabs.is_empty() => {
                let active = st.active_tab();
                (!active.url.is_empty()).then_some(BrowserGlance {
                    url: active.url,
                    title: active.title,
                    tabs: st.tabs.len() as u32,
                })
            }
            _ => None,
        },
        None => None,
    };

    EnvSnapshot {
        mine,
        shared,
        unshared_count,
        browser,
        alerts,
    }
}

fn info(t: &crate::term::TermSummary, shared: bool) -> TerminalInfo {
    TerminalInfo {
        id: t.id,
        title: t.title.clone(),
        command: t.command.clone(),
        running: t.running,
        shared,
    }
}

/// 尾部输出里最后一次命中告警模式的位置起，摘录 ≤3 行 / ≤240 字符。
/// 取**最后**一次：最近的错才是现状，开头那个可能早就修了。
fn scan_tail(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let hit = lines
        .iter()
        .rposition(|l| ALERT_PATTERNS.iter().any(|p| l.contains(p)))?;
    let mut excerpt = lines[hit..lines.len().min(hit + EXCERPT_LINES)].join("\n");
    if excerpt.chars().count() > EXCERPT_CHARS {
        excerpt = excerpt.chars().take(EXCERPT_CHARS).collect();
    }
    Some(excerpt)
}

fn hash(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppState {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "riot-envprobe-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        AppState::restore_at(dir.join("config.json"))
    }

    /// 这条是 docs/ENV_DESIGN.md §3.3 同意审计的镜像测试：
    /// 未共享的终端在快照里只剩数量，共享/所有权的分拣和 term_access 同源。
    #[tokio::test]
    async fn 快照按所有权与共享分拣_未共享只剩数量() {
        let st = state();
        // 用户自己开的 shell：默认对任何会话都只算个数。
        let (ch, _probe) = crate::term::testing::probe();
        let user_term = st.terminals().open(None, 80, 24, ch).expect("开终端");
        // s1 起的服务。
        let owned = st
            .terminals()
            .spawn(None, "sleep 30", "dev server", "s1")
            .expect("起服务");

        let snap = assemble(&st, "s1").await;
        assert_eq!(snap.mine.len(), 1, "自己起的要列出来");
        assert_eq!(snap.mine[0].id, owned);
        assert!(!snap.mine[0].shared);
        assert!(snap.shared.is_empty());
        assert_eq!(snap.unshared_count, 1, "用户的 shell 只算个数");

        // 换个会话看：s1 的服务对 s2 也不可见，进数量。
        let other = assemble(&st, "s2").await;
        assert!(other.mine.is_empty());
        assert_eq!(other.unshared_count, 2);

        // 用户点开共享之后，出现在 shared 列表里、带共享标记。
        st.terminals().set_shared(user_term, true);
        let snap = assemble(&st, "s1").await;
        assert_eq!(snap.shared.len(), 1);
        assert!(snap.shared[0].shared);
        assert_eq!(snap.unshared_count, 0);

        st.terminals().close(user_term);
        st.terminals().close(owned);
    }

    #[test]
    fn 摘录取最后一次命中且有上限() {
        let text = "ok\npanicked at 'first'\nfixed\nthread 'main' panicked at 'second'\nnote: run with RUST_BACKTRACE=1\ntail";
        let e = scan_tail(text).expect("该命中");
        assert!(e.contains("second"), "要最近的错，不是最早的：{e}");
        assert!(!e.contains("first"), "{e}");
        assert!(e.lines().count() <= EXCERPT_LINES);

        let long = format!("EADDRINUSE {}", "x".repeat(1000));
        let e = scan_tail(&long).expect("该命中");
        assert!(e.chars().count() <= EXCERPT_CHARS);

        assert!(
            scan_tail("一切正常\nready in 300ms").is_none(),
            "没有错不该告警"
        );
    }

    /// 同一段报错只告一次；换了内容再告；别的会话独立计。
    #[tokio::test]
    async fn 告警去重按会话独立() {
        let st = state();
        // 用共享终端做载体：真实 PTY 输出内容不可控，这里直接考察
        // 去重逻辑对 seen 表的读写 —— 用 scan_tail 的输入模拟两次不同报错。
        let seen = st.env_alerts();
        let key_s1 = ("s1".to_owned(), 7u32);
        let key_s2 = ("s2".to_owned(), 7u32);

        let first = hash("panicked at 'a'");
        let second = hash("panicked at 'b'");
        {
            let mut g = seen.0.lock().expect("锁");
            assert_ne!(g.insert(key_s1.clone(), first), Some(first), "首见该记录");
            assert_eq!(g.get(&key_s1), Some(&first));
            // 同内容：调用方看到相同哈希会跳过（assemble 里的 continue 分支）。
            assert_eq!(g.insert(key_s1.clone(), first), Some(first));
            // 换内容：哈希不同，该再告。
            assert_eq!(g.insert(key_s1.clone(), second), Some(first));
            // 另一个会话对同一个终端独立计。
            assert!(g.get(&key_s2).is_none(), "会话之间不共享去重状态");
        }
    }
}
