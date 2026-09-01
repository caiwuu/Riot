//! 应用内预览能读到哪些文件。
//!
//! `read_image` / `read_file_bytes` 收的是前端给的绝对路径，而 webview 里
//! 任何一段能发 IPC 的代码都调得动它们。不加边界的话，这两条就是一对
//! "读本机任意文件"的通用原语 —— 而同一个 webview 还有网络出口。
//! [`crate::fence`] 的模块文档把这条写死了：绕过围栏直接用用户传入路径的
//! 地方都是漏洞。
//!
//! # 边界画在哪
//!
//! 围栏的根不是单个工作区，而是"用户可能在界面上点开的东西"的并集：
//!
//! - **已登记的项目根 / 会话根**：聊天里的引用块、改动列表、Markdown 链接
//!   指的都是这里；
//! - **应用自己的数据目录**：截图原图、HAR、报告这些工件落在
//!   `<config>/artifacts/<会话>/`，界面点开的就是它们；
//! - **用户的常用文件夹**（桌面 / 下载 / 文档 / 图片 / 影片 / 音乐）：拖进
//!   输入框、或从系统对话框选的图，绝大多数来自这几处。
//!
//! 剩下的一律拒。挡掉的是 `~/.ssh`、`~/.aws`、`~/.config`、
//! `~/Library/Keychains`、`/etc` 这一类 —— 它们不在任何一条预览入口的语义
//! 里，却正是这对原语被利用时的第一批目标。
//!
//! `[取舍]` 更严的做法是只认"经系统对话框选中、或已经在对话流里展示过"的
//! 短期路径白名单。做不了：对话框和拖放都由前端直接调 Tauri 插件拿路径，
//! 宿主全程看不见，要收口就得改前端。这里的多根围栏是够得着的最紧边界。
//!
//! 副作用是外接盘、网络卷、`/opt` 这类位置上的文件预览不了。给用户的出路
//! 是"把它所在的目录作为项目打开"，错误文案里写明了。
//!
//! 豁免理由：和 [`crate::fence`] 同一条 —— 判断越界要看真实的 inode 和
//! symlink，注入 FileSystem 抽象会让这个检查失去意义。

#![allow(clippy::disallowed_methods)]

use std::path::PathBuf;

use crate::fence::Fence;
use crate::state::AppState;

/// 预览围栏的根集合。目录不存在的会在 [`resolve`] 里被跳过。
async fn roots(state: &AppState) -> Vec<PathBuf> {
    let config = state.config().await;
    let mut out: Vec<PathBuf> = config.projects.iter().map(PathBuf::from).collect();

    // 会话根不一定还在项目列表里（用户可以把项目从侧栏移除、而磁盘上的
    // 会话记录还在），但那个会话的历史照样能打开、里面的文件照样能点。
    out.extend(
        state
            .list_sessions()
            .await
            .into_iter()
            .map(|s| PathBuf::from(s.root)),
    );

    if let Some(data) = crate::config::config_path().parent() {
        out.push(data.to_path_buf());
    }

    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = PathBuf::from(home);
        // 两套名字都列上：Windows 是 Videos，macOS / Linux 是 Movies。
        // 不存在的名字不会有任何代价。
        for name in [
            "Desktop",
            "Downloads",
            "Documents",
            "Pictures",
            "Movies",
            "Videos",
            "Music",
        ] {
            out.push(home.join(name));
        }
    }

    // 项目根和会话根大量重复（一个项目下十几个会话是常态），而每个根都要
    // 走一次 canonicalize —— 预览每张图都白花这些系统调用。
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.clone()));
    out
}

/// 把前端传来的路径解析成一个允许预览的绝对路径。
///
/// `[约束]` 这是 `read_image` / `read_file_bytes` 唯一该用的入口。它们不做
/// 别的检查就直接读 `path` 的那个版本，等于把任意文件读取暴露给 webview。
pub async fn resolve(state: &AppState, requested: &str) -> Result<PathBuf, String> {
    resolve_in(&roots(state).await, requested).ok_or_else(|| {
        format!(
            "{requested} 不在应用能读的范围内。可读的是项目目录、应用自己的数据目录，\
             以及桌面 / 下载 / 文档 / 图片这些常用文件夹。要用别处的文件，\
             把它所在的目录作为项目打开，或者先拷到上面这些位置。"
        )
    })
}

