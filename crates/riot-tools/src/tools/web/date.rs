//! 纪元毫秒 → 年月日。
//!
//! 只为一件事存在：把当前年月写进 WebSearch 的工具描述里。
//!
//! # 为什么这件事重要
//!
//! 模型的知识截止日期会泄漏到它生成的搜索词里 —— 让它查"最新的 React
//! 文档"，它会搜 "React documentation 2024"，然后拿回一堆过时结果并且
//! 深信不疑。把当前年月写进描述能直接消掉这个偏差。
//!
//! # 为什么不用 chrono
//!
//! 整个 workspace 只有这一处需要日历换算，而 [`civil_from_days`] 是一段
//! 有名有姓、可以逐行验证的算法（Howard Hinnant 的 civil_from_days）。
//! 为它引一个传递依赖不划算。

/// UTC 年月日。
pub fn ymd_utc(epoch_ms: u64) -> (i64, u32, u32) {
    // 向下取整到天。epoch_ms 是无符号的，1970 年之前的时间表示不了，
    // 而那个范围对"当前是几月"没有意义。
    let days = (epoch_ms / 86_400_000) as i64;
    civil_from_days(days)
}

/// 写进提示词的年月，如 `2026年8月`。
pub fn year_month(epoch_ms: u64) -> String {
    let (y, m, _) = ymd_utc(epoch_ms);
    format!("{y}年{m}月")
}

/// 天序号（1970-01-01 为 0）→ 公历年月日。
///
/// Howard Hinnant, "chrono-Compatible Low-Level Date Algorithms"。
/// 内部把三月当作一年的开始，这样闰日落在年末，闰年判断就不用特判了。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    // 移到以 0000-03-01 为原点的纪元。
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // 纪元内第几天，[0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // 三月起算的年内第几天，[0, 365]
    let mp = (5 * doy + 2) / 153; // 三月起算的月序，[0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]

    // 一、二月属于下一个公历年。
    (y + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    const DAY: u64 = 86_400_000;

    #[test]
    fn 纪元起点() {
        assert_eq!(ymd_utc(0), (1970, 1, 1));
    }

    #[test]
    fn 闰年的二月二十九() {
        // 2024-02-29 00:00:00 UTC
        assert_eq!(ymd_utc(1_709_164_800_000), (2024, 2, 29));
    }

    #[test]
    fn 世纪闰年规则() {
        // 2000 是闰年（能被 400 整除），1900 不是。
        // 算错的话每 100 年偏一天，而这种 bug 在写完的当年测不出来。
        assert_eq!(ymd_utc(951_782_400_000), (2000, 2, 29));
    }

    #[test]
    fn 跨年边界() {
        // 2025-12-31 23:59:59 与 2026-01-01 00:00:00
        let new_year = 1_767_225_600_000u64;
        assert_eq!(ymd_utc(new_year - 1000), (2025, 12, 31));
        assert_eq!(ymd_utc(new_year), (2026, 1, 1));
    }

    #[test]
    fn 连续天数单调递增() {
        // 一整年逐天推进，日期必须严格递增且月份合法
        let mut prev = ymd_utc(0);
        for i in 1..=400u64 {
            let cur = ymd_utc(i * DAY);
            assert!((1..=12).contains(&cur.1), "第 {i} 天月份非法：{cur:?}");
            assert!((1..=31).contains(&cur.2), "第 {i} 天日期非法：{cur:?}");
            assert!(cur > prev, "第 {i} 天没有递增：{prev:?} → {cur:?}");
            prev = cur;
        }
    }

    #[test]
    fn 提示词里的年月格式() {
        assert_eq!(year_month(1_767_225_600_000), "2026年1月");
    }
}
