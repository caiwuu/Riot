//! 侧边抽屉的 Git 改动:工作区相对 HEAD 的未提交差异。
//!
//! 和 `changes`(会话改动)回答的问题不同:那边是"**这个会话**经工具改了
//! 什么",commit 之后依然在;这边跟着 git 走,commit 之后清零。两个面板
//! 并存,各答各的。
//!
//! # 为什么这里走 git 命令而不是自己扫目录
//!
//! rename 检测(`--find-renames`)、ignore 规则、staged/unstaged 合并 ——
//! 这些 git 都已经做对了,自己重造哪个都不值。会话改动那边反过来:它要
//! 回答的问题 git 答不了(见 changes.rs 头注),所以才自己记基线。
//!
//! 豁免理由:宿主层,跑的是用户自己仓库里的只读 git 命令。

#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use riot_protocol::changes::{ChangeStatus, FileChange, GitChanges};

/// 单条 git 命令的上限。比 git.rs 的快照宽一点:这是用户主动打开面板,
/// 等一下可以接受;但也不能没有底 —— 冷缓存的大仓库能把 status 跑出十几秒。
const GIT_TIMEOUT: Duration = Duration::from_secs(5);

/// 超过这么多文件就只列清单、不算逐行 diff。防的是"整个目录都没提交"
/// 的仓库:每个文件都要一次 `git show`,几千个文件能把面板卡上半分钟。
const MAX_DIFF_FILES: usize = 400;

/// 收集工作区相对所选基线的差异(staged + unstaged + untracked)。
///
/// `want` 是用户选的分支;对不上或为空就退回当前分支 / HEAD。
/// 只拿来 `git diff`,不 checkout。
pub async fn collect(cwd: &Path, want: Option<&str>) -> GitChanges {
    // 从仓库根算而不是会话目录:git 的 diff 输出路径一律相对仓库根,
    // 统一到这个基准,路径解析和显示才不用来回换算。
    let Some(top) = git_str(cwd, &["rev-parse", "--show-toplevel"]).await else {
        return GitChanges {
            repo: false,
            changes: Vec::new(),
            branch: None,
            base: None,
            refs: Vec::new(),
        };
    };
    let top = PathBuf::from(top);
    let refs = list_refs(&top).await;
    let branch = current_branch(&top).await;
    let base = resolve_base(&top, want, branch.as_deref(), &refs).await;

    // (status, 改名前的旧路径, 路径)。路径一律相对仓库根。
    let mut entries: Vec<(ChangeStatus, Option<String>, String)> = Vec::new();

    if let Some(base) = base.as_deref()
        && let Some(out) = git_raw(
            &top,
            &["diff", base, "--find-renames", "--name-status", "-z"],
        )
        .await
    {
        parse_name_status(&out, &mut entries);
    }
    // untracked 基本和 `diff HEAD` 不重叠(后者只看已跟踪的文件)。
    // 唯一的交集是 `git rm --cached`:索引里删了、文件还在磁盘上 ——
    // diff 报 D、ls-files 报未跟踪。合成一条"修改"(相对 HEAD 的净差异),
    // 不然同一路径出两行,一行说删了一行说新增,谁看谁糊涂。
    let mut index_of: std::collections::HashMap<String, usize> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.2.clone(), i))
        .collect();
    if let Some(out) = git_raw(&top, &["ls-files", "--others", "--exclude-standard", "-z"]).await {
        for p in split_z(&out) {
            if let Some(&i) = index_of.get(&p) {
                if entries[i].0 == ChangeStatus::Deleted {
                    entries[i].0 = ChangeStatus::Modified;
                }
                continue;
            }
            index_of.insert(p.clone(), entries.len());
            entries.push((ChangeStatus::Created, None, p));
        }
    }

    let with_hunks = entries.len() <= MAX_DIFF_FILES;
    let mut changes = Vec::with_capacity(entries.len());
    for (status, from, path) in entries {
        changes.push(one(&top, base.as_deref(), status, from, path, with_hunks).await);
    }
    // 按路径排序:每次打开面板顺序都一样,眼睛才能记住位置。
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    GitChanges {
        repo: true,
        changes,
        branch,
        base,
        refs,
    }
}

