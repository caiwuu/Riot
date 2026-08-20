//! 路径形状检查。
//!
//! # 这里曾经是"工作目录围栏"
//!
//! 原本这个模块还负责判断路径是否落在会话绑定的目录内，越界一律拒绝。
//! 那条限制去掉了：项目目录现在只用来给会话分类、并作为相对路径的基准，
//! **不再是访问边界**。理由是它和真实用法冲突得太厉害 —— 参考隔壁仓库、
//! 改 monorepo 的兄弟包、读一份共享配置，全是正当且常见的跨目录操作。
//!
//! `[前提]` 边界撤掉之后，挡在危险操作前面的是另外三层，别把它们也当成
//! 可以顺手简化的东西：
//!
//! 1. 默认模式下写操作逐次询问，弹窗里显示**解析后的绝对路径**；
//! 2. [`crate::safety`] 对敏感目标（SSH、凭证、shell 启动脚本、
//!    `.git` 内部、本应用自己的配置）**对「全部放行」免疫**；
//! 3. Bash 命令的静态分析。
//!
//! 代价要说清楚：「全部放行」和「无人值守」下，可写范围从"项目目录"
//! 变成了"整块磁盘减去上面那张敏感清单"。
//!
//! # 留下的部分：Windows 路径别名
//!
//! 一律检查，不分平台。理由：路径字符串来自模型，它可能生成任何风格；
//! 而且只在 Windows 上检查的话，这些用例在 Linux CI 上就跑不到 ——
//! 等于没测。
//!
//! 见 ARCHITECTURE.md §9.5

use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FenceViolation {
    #[error("路径 {path} 含有可疑构造：{reason}")]
    Suspicious { path: PathBuf, reason: &'static str },
}

/// 纯字符串层面的检查，不碰文件系统。
///
/// 这一步要在 `canonicalize` **之前**做：那些别名构造的目的就是让
/// 解析结果和字面看起来不一样，先归一化再检查等于自己把证据抹了。
pub fn check_shape(path: &Path) -> Result<(), FenceViolation> {
    let s = path.to_string_lossy();

    let suspicious = [
        // NTFS 备用数据流：`file.txt:evil` 写的是另一份内容，
        // 而目录列表里只看得到 file.txt
        (has_ads(&s), "NTFS 备用数据流"),
        // `\\?\` 绕过 Win32 路径规范化，能造出 `..` 不被处理的路径
        (s.contains("\\\\?\\") || s.contains("\\\\.\\"), "Win32 设备路径前缀"),
        // 8.3 短名：PROGRA~1 指向 "Program Files"，规则匹配不上
        (has_short_name(&s), "8.3 短文件名"),
        // Windows 会静默去掉尾部的点和空格，`foo.txt.` 实际写的是 `foo.txt`
        (has_trailing_dot_or_space(path), "尾部的点或空格"),
        // CON / NUL / COM1 这些在 Windows 上是设备，不是文件
        (has_dos_device_name(path), "DOS 设备名"),
        // NUL 字节能截断底层 C 字符串，让检查看到的和实际打开的不是一个路径
        (s.contains('\0'), "NUL 字节"),
    ];

    for (hit, reason) in suspicious {
        if hit {
            return Err(FenceViolation::Suspicious {
                path: path.to_path_buf(),
                reason,
            });
        }
    }

    Ok(())
}

/// 纯字符串的路径归一化。消掉 `.` 和 `..`，不解析 symlink。
///
/// `[约束]` 这**不能**替代 `canonicalize`。`a/link/../b` 在字面上归一成
/// `a/b`，但如果 `link` 是符号链接，实际的 `..` 是相对链接目标的父目录，
/// 两者可能完全不同。这里只是第一道筛子。
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();

    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                // 到根了还在往上走：不要越过根，也不要保留 `..`
                if !out.pop() {
                    // 相对路径向上逃逸。保留 `..` 让后续的 within 判定失败。
                    out.push("..");
                }
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }

    out
}

fn has_ads(s: &str) -> bool {
    // 冒号有两处是合法的前缀语法，不是数据流分隔符：盘符（`C:\`），以及
    // verbatim 前缀里的盘符（`\\?\C:\`）。后者是 `canonicalize` 在 Windows
    // 上的正常输出形态，漏掉它会把每一个解析过的路径都判成数据流。
    //
    // 用字符串而不是 `Path::components`：`C:\x` 在非 Windows 上不被识别成
    // Prefix，整段会当成一个分量，盘符的冒号就成了误判 —— 而这些检查一律
    // 不分平台执行。
    strip_prefix_colon(s).contains(':')
}

