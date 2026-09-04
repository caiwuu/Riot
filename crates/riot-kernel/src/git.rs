//! git 快照：让模型一开口就知道自己站在哪。
//!
//! # 为什么值得每个会话花这几毫秒
//!
//! 不给的话，模型要么先跑一轮 `git status` 才敢动（一次完整的模型往返，
//! 就为了拿一行分支名），要么干脆不查 —— 后者更常见也更糟：它会在
//! 一个有未提交改动的工作区里 `git checkout`，或者在 main 上直接开干。
//!
//! # 只给慢变量
//!
//! 这份快照跟着**第一条**用户消息进历史，之后不再重发（压缩时会重注一次
//! 新的）。所以放进来的东西必须是"半小时内不会变"的那类：分支名、是不是
//! 仓库、有没有脏改动。逐文件的 diff 不在这里 —— 那个变得快，而且模型
//! 需要时自己跑 `git status` 就有，重复塞只会挤占上下文。
//!
//! 豁免理由：宿主层，跑的是用户自己仓库里的只读 git 命令。

#![allow(clippy::disallowed_methods)]

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

/// 单条 git 命令的上限。
///
/// 大仓库上冷缓存的 `git status` 能跑好几秒，而这是挡在用户第一句话
/// 前面的路径 —— 宁可这一轮没有 git 信息，也不能让发消息卡住。
const GIT_TIMEOUT: Duration = Duration::from_secs(3);

/// 最近提交带几条。够看出"这个仓库最近在干什么"，又不至于占太多上下文。
const RECENT_COMMITS: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitInfo {
    /// 分支名；detached HEAD 时是短 sha。
    pub branch: String,
    pub detached: bool,
    /// 有改动（含未跟踪）的文件数。
    pub dirty: usize,
    /// 最近几条提交，每条一行 `sha 标题`。
    pub recent: Vec<String>,
}

/// 探测工作目录的 git 状态。`None` = 不是 git 仓库（或 git 不可用）。
pub async fn probe(root: &Path) -> Option<GitInfo> {
    // 先确认是仓库。不是的话后面几条全都会失败，没必要跑。
    let inside = git(root, &["rev-parse", "--is-inside-work-tree"]).await?;
    if inside.trim() != "true" {
        return None;
    }

    // 三条命令互不依赖，并行跑 —— 串行的话在大仓库上要等三个来回。
    let (branch, status, log) = tokio::join!(
        git(root, &["rev-parse", "--abbrev-ref", "HEAD"]),
        git(root, &["status", "--porcelain"]),
        git(
            root,
            &["log", "--oneline", "--no-decorate", "-n", "5", "--"],
        ),
    );

    // `--abbrev-ref HEAD` 在 detached 状态返回字面量 "HEAD"，那不是分支名。
    // 退回短 sha —— 告诉模型"你不在任何分支上"比给它一个假分支名重要。
    let raw = branch.unwrap_or_default();
    let detached = raw.trim() == "HEAD" || raw.trim().is_empty();
    let branch = if detached {
        git(root, &["rev-parse", "--short", "HEAD"])
            .await
            .unwrap_or_else(|| "unknown".to_owned())
    } else {
        raw.trim().to_owned()
    };

    Some(GitInfo {
        branch,
        detached,
        dirty: status
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count(),
        recent: log
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .take(RECENT_COMMITS)
            .map(str::to_owned)
            .collect(),
    })
}