/// 本地分支 + 远程跟踪分支。`origin/HEAD` 是符号别名,选了只会让人糊涂。
async fn list_refs(top: &Path) -> Vec<String> {
    let Some(out) = git_str(
        top,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "--sort=-committerdate",
            "refs/heads",
            "refs/remotes",
        ],
    )
    .await
    else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    let mut refs = Vec::new();
    for name in out.lines().map(str::trim).filter(|s| !s.is_empty()) {
        if name.ends_with("/HEAD") || !seen.insert(name.to_owned()) {
            continue;
        }
        refs.push(name.to_owned());
    }
    refs
}

async fn current_branch(top: &Path) -> Option<String> {
    let raw = git_str(top, &["rev-parse", "--abbrev-ref", "HEAD"]).await?;
    let name = raw.trim();
    if name.is_empty() || name == "HEAD" {
        None
    } else {
        Some(name.to_owned())
    }
}

/// 用户选的基线必须是我们列出来的 ref,或字面量 HEAD。对不上就退回
/// 当前分支 / HEAD —— 不把任意字符串丢给 git(防 `-c` / `..` 这类)。
async fn resolve_base(
    top: &Path,
    want: Option<&str>,
    branch: Option<&str>,
    refs: &[String],
) -> Option<String> {
    for cand in [want, branch, Some("HEAD")] {
        let Some(name) = cand.filter(|s| !s.is_empty()) else {
            continue;
        };
        if !safe_ref(name) {
            continue;
        }
        if name != "HEAD" && !refs.iter().any(|r| r == name) && Some(name) != branch {
            continue;
        }
        let spec = format!("{name}^{{commit}}");
        if git_raw(top, &["rev-parse", "--verify", "--quiet", &spec])
            .await
            .is_some()
        {
            return Some(name.to_owned());
        }
    }
    None
}

fn safe_ref(name: &str) -> bool {
    !name.starts_with('-')
        && !name.contains([' ', '\0', ':', '\\', '^', '~', '?', '*', '['])
        && !name.contains("..")
}

/// `--name-status -z` 的输出:`状态 NUL 路径 NUL`,R/C 多一个源路径字段。
fn parse_name_status(raw: &[u8], out: &mut Vec<(ChangeStatus, Option<String>, String)>) {
    let mut it = split_z(raw).into_iter();
    while let Some(st) = it.next() {
        let Some(kind) = st.chars().next() else {
            continue;
        };
        match kind {
            // R100 = 纯改名,R087 = 改名加改动;分数只影响 diff,状态一样。
            'R' | 'C' => {
                let (Some(src), Some(dst)) = (it.next(), it.next()) else {
                    break;
                };
                if kind == 'R' {
                    out.push((ChangeStatus::Renamed, Some(src), dst));
                } else {
                    // 拷贝的目标对用户来说就是个新文件。
                    out.push((ChangeStatus::Created, None, dst));
                }
            }
            'A' => {
                let Some(p) = it.next() else { break };
                out.push((ChangeStatus::Created, None, p));
            }
            'D' => {
                let Some(p) = it.next() else { break };
                out.push((ChangeStatus::Deleted, None, p));
            }
            // M / T(类型变化)/ U(冲突)都按"修改"给用户看。
            _ => {
                let Some(p) = it.next() else { break };
                out.push((ChangeStatus::Modified, None, p));
            }
        }
    }
}