/// 剥掉开头那段"冒号属于前缀语法"的部分，返回真正该检查数据流的尾巴。
fn strip_prefix_colon(s: &str) -> &str {
    let rest = s
        .strip_prefix(r"\\?\")
        .or_else(|| s.strip_prefix(r"\\.\"))
        .unwrap_or(s);

    if rest.len() > 2 && rest.as_bytes()[1] == b':' {
        &rest[2..]
    } else {
        rest
    }
}

/// 把 Windows `canonicalize` 返回的 verbatim 前缀（`\\?\C:\` / `\\?\UNC\`）
/// 剥回常规写法。非 Windows 路径没有 Prefix 组件，原样返回。
///
/// `[约束]` `canonicalize` 的结果在参与形状检查、字符串比较或显示之前都要
/// 过这一道。不剥的话有三笔账：
/// 1. [`check_shape`] 会把 `\\?\` 当成设备路径前缀、把盘符的冒号当成数据流，
///    于是每一个解析过的路径都被拒；
/// 2. `Path::starts_with` 按组件比较，`VerbatimDisk(D)` 和 `Disk(D)` 不相等，
///    两侧只要一边带前缀，前缀判定就全不成立；
/// 3. 前缀漏进配置或界面时，用户手选的 `D:\x` 和回来的 `\\?\D:\x` 对不上。
///
/// 超长路径不受影响：std 的 fs 调用在需要时会自己把路径转回 verbatim 形式，
/// 剥掉只影响字符串形态，不影响能力。
pub fn strip_verbatim(p: PathBuf) -> PathBuf {
    use std::path::Prefix;

    let mut comps = p.components();
    let Some(Component::Prefix(pre)) = comps.next() else {
        return p;
    };
    let base = match pre.kind() {
        Prefix::VerbatimDisk(d) => PathBuf::from(format!(r"{}:\", d as char)),
        Prefix::VerbatimUNC(server, share) => {
            let mut s = std::ffi::OsString::from(r"\\");
            s.push(server);
            s.push(r"\");
            s.push(share);
            PathBuf::from(s)
        }
        // `\\?\pipe\…` 这类没有常规等价形式，保持原样。
        _ => return p,
    };

    let mut out = base;
    for c in comps {
        if !matches!(c, Component::RootDir) {
            out.push(c.as_os_str());
        }
    }
    out
}

fn has_short_name(s: &str) -> bool {
    s.split(['/', '\\']).any(|seg| {
        let Some(pos) = seg.find('~') else {
            return false;
        };
        // PROGRA~1 —— 波浪号后面跟数字
        seg[pos + 1..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
    })
}

fn has_trailing_dot_or_space(path: &Path) -> bool {
    path.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        // `.` 和 `..` 是正常的路径分量
        if s == "." || s == ".." {
            return false;
        }
        s.ends_with('.') || s.ends_with(' ')
    })
}

fn has_dos_device_name(path: &Path) -> bool {
    const DEVICES: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];

    path.components().any(|c| {
        let s = c.as_os_str().to_string_lossy().to_ascii_uppercase();
        // `NUL.txt` 也是设备 —— Windows 只看第一个点之前的部分
        let stem = s.split('.').next().unwrap_or(&s);
        DEVICES.contains(&stem)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn 项目目录之外的路径不再被拦() {
        // 这里曾经有一整组"越界即拒"的用例。边界撤掉了：项目目录只用来
        // 给会话分类、并作为相对路径的基准，参考隔壁仓库、改 monorepo 的
        // 兄弟包、读一份共享配置都是正当操作，不该在这一层被否掉。
        //
        // `[前提]` 危险操作现在由权限层挡：默认模式逐次询问，
        // 敏感目标（SSH / 凭证 / shell 启动脚本）走 safety 且对放行免疫。
        // 改那两处之前，先想清楚这里已经没有第二道网了。
        for path in [
            "/etc/passwd",
            "/Users/someone/other-repo/src/main.rs",
            "/work/../etc/passwd",
        ] {
            assert_eq!(check_shape(&p(path)), Ok(()), "{path} 不该被拦");
        }
    }

    #[test]
    fn 形状可疑的路径仍然被拒() {
        // 和边界无关：这些构造本身就不该出现在一个正常路径里。
        for evil in [
            "/work/notes.txt:hidden", // NTFS 备用数据流
            "/work/NUL",              // DOS 设备名
            "/work/foo.txt.",         // 尾部的点
            "/work/PROGRA~1/x",       // 8.3 短名
        ] {
            assert!(
                matches!(check_shape(&p(evil)), Err(FenceViolation::Suspicious { .. })),
                "{evil} 应该被形状检查拦下"
            );
        }
    }

    // ── 路径别名构造 ──────────────────────────────────

    #[test]
    fn ntfs_备用数据流被拒() {
        // file.txt:evil 写的是另一份内容，目录列表里只看得到 file.txt
        let err = check_shape(&p("/work/notes.txt:hidden")).expect_err("应该拒绝");
        assert!(matches!(err, FenceViolation::Suspicious { .. }));
    }

    #[test]
    fn windows_盘符不算数据流() {
        assert_eq!(check_shape(&p("C:\\work\\a.txt")), Ok(()));
    }

    #[test]
    fn verbatim_前缀里的盘符不算数据流() {
        // 回归：`canonicalize` 在 Windows 上返回 `\\?\D:\…`，盘符的冒号一度
        // 被当成数据流分隔符。这条路径确实该拒（字面输入不该带 `\\?\`），
        // 但理由必须是前缀，不能是数据流 —— 否则给模型的纠错指令是错的，
        // 它会去改一个根本不存在的"数据流"写法。
        let err = check_shape(&p(r"\\?\D:\work\a.txt")).expect_err("字面 verbatim 该拒");
        let FenceViolation::Suspicious { reason, .. } = err;
        assert_eq!(reason, "Win32 设备路径前缀");
    }

    #[test]
    fn 盘符路径里的数据流照样被抓() {
        // 剥前缀不能把 ADS 检查一起剥没了。这是 `resolve` 剥完前缀之后的形态。
        assert!(check_shape(&p(r"D:\work\a.txt:hidden")).is_err());
    }

    /// `strip_verbatim` 靠 `Component::Prefix` 识别前缀，而非 Windows 平台不
    /// 解析 `\\?\` —— 这几条只有在 Windows 上才验证得到真实行为。
    #[cfg(windows)]
    #[test]
    fn 解析结果剥掉前缀后完全通过() {
        // 这是 `resolve` 的真实路径：canonicalize 给回 verbatim，剥掉之后再做
        // 形状检查。忘了剥，Windows 上每一次 Read/Write 都会栽在第二道检查上。
        for raw in [r"\\?\D:\work\pkg.json", r"\\?\UNC\srv\share\pkg.json"] {
            let stripped = strip_verbatim(p(raw));
            assert_eq!(check_shape(&stripped), Ok(()), "{raw} 剥完该放行");
        }
    }

    #[cfg(windows)]
    #[test]
    fn strip_verbatim_的三种前缀() {
        assert_eq!(strip_verbatim(p(r"\\?\D:\work\Riot")), p(r"D:\work\Riot"));
        assert_eq!(
            strip_verbatim(p(r"\\?\UNC\srv\share\dir")),
            p(r"\\srv\share\dir")
        );
        // 没有常规等价形式的，保持原样
        assert_eq!(strip_verbatim(p(r"\\?\pipe\x")), p(r"\\?\pipe\x"));
    }

    #[test]
    fn strip_verbatim_不碰普通路径() {
        for ok in ["/work/a.txt", r"C:\work\a.txt", "relative/a.txt"] {
            assert_eq!(strip_verbatim(p(ok)), p(ok), "{ok}");
        }
    }

    #[test]
    fn 设备路径前缀被拒() {
        for evil in ["\\\\?\\C:\\work\\a.txt", "\\\\.\\PhysicalDrive0"] {
            assert!(
                check_shape(&p(evil)).is_err(),
                "`\\\\?\\` 能造出 `..` 不被处理的路径：{evil}"
            );
        }
    }

    #[test]
    fn 短文件名被拒() {
        // PROGRA~1 指向 "Program Files"，规则匹配不上
        assert!(check_shape(&p("C:\\PROGRA~1\\evil")).is_err());
        // 普通的波浪号不该误伤
        assert_eq!(check_shape(&p("/work/my~notes.txt")), Ok(()));
    }

    #[test]
    fn 尾部点和空格被拒() {
        // Windows 会静默去掉它们，`foo.txt.` 实际写的是 `foo.txt`
        assert!(check_shape(&p("/work/foo.txt.")).is_err());
        assert!(check_shape(&p("/work/foo.txt ")).is_err());
    }

    #[test]
    fn 正常的点点分量不误伤() {
        assert_eq!(check_shape(&p("/work/../work/a.txt")), Ok(()));
        assert_eq!(check_shape(&p("./a.txt")), Ok(()));
    }

    #[test]
    fn dos_设备名被拒() {
        for dev in ["/work/CON", "/work/nul", "/work/COM1", "/work/NUL.txt"] {
            assert!(check_shape(&p(dev)).is_err(), "{dev} 在 Windows 上是设备");
        }
    }

    #[test]
    fn 设备名不误伤正常文件() {
        for ok in ["/work/console.rs", "/work/nullable.ts", "/work/context"] {
            assert_eq!(check_shape(&p(ok)), Ok(()), "{ok}");
        }
    }

    #[test]
    fn nul_字节被拒() {
        // NUL 能截断底层 C 字符串，让检查看到的和实际打开的不是一个路径
        assert!(check_shape(&p("/work/a.txt\0/../../etc/passwd")).is_err());
    }

    // ── normalize ─────────────────────────────────────

    #[test]
    fn 归一化消掉点和点点() {
        assert_eq!(normalize(&p("/a/./b/../c")), p("/a/c"));
        assert_eq!(normalize(&p("a/b/../../c")), p("c"));
    }

    #[test]
    fn 归一化不越过根() {
        assert_eq!(normalize(&p("/../../etc")), p("/etc"));
    }

    #[test]
    fn 相对路径向上逃逸时保留点点() {
        // 保留 `..` 让后续的 within 判定失败，而不是静默消掉
        assert_eq!(normalize(&p("../etc")), p("../etc"));
    }
}
