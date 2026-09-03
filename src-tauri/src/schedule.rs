//! 定时任务：存储、时间运算与错过检测。
//!
//! # 调度权威在宿主
//!
//! 任务表持久化在 `sessions/schedules.json`（挨着 index.json，同一条
//! 原子写的规矩）；tick 循环、到点执行、系统通知都在宿主进程 ——
//! 内核只是经反向 RPC 发起操作的客户端（riot_protocol::schedule）。
//!
//! # 时间语义
//!
//! - 所有"几点"都是**本地时间**。用户说"每天八点"指的是他墙上的钟，
//!   不是 UTC；换时区后任务跟着墙钟走。
//! - `next_run_ms` 是唯一的调度依据：tick 只看 `enabled && next_run <= now`。
//!   一次性任务（`Repeat::Once`）的时刻就存在 next_run 上，跑完清空。
//! - 只在 **App 运行时**调度（v1 拍板）。错过的到点在启动时收进
//!   [`MissedRun`] 清单，交给用户决定补跑还是算了 —— 静默补跑一堆
//!   积压任务会在开机瞬间并发烧一堆模型调用。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use riot_protocol::schedule::{MissedRun, Repeat, ScheduledTask, WhenSpec};

/// 存储结构。字段全部 `default` —— 加载老文件不能因缺字段整体失败
/// （和 PersistedSession 同一条向后兼容约束）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedTask {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub repeat: Repeat,
    /// Some = 到点在这个会话续跑；None = 每次新开会话。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// 新开会话时绑定的项目根。
    pub root: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_session_id: Option<String>,
    pub created_at_ms: u64,
}