async fn one(
    top: &Path,
    base: Option<&str>,
    status: ChangeStatus,
    renamed_from: Option<String>,
    path: String,
    with_hunks: bool,
) -> FileChange {
    let mut change = FileChange {
        path,
        status,
        added: 0,
        removed: 0,
        hunks: Vec::new(),
        // 文件太多没算 diff 时如实标出来,界面会提示"去看文件本身"。
        truncated: !with_hunks,
        renamed_from,
        binary: false,
    };
    if !with_hunks {
        return change;
    }

    // before = 基线里的旧内容(新增没有);after = 工作区现状(删除没有)。
    let before: Option<Vec<u8>> = match status {
        ChangeStatus::Created => None,
        _ => {
            let old = change.renamed_from.as_deref().unwrap_or(&change.path);
            let spec = format!("{}:{old}", base.unwrap_or("HEAD"));
            git_raw(top, &["show", &spec]).await
        }
    };
    let after: Option<Vec<u8>> = match status {
        ChangeStatus::Deleted => None,
        _ => tokio::fs::read(top.join(&change.path)).await.ok(),
    };

    // 该有内容的一侧拿不到(submodule、权限、文件正被换掉…):
    // 宁可只给状态不给 diff,也不对着半份数据编一个假 diff。
    let missing_before = before.is_none() && !matches!(status, ChangeStatus::Created);
    let missing_after = after.is_none() && !matches!(status, ChangeStatus::Deleted);
    if missing_before || missing_after {
        change.truncated = true;
        return change;
    }

    if looks_binary(before.as_deref()) || looks_binary(after.as_deref()) {
        change.binary = true;
        return change;
    }

    let b = before.map(lossy).unwrap_or_default();
    let a = after.map(lossy).unwrap_or_default();
    // 纯重命名(内容一字未动)留在列表里,hunks 为空 —— 界面据此显示
    // "文件已重命名,内容未变更",这正是 git 视图比会话视图多出来的信息。
    if b == a {
        change.truncated = false;
        return change;
    }

    let (added, removed, hunks, truncated) = crate::changes::build_hunks(&b, &a);
    change.added = added;
    change.removed = removed;
    change.hunks = hunks;
    change.truncated = truncated;
    change
}

/// git 自己的二进制判定也是"开头有没有 NUL"。只看前 8KiB,够了。
fn looks_binary(bytes: Option<&[u8]>) -> bool {
    let Some(b) = bytes else { return false };
    b[..b.len().min(8192)].contains(&0)
}

fn lossy(v: Vec<u8>) -> String {
    String::from_utf8_lossy(&v).into_owned()
}