/// 渲染成注入给模型的那段文字。
pub fn describe(info: &GitInfo) -> String {
    let mut s = String::from("Git repository\n");
    if info.detached {
        s.push_str(&format!(
            "Not on any branch right now (detached HEAD, {}). Commits made here are easily \
             lost, so ALWAYS ask the user whether to create a branch before you start.\n",
            info.branch
        ));
    } else {
        s.push_str(&format!("Current branch: {}\n", info.branch));
    }

    if info.dirty > 0 {
        // 这句是防事故的。模型看不到工作区脏不脏时，会若无其事地
        // checkout / stash / reset，把用户还没提交的活儿冲掉。
        s.push_str(&format!(
            "The working tree has {} uncommitted file(s). ALWAYS ask before switching branches, \
             stashing, or resetting — those changes may be the user's own, and there is no way \
             to get them back.\n",
            info.dirty
        ));
    } else {
        s.push_str("The working tree is clean.\n");
    }

    if !info.recent.is_empty() {
        s.push_str("Recent commits (as a model for how commit messages are written here):\n");
        for line in &info.recent {
            s.push_str("  ");
            s.push_str(line);
            s.push('\n');
        }
    }
    // `[约束]` 这句的措辞很要命。第一版写的是"要准确状态自己跑 git"——
    // 结果用户一问"我在哪个分支"，模型就老老实实去跑了一遍 git status，
    // 整份快照白注。既要说清它是快照，又必须明说"不用再查一遍"。
    s.push_str(
        "The above is the state at the start of this session. Unless you have used git yourself \
         since then (switching branches, committing, stashing), answer from this information \
         directly — there is no need to run git again to confirm it.",
    );
    s
}

/// 跑一条只读 git 命令。任何失败都返回 `None` —— git 信息是锦上添花，
/// 拿不到就不给，不该让它挡住用户发消息。
async fn git(root: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // 超时后这个 future 被 drop，进程得跟着走，否则大仓库上会攒下
        // 一堆还在扫描的 git。
        .kill_on_drop(true);
    // Windows:不带 CREATE_NO_WINDOW 的话，打包后的 GUI 主程序每问一次
    // git 就闪一个黑色控制台窗。理由的完整版见 riot-runtime 的命令执行器。
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW

    let out = tokio::time::timeout(GIT_TIMEOUT, cmd.output())
        .await
        .ok()?
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(branch: &str, detached: bool, dirty: usize) -> GitInfo {
        GitInfo {
            branch: branch.to_owned(),
            detached,
            dirty,
            recent: vec!["a1b2c3d 修好了终端".to_owned()],
        }
    }

    #[test]
    fn 干净和脏工作区说法不同() {
        let clean = describe(&info("main", false, 0));
        assert!(clean.contains("Current branch: main"));
        assert!(clean.contains("The working tree is clean"));

        let dirty = describe(&info("main", false, 12));
        assert!(dirty.contains("12 uncommitted file(s)"));
        // 这句不能少：模型看不到脏工作区时会若无其事地 checkout。
        assert!(dirty.contains("ALWAYS ask before switching branches"));
    }

    /// detached 时 `--abbrev-ref HEAD` 会返回字面量 "HEAD"。把它当分支名
    /// 报出去，模型会以为有个叫 HEAD 的分支，然后往上面提交。
    /// 快照必须明说"不用再查"。
    ///
    /// 第一版结尾是"要准确状态自己跑 git"，于是模型每次被问到分支都去跑
    /// 一遍 `git status` —— 注入的信息一点没省下那一轮。
    #[test]
    fn 不能反过来诱导模型去跑_git() {
        let d = describe(&info("main", false, 0));
        assert!(d.contains("no need to run git again"), "{d}");
        assert!(!d.contains("run git yourself"), "别把模型推回去查：{d}");
    }

    #[test]
    fn detached_要说清不在分支上() {
        let d = describe(&info("a1b2c3d", true, 0));
        assert!(d.contains("detached"));
        assert!(!d.contains("当前分支："));
    }

    #[tokio::test]
    async fn 不是仓库时返回_none() {
        let dir = std::env::temp_dir().join(format!("riot-nogit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建目录");
        assert!(probe(&dir).await.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 跑在 Riot 自己的仓库上 —— 这条同时验证了命令拼写和解析。
    #[tokio::test]
    async fn 探测真实仓库() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("仓库根");
        let Some(got) = probe(root).await else {
            eprintln!("这里不是 git 仓库（打包源码树？），跳过");
            return;
        };
        assert!(!got.branch.is_empty(), "分支名不该是空的");
        assert!(!got.recent.is_empty(), "Riot 自己的仓库总该有提交");
    }
}