impl PersistedTask {
    /// 给前端 / 模型的视图：附上现算的本地时间文字。
    pub fn to_view(&self) -> ScheduledTask {
        ScheduledTask {
            id: self.id.clone(),
            name: self.name.clone(),
            prompt: self.prompt.clone(),
            repeat: self.repeat.clone(),
            session_id: self.session_id.clone(),
            root: self.root.clone(),
            enabled: self.enabled,
            next_run_ms: self.next_run_ms,
            next_run_local: self.next_run_ms.map(local_text),
            last_run_ms: self.last_run_ms,
            last_run_local: self.last_run_ms.map(local_text),
            last_session_id: self.last_session_id.clone(),
            created_at_ms: self.created_at_ms,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ScheduleBook {
    #[serde(default)]
    pub tasks: Vec<PersistedTask>,
}

fn book_path(sessions_dir: &Path) -> PathBuf {
    sessions_dir.join("schedules.json")
}

/// 读任务表。不存在 = 空表；损坏 = 备份后空表（任务没有第二事实来源，
/// 丢了只能告警，但决不能让启动失败）。
pub fn load(sessions_dir: &Path) -> ScheduleBook {
    let p = book_path(sessions_dir);
    // 豁免理由：宿主持久化层，读的是自己的任务表。
    #[allow(clippy::disallowed_methods)]
    match std::fs::read_to_string(&p) {
        Ok(raw) => match serde_json::from_str::<ScheduleBook>(&raw) {
            Ok(book) => book,
            Err(e) => {
                tracing::error!(error = %e, "任务表读不懂，备份后从空表开始");
                backup_unreadable(&p);
                ScheduleBook::default()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ScheduleBook::default(),
        Err(e) => {
            tracing::error!(error = %e, "任务表读取失败，从空表开始（文件保留原样）");
            ScheduleBook::default()
        }
    }
}

/// 原子写任务表：临时文件 + rename，和 index.json 同一条数据安全线。
pub fn save(sessions_dir: &Path, book: &ScheduleBook) -> std::io::Result<()> {
    // 豁免理由：宿主持久化层，写的是自己的任务表。
    #[allow(clippy::disallowed_methods)]
    {
        std::fs::create_dir_all(sessions_dir)?;
        let json = serde_json::to_string_pretty(book).map_err(std::io::Error::other)?;
        let tmp = sessions_dir.join("schedules.json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, book_path(sessions_dir))
    }
}

/// 把读不懂的任务表挪去旁边，别让下一次保存把可能还能捞的内容盖掉。
fn backup_unreadable(p: &Path) {
    let bak = p.with_extension("json.bak");
    // 豁免理由：宿主持久化层。
    #[allow(clippy::disallowed_methods)]
    if let Err(e) = std::fs::rename(p, &bak) {
        tracing::error!(error = %e, "任务表备份失败，保留原文件");
    }
}

// ────────────────────────────────────────────────────────────
// 时间运算。全部是 (输入, now) → 输出 的纯函数，单测不需要真实时钟。
// ────────────────────────────────────────────────────────────

/// 启动时判定"错过"的容差。刚到点 90 秒内算正常到期（tick 周期的余量），
/// 更早的才算错过 —— 不然每次启动都把上一 tick 边缘的任务报成错过。
pub const MISS_GRACE_MS: u64 = 90_000;

/// 解析创建说法：返回（存储用的重复规则，首次运行时刻）。
///
/// 错误文案带上当前本地时间 —— 模型对"现在几点"没有可靠感知，给了
/// 过去的时刻就把钟报给它，照着改一次就对。
pub fn resolve_spec(when: &WhenSpec, now_ms: u64) -> Result<(Repeat, u64), String> {
    match when {
        WhenSpec::Once { at } => {
            let ts = parse_local(at)?;
            if ts <= now_ms {
                return Err(format!(
                    "{at} 已经过了（现在是 {}）。给一个未来的时刻，或者用 after 相对分钟数。",
                    local_text(now_ms)
                ));
            }
            Ok((Repeat::Once, ts))
        }
        WhenSpec::After { minutes } => {
            if *minutes == 0 {
                return Err("after 的 minutes 至少是 1。".to_owned());
            }
            // 一年封顶：更久的多半是单位写错了（分钟当成了秒）。
            let m = (*minutes).min(60 * 24 * 366) as u64;
            Ok((Repeat::Once, now_ms + m * 60_000))
        }
        WhenSpec::Daily { time } => {
            let repeat = Repeat::Daily { time: time.clone() };
            let first = next_run(&repeat, now_ms).ok_or_else(|| bad_time(time))?;
            Ok((repeat, first))
        }
        WhenSpec::Weekdays { time } => {
            let repeat = Repeat::Weekdays { time: time.clone() };
            let first = next_run(&repeat, now_ms).ok_or_else(|| bad_time(time))?;
            Ok((repeat, first))
        }
        WhenSpec::Weekly { weekday, time } => {
            if !(1..=7).contains(weekday) {
                return Err(format!(
                    "weekday 要在 1（周一）到 7（周日）之间，收到 {weekday}。"
                ));
            }
            let repeat = Repeat::Weekly {
                weekday: *weekday,
                time: time.clone(),
            };
            let first = next_run(&repeat, now_ms).ok_or_else(|| bad_time(time))?;
            Ok((repeat, first))
        }
    }
}

fn bad_time(time: &str) -> String {
    format!("时间「{time}」不是 HH:MM 格式（例：08:00、15:30）。")
}

/// 周期任务在 `after_ms` 之后最近的一次运行时刻。`Once` 没有下一次。
pub fn next_run(repeat: &Repeat, after_ms: u64) -> Option<u64> {
    use chrono::{Datelike, Duration, Local, TimeZone};

    let (time, want_day): (&str, fn(chrono::Weekday) -> bool) = match repeat {
        Repeat::Once => return None,
        Repeat::Daily { time } => (time, |_| true),
        Repeat::Weekdays { time } => (time, |w| {
            !matches!(w, chrono::Weekday::Sat | chrono::Weekday::Sun)
        }),
        Repeat::Weekly { weekday, time } => {
            let target = *weekday;
            // 闭包不能捕获又当 fn 用，Weekly 单独走一条循环。
            let (h, m) = parse_hhmm(time)?;
            let after = Local.timestamp_millis_opt(after_ms as i64).single()?;
            let mut day = after.date_naive();
            for _ in 0..15 {
                if day.weekday().number_from_monday() as u8 == target
                    && let Some(ts) = local_at(day, h, m)
                    && ts > after_ms
                {
                    return Some(ts);
                }
                day = day.succ_opt()?;
            }
            return None;
        }
    };

    let (h, m) = parse_hhmm(time)?;
    let after = Local.timestamp_millis_opt(after_ms as i64).single()?;
    let mut day = after.date_naive();
    // 15 天上限：Daily/Weekdays 正常两三天内必有解，到不了就是时间
    // 数据坏了，返回 None 让调用方停用任务，别在循环里空转。
    for _ in 0..15 {
        if want_day(day.weekday())
            && let Some(ts) = local_at(day, h, m)
            && ts > after_ms
        {
            return Some(ts);
        }
        day = day.checked_add_signed(Duration::days(1))?;
    }
    None
}

/// 某个本地日期的 HH:MM 对应的 Unix 毫秒。
///
/// DST 边界的取舍：重复的时刻取**早的那个**（宁可早跑不漏跑）；被跳过
/// 的时刻（春季拨快，02:30 不存在）顺延一小时再试 —— 那天的"每天 02:30"
/// 变成 03:30 跑，比整天不跑好。
fn local_at(day: chrono::NaiveDate, h: u32, m: u32) -> Option<u64> {
    use chrono::{Local, TimeZone};
    let naive = day.and_hms_opt(h, m, 0)?;
    match Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(dt) => Some(dt.timestamp_millis() as u64),
        chrono::LocalResult::Ambiguous(early, _) => Some(early.timestamp_millis() as u64),
        chrono::LocalResult::None => {
            let shifted = naive.checked_add_signed(chrono::Duration::hours(1))?;
            Local
                .from_local_datetime(&shifted)
                .earliest()
                .map(|dt| dt.timestamp_millis() as u64)
        }
    }
}

/// "HH:MM" → (时, 分)。
fn parse_hhmm(s: &str) -> Option<(u32, u32)> {
    let (h, m) = s.trim().split_once(':')?;
    let h: u32 = h.trim().parse().ok()?;
    let m: u32 = m.trim().parse().ok()?;
    (h < 24 && m < 60).then_some((h, m))
}

/// "YYYY-MM-DD HH:MM"（也认 `/` 分隔和带秒）→ Unix 毫秒。
fn parse_local(s: &str) -> Result<u64, String> {
    use chrono::{Local, NaiveDateTime, TimeZone};
    let t = s.trim().replace('/', "-");
    let naive = NaiveDateTime::parse_from_str(&t, "%Y-%m-%d %H:%M")
        .or_else(|_| NaiveDateTime::parse_from_str(&t, "%Y-%m-%d %H:%M:%S"))
        .map_err(|_| {
            format!("时间「{s}」读不懂。用 \"YYYY-MM-DD HH:MM\"（例：2026-09-01 15:30）。")
        })?;
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|dt| dt.timestamp_millis() as u64)
        .ok_or_else(|| format!("时间「{s}」在本地时区不存在（夏令时跳过的区间），换一个时刻。"))
}

/// 当前 Unix 毫秒。
///
/// 豁免理由：宿主调度层 —— 调度这件事本身就是对真实时钟做出反应，
/// 黄金回放不经过宿主。纯计算部分（resolve_spec / next_run）都以参数
/// 收时间，单测不需要它。
#[allow(clippy::disallowed_methods)]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// ms → 本地时间文字 "YYYY-MM-DD HH:MM"。
pub fn local_text(ms: u64) -> String {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_millis_opt(ms as i64)
        .single()
        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| format!("@{ms}"))
}

/// 起调度循环：周期检查到期任务并开跑。setup 时调一次。
///
/// 20 秒一查：定时任务是分钟级语义，更密只是空转，更疏会让"90 分钟后"
/// 这类相对任务的迟到能被感觉出来。第一次 tick 立即发生 —— 启动时就
/// 查一遍（真正错过的已经在 [`reconcile_on_start`] 里被拦下，这里查到
/// 的只有"启动前 90 秒内刚到点"的正常到期）。
pub fn spawn_ticker(app: tauri::AppHandle) {
    use tauri::Manager;
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(20));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let state = app.state::<crate::state::AppState>().inner().clone();
            state.schedule_tick().await;
        }
    });
}