fn split_z(raw: &[u8]) -> Vec<String> {
    raw.split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

/// 跑一条只读 git 命令,拿原始字节。失败一律 `None` —— 面板宁可少一个
/// 文件的 diff,也不该因为一条命令挂了就整个报错。
async fn git_raw(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // 超时后 future 被 drop,进程必须跟着走,不然大仓库上会攒一堆。
        .kill_on_drop(true);
    // Windows:不带 CREATE_NO_WINDOW 的话,打包后的 GUI 主程序每刷一次
    // 变更面板就闪一串黑色控制台窗。理由的完整版见 riot-runtime 的命令执行器。
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let out = tokio::time::timeout(GIT_TIMEOUT, cmd.output())
        .await
        .ok()?
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// 文本型输出:trim 掉尾部换行。**不要**用它拿文件内容 —— trim 会吃掉
/// 内容本身的首尾空白,diff 就不准了,拿内容走 [`git_raw`]。
async fn git_str(root: &Path, args: &[&str]) -> Option<String> {
    let out = git_raw(root, args).await?;
    Some(String::from_utf8_lossy(&out).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 在临时目录里搭一个真仓库跑全链路 —— name-status 解析、rename
    /// 检测、untracked、二进制,全是 git 的真实输出,不是手写的样例。
    async fn sh(dir: &Path, args: &[&str]) {
        let ok = tokio::process::Command::new(args[0])
            .args(&args[1..])
            .current_dir(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .expect("命令能跑")
            .success();
        assert!(ok, "命令失败: {args:?}");
    }

    async fn init_repo(dir: &Path) {
        sh(dir, &["git", "init", "-q", "-b", "main"]).await;
        // 测试环境可能没有全局身份;提交需要。
        sh(dir, &["git", "config", "user.email", "t@t"]).await;
        sh(dir, &["git", "config", "user.name", "t"]).await;
    }

    #[tokio::test]
    async fn 不是仓库时_repo_为假() {
        let dir = tempfile::tempdir().expect("临时目录");
        let got = collect(dir.path(), None).await;
        assert!(!got.repo);
        assert!(got.changes.is_empty());
    }

    #[tokio::test]
    async fn 认得出修改_新增_删除_和重命名() {
        let dir = tempfile::tempdir().expect("临时目录");
        let root = dir.path();
        init_repo(root).await;
        std::fs::write(root.join("a.txt"), "one\ntwo\n").expect("写");
        std::fs::write(root.join("gone.txt"), "bye\n").expect("写");
        std::fs::write(root.join("old_name.txt"), "内容完全一样,只是换个名字\n").expect("写");
        sh(root, &["git", "add", "."]).await;
        sh(root, &["git", "commit", "-q", "-m", "init"]).await;

        std::fs::write(root.join("a.txt"), "one\n2\n").expect("改");
        std::fs::remove_file(root.join("gone.txt")).expect("删");
        std::fs::rename(root.join("old_name.txt"), root.join("new_name.txt")).expect("改名");
        // 改名要 stage 之后 git 才配对得出来(git mv 同效)。没 stage 的话
        // git 自己也只报"删了旧的 + 多了个未跟踪的" —— 我们不比 git 聪明。
        sh(root, &["git", "add", "-A"]).await;
        // stage 之后再新建 —— 这个走 untracked 的路。
        std::fs::write(root.join("fresh.txt"), "hi\n").expect("新建");

        let got = collect(root, None).await;
        assert!(got.repo);

        let by = |p: &str| {
            got.changes
                .iter()
                .find(|c| c.path == p)
                .unwrap_or_else(|| panic!("缺 {p}: {:?}", got.changes))
        };

        let m = by("a.txt");
        assert_eq!(m.status, ChangeStatus::Modified);
        assert_eq!((m.added, m.removed), (1, 1));

        assert_eq!(by("gone.txt").status, ChangeStatus::Deleted);
        assert_eq!(by("fresh.txt").status, ChangeStatus::Created);
        assert_eq!(by("fresh.txt").added, 1);

        let r = by("new_name.txt");
        assert_eq!(r.status, ChangeStatus::Renamed, "{:?}", got.changes);
        assert_eq!(r.renamed_from.as_deref(), Some("old_name.txt"));
        assert!(r.hunks.is_empty(), "纯改名不该有 hunks");
        assert!(!r.truncated);
    }

    #[tokio::test]
    async fn 二进制文件只标状态不给行() {
        let dir = tempfile::tempdir().expect("临时目录");
        let root = dir.path();
        init_repo(root).await;
        std::fs::write(root.join("blob.bin"), [0u8, 159, 146, 150]).expect("写");

        let got = collect(root, None).await;
        let c = &got.changes[0];
        assert!(c.binary);
        assert!(c.hunks.is_empty());
    }

    #[tokio::test]
    async fn 干净仓库是空列表但_repo_为真() {
        let dir = tempfile::tempdir().expect("临时目录");
        let root = dir.path();
        init_repo(root).await;
        std::fs::write(root.join("a.txt"), "x\n").expect("写");
        sh(root, &["git", "add", "."]).await;
        sh(root, &["git", "commit", "-q", "-m", "init"]).await;

        let got = collect(root, None).await;
        assert!(got.repo);
        assert!(got.changes.is_empty(), "{:?}", got.changes);
        assert_eq!(got.branch.as_deref(), Some("main"));
        assert_eq!(got.base.as_deref(), Some("main"));
        assert!(got.refs.iter().any(|r| r == "main"));
    }

    #[tokio::test]
    async fn 换基线能看到已提交的分支差异() {
        let dir = tempfile::tempdir().expect("临时目录");
        let root = dir.path();
        init_repo(root).await;
        std::fs::write(root.join("a.txt"), "base\n").expect("写");
        sh(root, &["git", "add", "."]).await;
        sh(root, &["git", "commit", "-q", "-m", "init"]).await;
        sh(root, &["git", "checkout", "-q", "-b", "feat"]).await;
        std::fs::write(root.join("a.txt"), "feat\n").expect("改");
        sh(root, &["git", "add", "."]).await;
        sh(root, &["git", "commit", "-q", "-m", "feat"]).await;

        let vs_head = collect(root, None).await;
        assert!(
            vs_head.changes.is_empty(),
            "相对当前分支应干净: {:?}",
            vs_head.changes
        );

        let vs_main = collect(root, Some("main")).await;
        assert_eq!(vs_main.base.as_deref(), Some("main"));
        let a = vs_main
            .changes
            .iter()
            .find(|c| c.path == "a.txt")
            .expect("相对 main 应看到 a.txt");
        assert_eq!(a.status, ChangeStatus::Modified);
        assert_eq!((a.added, a.removed), (1, 1));
    }
}