/// 在一组根里挨个试。哪些根，见 [`roots`]。
fn resolve_in(roots: &[PathBuf], requested: &str) -> Option<PathBuf> {
    let mut fallback: Option<PathBuf> = None;

    for root in roots {
        // 根本身不在磁盘上（项目被删、家目录里没有 Movies）就跳过 ——
        // 建不出围栏不代表请求越界。
        let Ok(fence) = Fence::new(root) else {
            continue;
        };
        let Ok(resolved) = fence.resolve(requested) else {
            continue;
        };
        // 相对路径会被拼到每一个根上，第一个根总会"接受"它。先挑真的存在
        // 的那个，否则多项目下点开的永远是第一个项目里的同名文件。
        if resolved.exists() {
            return Some(resolved);
        }
        fallback.get_or_insert(resolved);
    }

    // 拼得出来但文件不存在时也放行:让下游的读操作去报"读不到文件"——
    // 那条错误指的是真实原因，比一句"不在范围内"准。
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;

    // 测的是 `resolve_in`：根集合的装配（读配置、读会话表）要一整个
    // AppState，而这些用例盯的是"给定一组根，哪些路径该放行"。

    /// 这条是这个模块存在的理由：webview 里的一行 `invoke` 不能把
    /// `~/.ssh/id_rsa` 读走。围栏外的绝对路径必须一个都过不去。
    #[test]
    fn 围栏外的绝对路径读不到() {
        let project = tempfile::tempdir().expect("建项目目录");
        let outside = tempfile::tempdir().expect("建围栏外的目录");
        let secret = outside.path().join("id_rsa");
        std::fs::write(&secret, "私钥").expect("写密钥");

        let roots = vec![project.path().to_path_buf()];
        assert_eq!(
            resolve_in(&roots, &secret.display().to_string()),
            None,
            "围栏外的文件被放行了"
        );
        assert_eq!(resolve_in(&roots, "../../../etc/passwd"), None);
    }

    /// 工件（截图原图、HAR、报告）在应用数据目录下，不在任何项目里。
    /// 把它们挡掉的话，聊天里的截图点开就是一片空白。
    #[test]
    fn 项目内和应用数据目录内的文件都放行() {
        let project = tempfile::tempdir().expect("建项目目录");
        let data = tempfile::tempdir().expect("建数据目录");
        std::fs::write(project.path().join("a.png"), "图").expect("写项目文件");
        let shot = data.path().join("artifacts/s1/t1.jpg");
        std::fs::create_dir_all(shot.parent().expect("有父目录")).expect("建工件目录");
        std::fs::write(&shot, "图").expect("写工件");

        let roots = vec![project.path().to_path_buf(), data.path().to_path_buf()];
        assert!(
            resolve_in(&roots, &project.path().join("a.png").display().to_string()).is_some(),
            "项目内的文件该放行"
        );
        assert!(
            resolve_in(&roots, &shot.display().to_string()).is_some(),
            "工件目录里的截图该放行"
        );
    }

    /// 同名文件在两个项目里都存在时，点开的必须是请求指到的那个。
    /// 只看"第一个接受的根"的话，相对路径会被拼到第一个项目上。
    #[test]
    fn 相对路径落在真的存在文件的那个根上() {
        let a = tempfile::tempdir().expect("建项目 A");
        let b = tempfile::tempdir().expect("建项目 B");
        std::fs::write(b.path().join("only-in-b.txt"), "B").expect("写文件");

        let roots = vec![a.path().to_path_buf(), b.path().to_path_buf()];
        let got = resolve_in(&roots, "only-in-b.txt").expect("该解析得出来");
        // 比的是 canonicalize 之后的根：macOS 的临时目录是 /var → /private/var
        // 的软链，拿原始路径比会假红。
        let b_real = std::fs::canonicalize(b.path()).expect("canonicalize");
        assert!(got.starts_with(&b_real), "解析到了 {}", got.display());
    }

    /// 围栏内一个指向围栏外的符号链接，跟着它走就等于没有围栏。
    #[cfg(unix)]
    #[test]
    fn 指向围栏外的符号链接读不到() {
        let project = tempfile::tempdir().expect("建项目目录");
        let outside = tempfile::tempdir().expect("建围栏外的目录");
        let secret = outside.path().join("id_rsa");
        std::fs::write(&secret, "私钥").expect("写密钥");
        std::os::unix::fs::symlink(&secret, project.path().join("link")).expect("建链接");

        let roots = vec![project.path().to_path_buf()];
        assert_eq!(resolve_in(&roots, "link"), None);
    }
}