/// 启动对账：把"App 没开着时错过的到点"收进清单，并把任务表修到
/// 面向未来的状态（周期任务的 next_run 推进到未来；一次性任务停用，
/// 跑不跑由用户在补跑提示里决定）。
///
/// 返回错过清单。任务表有改动时返回 `true`（调用方负责落盘）。
pub fn reconcile_on_start(tasks: &mut [PersistedTask], now_ms: u64) -> (Vec<MissedRun>, bool) {
    let mut missed = Vec::new();
    let mut dirty = false;
    for t in tasks.iter_mut() {
        let Some(due) = t.next_run_ms else { continue };
        if !t.enabled || due + MISS_GRACE_MS > now_ms {
            continue;
        }
        // 数一下错过了几次（关机三天的每日任务 = 3 次），封顶别空转。
        let mut count: u32 = 1;
        let mut last = due;
        while let Some(next) = next_run(&t.repeat, last) {
            if next + MISS_GRACE_MS > now_ms || count >= 60 {
                break;
            }
            last = next;
            count += 1;
        }
        missed.push(MissedRun {
            task_id: t.id.clone(),
            name: t.name.clone(),
            count,
            last_ms: last,
            last_local: local_text(last),
        });
        dirty = true;
        match next_run(&t.repeat, now_ms) {
            Some(next) => t.next_run_ms = Some(next),
            None => {
                // 一次性任务：时刻已过，停下来等用户决定。
                t.next_run_ms = None;
                t.enabled = false;
            }
        }
    }
    (missed, dirty)
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use chrono::{Local, TimeZone};

    /// 本地时间 → ms 的测试助手。
    fn ms(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> u64 {
        Local
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .earliest()
            .expect("合法本地时间")
            .timestamp_millis() as u64
    }

    #[test]
    fn 每天_今天没过就是今天_过了就是明天() {
        let repeat = Repeat::Daily {
            time: "08:00".into(),
        };
        // 2026-09-02 是周三
        let before = ms(2026, 9, 2, 6, 0);
        assert_eq!(next_run(&repeat, before), Some(ms(2026, 9, 2, 8, 0)));
        let after = ms(2026, 9, 2, 9, 0);
        assert_eq!(next_run(&repeat, after), Some(ms(2026, 9, 3, 8, 0)));
        // 正好压在 08:00 上：> 语义，算已过，去明天 —— 到点执行后重算
        // next_run 时传的就是"此刻"，不去明天会当场再跑一遍。
        assert_eq!(
            next_run(&repeat, ms(2026, 9, 2, 8, 0)),
            Some(ms(2026, 9, 3, 8, 0))
        );
    }

    #[test]
    fn 工作日跳过周末() {
        let repeat = Repeat::Weekdays {
            time: "09:00".into(),
        };
        // 2026-09-04 是周五：过了 09:00，下一次是周一（09-07）
        let friday_late = ms(2026, 9, 4, 10, 0);
        assert_eq!(next_run(&repeat, friday_late), Some(ms(2026, 9, 7, 9, 0)));
    }

    #[test]
    fn 每周指定星期() {
        let repeat = Repeat::Weekly {
            weekday: 5, // 周五
            time: "16:00".into(),
        };
        // 2026-09-02 周三 → 本周五
        assert_eq!(
            next_run(&repeat, ms(2026, 9, 2, 12, 0)),
            Some(ms(2026, 9, 4, 16, 0))
        );
        // 周五 17:00 已过 → 下周五
        assert_eq!(
            next_run(&repeat, ms(2026, 9, 4, 17, 0)),
            Some(ms(2026, 9, 11, 16, 0))
        );
    }

    #[test]
    fn 一次性没有下一次() {
        assert_eq!(next_run(&Repeat::Once, 1), None);
    }

    #[test]
    fn 解析_过去的时刻要报当前时间() {
        let now = ms(2026, 9, 2, 12, 0);
        let err = resolve_spec(
            &WhenSpec::Once {
                at: "2026-09-02 08:00".into(),
            },
            now,
        )
        .expect_err("过去的时刻该拒绝");
        assert!(
            err.contains("2026-09-02 12:00"),
            "要报当前时刻让模型自纠：{err}"
        );

        let (repeat, ts) = resolve_spec(
            &WhenSpec::Once {
                at: "2026-09-02 15:30".into(),
            },
            now,
        )
        .expect("未来时刻");
        assert_eq!(repeat, Repeat::Once);
        assert_eq!(ts, ms(2026, 9, 2, 15, 30));
    }

    #[test]
    fn 解析_相对分钟() {
        let now = ms(2026, 9, 2, 12, 0);
        let (repeat, ts) = resolve_spec(&WhenSpec::After { minutes: 90 }, now).expect("90 分钟后");
        assert_eq!(repeat, Repeat::Once);
        assert_eq!(ts, now + 90 * 60_000);
        assert!(resolve_spec(&WhenSpec::After { minutes: 0 }, now).is_err());
    }

    #[test]
    fn 解析_坏时间格式给修法() {
        let err = resolve_spec(
            &WhenSpec::Daily {
                time: "8点".into()
            },
            0,
        )
        .expect_err("坏格式该拒绝");
        assert!(err.contains("HH:MM"), "{err}");

        let err = resolve_spec(
            &WhenSpec::Weekly {
                weekday: 8,
                time: "08:00".into(),
            },
            0,
        )
        .expect_err("weekday 越界该拒绝");
        assert!(err.contains("周日"), "{err}");
    }

    #[test]
    fn 保存后能读回_缺文件为空() {
        let dir = tempfile::tempdir().expect("临时目录");
        assert!(load(dir.path()).tasks.is_empty());

        let book = ScheduleBook {
            tasks: vec![PersistedTask {
                id: "sch_1".into(),
                name: "晨报".into(),
                prompt: "给我晨报".into(),
                repeat: Repeat::Daily {
                    time: "08:00".into(),
                },
                session_id: None,
                root: "/w".into(),
                enabled: true,
                next_run_ms: Some(42),
                last_run_ms: None,
                last_session_id: None,
                created_at_ms: 1,
            }],
        };
        save(dir.path(), &book).expect("保存");
        let back = load(dir.path());
        assert_eq!(back.tasks, book.tasks);
    }

    #[test]
    fn 任务表损坏时备份并从空表开始() {
        let dir = tempfile::tempdir().expect("临时目录");
        std::fs::write(dir.path().join("schedules.json"), "{坏的").expect("写坏");
        assert!(load(dir.path()).tasks.is_empty());
        assert!(
            dir.path().join("schedules.json.bak").exists(),
            "损坏的要备份，不能直接扔"
        );
    }

    fn task(repeat: Repeat, next: Option<u64>) -> PersistedTask {
        PersistedTask {
            id: "sch_1".into(),
            name: "t".into(),
            prompt: "p".into(),
            repeat,
            session_id: None,
            root: "/w".into(),
            enabled: true,
            next_run_ms: next,
            last_run_ms: None,
            last_session_id: None,
            created_at_ms: 1,
        }
    }

    #[test]
    fn 启动对账_周期任务错过要计数并推进() {
        // 关机三天：错过 3 次，next_run 推到未来
        let now = ms(2026, 9, 5, 12, 0);
        let mut tasks = vec![task(
            Repeat::Daily {
                time: "08:00".into(),
            },
            Some(ms(2026, 9, 3, 8, 0)),
        )];
        let (missed, dirty) = reconcile_on_start(&mut tasks, now);
        assert!(dirty);
        assert_eq!(missed.len(), 1);
        assert_eq!(missed[0].count, 3, "9/3、9/4、9/5 早上各错过一次");
        assert_eq!(
            tasks[0].next_run_ms,
            Some(ms(2026, 9, 6, 8, 0)),
            "推进到明天，不能开机就补跑"
        );
        assert!(tasks[0].enabled, "周期任务照常跑下去");
    }

    #[test]
    fn 启动对账_一次性错过要停用等用户决定() {
        let now = ms(2026, 9, 5, 12, 0);
        let mut tasks = vec![task(Repeat::Once, Some(ms(2026, 9, 5, 9, 0)))];
        let (missed, dirty) = reconcile_on_start(&mut tasks, now);
        assert!(dirty);
        assert_eq!(missed.len(), 1);
        assert_eq!(missed[0].count, 1);
        assert!(!tasks[0].enabled, "跑不跑由用户在补跑提示里决定");
        assert_eq!(tasks[0].next_run_ms, None);
    }

    #[test]
    fn 启动对账_容差内和未来的不算错过() {
        let now = ms(2026, 9, 5, 12, 0);
        let mut tasks = vec![
            // 30 秒前刚到点：正常到期，tick 会接手，不算错过
            task(Repeat::Once, Some(now - 30_000)),
            // 未来的
            task(
                Repeat::Daily {
                    time: "08:00".into(),
                },
                Some(ms(2026, 9, 6, 8, 0)),
            ),
        ];
        let (missed, dirty) = reconcile_on_start(&mut tasks, now);
        assert!(missed.is_empty(), "{missed:?}");
        assert!(!dirty);
    }

    #[test]
    fn 启动对账_暂停的不算错过() {
        let now = ms(2026, 9, 5, 12, 0);
        let mut t = task(
            Repeat::Daily {
                time: "08:00".into(),
            },
            Some(ms(2026, 9, 1, 8, 0)),
        );
        t.enabled = false;
        let mut tasks = vec![t];
        let (missed, dirty) = reconcile_on_start(&mut tasks, now);
        assert!(missed.is_empty(), "用户自己暂停的，不该拿来烦他");
        assert!(!dirty);
    }
}
