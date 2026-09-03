//! 敏感操作安全检查。
//!
//! `[约束]` 这一层**对 bypass 模式免疫**。
//!
//! 判断标准很具体：这个操作会不会让 agent 取得超出"帮我写代码"范围的
//! 能力？改 `.zshrc` 会（下次开终端就执行了）、写 `.git/hooks/` 会
//! （下次 commit 就执行了）、读写 SSH 密钥会。改 `src/main.rs` 不会。
//!
//! 用户开 bypass 的时候想的是前者不会发生。
//!
//! 见 ARCHITECTURE.md §9.2 第 4 步

use std::path::Path;

use riot_protocol::permission::{
    PermissionContext, PermissionUpdate, RuleDecision, SafetyKind, UpdateScope,
};
use riot_protocol::tool::Tool;

#[derive(Debug, Clone, PartialEq)]
pub struct SafetyFinding {
    pub kind: SafetyKind,
    pub message: String,
    pub suggestions: Vec<PermissionUpdate>,
}

/// 检查一次工具调用有没有触碰敏感目标。
///
/// 返回 `None` 表示没问题。
pub fn check(
    tool: &dyn Tool,
    input: &serde_json::Value,
    _ctx: &PermissionContext,
) -> Option<SafetyFinding> {
    // 只读操作不在这一层管。读 `.zshrc` 不会让 agent 获得执行权，
    // 而把读也拦下来会让"看一眼配置"这种正常需求变得很烦。
    //
    // 凭证文件是例外 —— 读到就是泄露。
    let read_only = tool.is_read_only(input);
    let path = tool.target_path(input)?;

    let kind = classify_path(&path, read_only)?;

    Some(SafetyFinding {
        kind,
        message: describe(kind, &path),
        suggestions: vec![PermissionUpdate::AddRule {
            tool: tool.name().to_owned(),
            pattern: Some(path.to_string_lossy().into_owned()),
            decision: RuleDecision::Allow,
            // 敏感操作的"永久同意"只给会话级。写进配置文件意味着
            // 以后每次都静默放行，那个决定不该在一个弹窗里做完。
            scope: UpdateScope::Session,
        }],
    })
}

/// Bash 的参数/重定向目标碰没碰到敏感路径。
///
/// 存在的理由是 Bash 没有单一目标路径:`safety::check` 走
/// [`Tool::target_path`] 返回 `None`,整个路径检查对它不生效 ——
/// `echo evil >> ~/.zshrc` 里那个 `~/.zshrc` 没有任何一层看得见。
///
/// `read_only` 透传给 [`classify_path`],语义和 `safety::check` 完全一致:
/// 凭证类读到即泄露,读写都拦;其余几类只有**写**才换来执行权,只读放行。
/// 少了这个参数,`cat .git/config`(只读)会被当成写 `.git/` 拦成 Ask,
/// 而同一个读操作走 Write 工具却是放行的 —— 同一条不变量两处给出相反答案。
///
/// `[约束]` 这就是为什么 Bash 的重定向不能整类交给「全部放行」。命令
/// 分析器把重定向目标交到这里分级:敏感的仍然拦,普通的（`> /tmp/out.log`）
/// 才算作单纯的不确定性。见 `bash/ast.rs` 的 `redirect_target_risk`。
pub fn write_target_risk(path: &Path, read_only: bool) -> Option<SafetyKind> {
    classify_path(path, read_only)
}

fn classify_path(path: &Path, read_only: bool) -> Option<SafetyKind> {
    let normalized = fold_for_match(&path.to_string_lossy());

    // 凭证读了就是泄露，读写都拦
    if looks_like_credentials(&normalized) {
        return Some(SafetyKind::Credentials);
    }
    if contains_segment(&normalized, ".ssh") {
        return Some(SafetyKind::SshConfig);
    }

    // 以下几类是"获得执行权"，只有写才危险
    if read_only {
        return None;
    }

    if contains_segment(&normalized, ".git") {
        return Some(SafetyKind::GitInternals);
    }
    if is_shell_rc(&normalized) || is_autostart_surface(&normalized) {
        return Some(SafetyKind::ShellRc);
    }
    if contains_segment(&normalized, ".riot") || contains_segment(&normalized, ".claude") {
        return Some(SafetyKind::AgentConfig);
    }
    if is_toolchain_exec_surface(&normalized) {
        return Some(SafetyKind::ToolchainConfig);
    }

    None
}

/// 把路径折成"和文件系统同一个口径"的比较形态。
///
/// `[约束]` 下面每一份清单都是小写 ASCII，比对前必须过这个函数。少了
/// 大小写折叠就是一条零成本的整层绕过:macOS 的 APFS 和 Windows 的 NTFS
/// 默认大小写不敏感,`~/.ZSHRC` 打开的就是 `~/.zshrc` 那个文件,而
/// `canonicalize` **不会**把它纠回小写 —— 于是 `Write ~/.ZSHRC` 在
/// 「全部放行」下静默取得持久化执行权,`Read /work/.ENV` 连默认模式都不问。
///
/// 折叠不分平台。Linux 上 `.ZSHRC` 确实是另一个文件,代价是那种罕见
/// 文件名上多问一次;而漏判换来的是上面那条。这和本模块其余部分
/// （以及 [`crate::fence`]）"检查一律不分平台"的取舍一致 —— 只在
/// Windows/macOS 上折叠的话,这些用例在 Linux CI 上就跑不到。
///
/// 用 Unicode 的 `to_lowercase` 而不是 `to_ascii_lowercase`:APFS 折的是
/// 完整的 Unicode 大小写,`.doc\u{212a}er`（开尔文符号）在盘上就是
/// `.docker`,只折 ASCII 的话这一类同形字仍然能绕过去。
fn fold_for_match(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

/// 构建工具链里「写了就等于让下次构建执行任意代码」的位置。
///
/// `[约束]` 这几条**必须**在这里拦住，因为 OS 沙箱指望不上:
/// `~/.cargo`、`~/.rustup` 本来就在可写集里（不放开的话第一条
/// `cargo build` 就死在写不了缓存上，见 `riot_runtime::sandbox` 的取舍）。
/// 也就是说边界之内还藏着一条通往边界之外的路,只有这一层看得见。
///
/// 按「谁会被自动执行」筛:
/// - `.cargo/config.toml` 的 `[build] rustc-wrapper`、`[target.*] runner`、
///   `[alias]` 都在 `cargo build` 时被 spawn；
/// - `.cargo/bin`、`.rustup` 下面放的就是 `cargo` / `rustc` 本身；
/// - `.envrc` 是 direnv 的钩子,`cd` 进目录就执行。
///
/// 项目内的 `.cargo/config.toml` 和主目录下的同样危险,所以按**路径分量**
/// 匹配而不是绑定主目录。
fn is_toolchain_exec_surface(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or_default();
    if name == ".envrc" {
        return true;
    }
    if contains_segment(path, ".rustup") {
        return true;
    }
    matches!(
        segment_after(path, ".cargo"),
        Some("config.toml" | "config" | "bin")
    )
}

/// 紧跟在 `segment` 后面的那一段路径分量。
///
/// 用它而不是 `contains_segment`:`~/.cargo/registry/src/…/bin/main.rs`
/// 里那个 `bin` 只是某个 crate 的源码目录,和 `~/.cargo/bin` 不是一回事。
fn segment_after<'a>(path: &'a str, segment: &str) -> Option<&'a str> {
    let mut it = path.split('/');
    while let Some(s) = it.next() {
        if s == segment {
            return it.next();
        }
    }
    None
}

/// 按路径分量匹配，不是子串匹配。
///
/// `[约束]` 用子串的话 `src/legit.git-helper.rs` 会被误判成 `.git` 写入，
/// 而 `my.ssh.notes.md` 会被当成 SSH 配置。误报比漏报更快消耗掉用户的
/// 注意力 —— 弹窗多了他就不看内容直接点允许了，那时候真的危险操作也放行了。
fn contains_segment(path: &str, segment: &str) -> bool {
    path.split('/').any(|s| s == segment)
}

fn is_shell_rc(path: &str) -> bool {
    const RC_FILES: &[&str] = &[
        ".zshrc",
        ".bashrc",
        ".bash_profile",
        ".profile",
        ".zshenv",
        ".zprofile",
        ".fishrc",
        "config.fish",
        ".inputrc",
    ];

    let Some(name) = path.rsplit('/').next() else {
        return false;
    };

    RC_FILES.contains(&name)
        // fish 的配置在 ~/.config/fish/ 下，文件名不带点
        || contains_segment(path, "fish") && name.ends_with(".fish")
        // 系统级 profile
        || path.starts_with("/etc/profile")
        || contains_segment(path, "profile.d")
}

/// 登录 / 开机时自动拉起的位置，以及 PATH 上游的可写目录。
///
/// 和 [`is_shell_rc`] 归到同一个 [`SafetyKind::ShellRc`]:判据完全一样 ——
/// 写进去就换来一份不需要用户再做任何事就会执行的代码。触发点不同
/// （登录、定时器、下一次敲那个命令）不改变这件事的性质,而分成几档只会
/// 让弹窗文案更碎。
///
/// `[约束]` 这一档不能漏,否则 `.zshrc` 那条防线是可以绕开的:写
/// `~/Library/LaunchAgents/x.plist` 换来的持久执行权和改 `.zshrc` 等价,
/// 用户下次登录就中招,而他开「全部放行」时想的是"帮我写代码"。
///
/// `~/.local/bin` 在多数发行版的默认 PATH 里且排在 `/usr/bin` 前面 ——
/// 往里放一个 `git` 就劫持了之后每一次 `git`。
fn is_autostart_surface(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or_default();

    // macOS：登录时由 launchd 拉起
    if contains_segment(path, "launchagents") || contains_segment(path, "launchdaemons") {
        return true;
    }
    // Linux：systemd 的 user / system unit
    if name.ends_with(".service") || name.ends_with(".timer") {
        return true;
    }
    if name == "crontab" || contains_segment(path, "cron.d") || path.starts_with("/etc/cron") {
        return true;
    }
    if matches!(segment_after(path, ".local"), Some("bin")) {
        return true;
    }
    // git 的全局配置：`core.hooksPath` 和 `alias.*` 都是执行面，而
    // `.git` 那条分量检查只看得见仓库内的 `.git/config`。
    //
    // `.gitattributes` 不在这里 —— 它要配合 `.gitconfig` 里的 filter 定义
    // 才能执行，而它本身是仓库里每天都在改的正常文件。
    if name == ".gitconfig" {
        return true;
    }
    // 编辑器的任务配置：装了 runOnFolderOpen 一类扩展之后开项目就会跑
    matches!(name, "tasks.json" | "launch.json") && contains_segment(path, ".vscode")
}

fn looks_like_credentials(path: &str) -> bool {
    let Some(name) = path.rsplit('/').next() else {
        return false;
    };

    const EXACT: &[&str] = &[
        ".netrc",
        ".npmrc",
        ".pypirc",
        ".pgpass",
        "credentials",
        "id_rsa",
        "id_dsa",
        "id_ecdsa",
        "id_ed25519",
        "id_ecdsa_sk",
        "id_ed25519_sk",
        ".git-credentials",
        ".dockercfg",
    ];

    if EXACT.contains(&name) {
        return true;
    }

    // 私钥和密钥库的扩展名。按扩展名而不是文件名 —— 私钥叫什么全凭习惯
    // （`server.key`、`privkey.pem`、`prod-2024.p12`），穷举文件名是穷举
    // 不完的,而这几个扩展名的内容几乎不可能是别的东西。
    //
    // `[取舍]` `.key` 会误伤 macOS 的 Keynote 文稿。代价是读一份 `.key`
    // 多问一次;而 TLS 私钥叫 `server.key` 是 Web 项目里最常见的形态,
    // 正好是这个产品的用户群。
    const KEY_EXT: &[&str] = &[".pem", ".key", ".p12", ".pfx", ".jks", ".keystore"];
    if KEY_EXT.iter().any(|e| name.ends_with(e)) {
        return true;
    }

    // 云厂商的服务账号密钥，文件名有稳定前缀
    if name.starts_with("service-account") && name.ends_with(".json") {
        return true;
    }

    // .env 及其变体（.env.local、.env.production），但不含 .env.example
    if name == ".env" || (name.starts_with(".env.") && !name.ends_with(".example")) {
        return true;
    }

    // 云厂商凭证目录
    [".aws", ".gnupg", ".kube", ".docker"]
        .iter()
        .any(|d| contains_segment(path, d))
        && name != "config.example"
}

/// 弹窗里那句「为什么拦你」。
///
/// `pub` 是给 Bash 用的:它没有单一目标路径，走的是
/// [`crate::bash::write_targets`] 那条参数扫描,但拦下来之后要说的话
/// 和这里完全一样 —— 两处各写一份迟早漂移，而漂移的那侧不会报错。
pub fn describe(kind: SafetyKind, path: &Path) -> String {
    let p = path.display();
    match kind {
        SafetyKind::GitInternals => {
            format!("这会修改 Git 内部文件 {p}。写 .git/hooks/ 等于让下次提交自动执行代码。")
        }
        SafetyKind::SshConfig => format!("这会读写 SSH 配置或密钥 {p}。"),
        SafetyKind::ShellRc => {
            format!(
                "这会修改自动执行的配置 {p}。改这个等于取得持久化执行权 —— \
                 下次开终端、下次登录或下次敲那个命令时就会运行。"
            )
        }
        SafetyKind::AgentConfig => {
            format!("这会修改本应用自己的配置 {p}，可能影响后续的权限判断。")
        }
        SafetyKind::ToolchainConfig => format!(
            "这会修改构建工具链的配置或可执行文件 {p}。改这个等于取得持久化执行权 —— \
             下次构建就会运行，而那次构建不在沙箱里。"
        ),
        SafetyKind::Credentials => format!("{p} 看起来是凭证文件。"),
        SafetyKind::CommandInjection => format!("命令里检测到注入模式：{p}"),
        SafetyKind::UnparseableCommand => format!("无法解析这个命令：{p}"),
        // scope 不走文件路径的 safety::check，这一分支只为穷尽匹配存在;
        // 真正的 scope 提示由渗透工具自己拼（带目标域名）。
        SafetyKind::OutOfScope => format!("目标 {p} 不在授权的渗透范围内。"),
        // 出沙箱同样不走文件路径,真正的提示由 Bash 自己拼（带整条命令）。
        SafetyKind::SandboxEscape => {
            format!("{p} 会在 OS 沙箱之外执行，文件系统边界对它不生效。")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{PermTool, ctx_with};
    use pretty_assertions::assert_eq;
    use riot_protocol::permission::PermissionMode;

    fn found(tool: &PermTool, path: &str) -> Option<SafetyKind> {
        check(
            tool,
            &serde_json::json!({ "path": path }),
            &ctx_with(PermissionMode::Default),
        )
        .map(|f| f.kind)
    }

    fn on_write(path: &str) -> Option<SafetyKind> {
        found(&PermTool::writer("Write"), path)
    }

    fn on_read(path: &str) -> Option<SafetyKind> {
        found(&PermTool::read_only("Read"), path)
    }

    #[test]
    fn git_内部文件() {
        assert_eq!(
            on_write("/work/.git/config"),
            Some(SafetyKind::GitInternals)
        );
        assert_eq!(
            on_write("/work/.git/hooks/pre-commit"),
            Some(SafetyKind::GitInternals),
            "写 hooks 等于让下次提交自动执行代码"
        );
    }

    #[test]
    fn shell_启动脚本() {
        for rc in ["/home/u/.zshrc", "/home/u/.bashrc", "/home/u/.bash_profile"] {
            assert_eq!(on_write(rc), Some(SafetyKind::ShellRc), "{rc}");
        }
    }

    #[test]
    fn ssh_配置读写都拦() {
        assert_eq!(on_write("/home/u/.ssh/config"), Some(SafetyKind::SshConfig));
        assert_eq!(
            on_read("/home/u/.ssh/id_rsa"),
            Some(SafetyKind::Credentials),
            "私钥读到就是泄露"
        );
    }

    #[test]
    fn 凭证文件读也要拦() {
        // 其它几类只读不危险，凭证不一样 —— 读到就泄露了
        for cred in [
            "/work/.env",
            "/work/.env.production",
            "/home/u/.aws/credentials",
            "/home/u/.netrc",
        ] {
            assert_eq!(on_read(cred), Some(SafetyKind::Credentials), "{cred}");
        }
    }

    #[test]
    fn env_example_不算凭证() {
        // 这是模板文件，仓库里到处都是
        assert_eq!(on_read("/work/.env.example"), None);
    }

    #[test]
    fn 只读不触发非凭证类检查() {
        // 读 .zshrc 不会让 agent 获得执行权，
        // 把读也拦下来会让"看一眼配置"变得很烦
        assert_eq!(on_read("/home/u/.zshrc"), None);
        assert_eq!(on_read("/work/.git/config"), None);
    }

    #[test]
    fn 普通源文件不触发() {
        for ok in ["/work/src/main.rs", "/work/README.md", "/work/tests/env.rs"] {
            assert_eq!(on_write(ok), None, "{ok}");
        }
    }

    #[test]
    fn 按路径分量匹配而不是子串() {
        // 误报比漏报更快消耗用户的注意力 —— 弹窗多了他就不看内容直接点
        // 允许了，那时候真的危险操作也放行了。
        for false_positive in [
            "/work/src/legit.git-helper.rs",
            "/work/docs/my.ssh.notes.md",
            "/work/gitignore-parser.rs",
            "/work/src/.gitkeep-generator.ts",
        ] {
            assert_eq!(on_write(false_positive), None, "{false_positive} 被误判了");
        }
    }

    #[test]
    fn windows_反斜杠路径也认() {
        assert_eq!(
            on_write("C:\\work\\.git\\config"),
            Some(SafetyKind::GitInternals)
        );
    }

    #[test]
    fn 大写路径不能绕过整层检查() {
        // 回归：这里曾经全是大小写敏感的精确比较。macOS 的 APFS 和
        // Windows 的 NTFS 默认大小写不敏感，`~/.ZSHRC` 打开的就是
        // `~/.zshrc` 那个文件，而 canonicalize 不会把它纠回小写 ——
        // 于是把路径改成大写就是一条零成本的整层绕过：
        // `Write ~/.ZSHRC` 在「全部放行」下静默取得持久化执行权。
        for (path, want) in [
            ("/home/u/.ZSHRC", SafetyKind::ShellRc),
            ("/home/u/.Bashrc", SafetyKind::ShellRc),
            ("/work/.GIT/hooks/pre-commit", SafetyKind::GitInternals),
            ("/home/u/.SSH/config", SafetyKind::SshConfig),
            ("/home/u/.Riot/hooks.json", SafetyKind::AgentConfig),
            ("/home/u/.CARGO/config.toml", SafetyKind::ToolchainConfig),
        ] {
            assert_eq!(on_write(path), Some(want), "{path} 被大小写绕过了");
        }
    }

    #[test]
    fn 大写的凭证路径读也要拦() {
        // 读这条更狠：默认模式下就直接放行，一句都不问。
        for cred in [
            "/work/.ENV",
            "/home/u/.SSH/id_rsa",
            "/home/u/.AWS/credentials",
        ] {
            assert_eq!(on_read(cred), Some(SafetyKind::Credentials), "{cred}");
        }
    }

    #[test]
    fn 私钥和密钥库按扩展名拦() {
        // 私钥叫什么全凭习惯，穷举文件名是穷举不完的
        for key in [
            "/work/certs/server.key",
            "/work/certs/privkey.pem",
            "/home/u/prod-2024.p12",
            "/work/android/release.keystore",
            "/work/gcp/service-account-prod.json",
            "/home/u/.pgpass",
            "/home/u/.ssh/id_ecdsa",
        ] {
            assert_eq!(on_read(key), Some(SafetyKind::Credentials), "{key}");
        }
    }

    #[test]
    fn 登录时自动执行的位置和_zshrc_同档() {
        // 写 LaunchAgent 换来的持久执行权和改 .zshrc 等价 —— 用户下次
        // 登录就中招，而他开「全部放行」时想的是"帮我写代码"。
        for auto in [
            "/Users/u/Library/LaunchAgents/com.evil.plist",
            "/home/u/.config/systemd/user/evil.service",
            "/home/u/.config/systemd/user/evil.timer",
            "/etc/cron.d/evil",
            "/home/u/.local/bin/git",
            "/home/u/.gitconfig",
            "/work/.vscode/tasks.json",
        ] {
            assert_eq!(on_write(auto), Some(SafetyKind::ShellRc), "{auto}");
        }
    }

    #[test]
    fn 自启动清单不误伤普通文件() {
        // 这一档新加的几条最容易误伤 —— 误报多了用户就不看弹窗内容了
        for ok in [
            "/work/src/service.rs",
            "/work/docs/cron.md",
            "/work/src/bin/main.rs",
            "/work/.gitattributes",
            "/work/src/tasks.json",
            "/work/certs/README.md",
        ] {
            assert_eq!(on_write(ok), None, "{ok} 被误判了");
        }
    }

    #[test]
    fn 本应用的配置目录() {
        assert_eq!(
            on_write("/home/u/.riot/settings.json"),
            Some(SafetyKind::AgentConfig),
            "改这个可能影响后续的权限判断"
        );
    }

    #[test]
    fn 没有目标路径的工具不检查() {
        let tool = PermTool::writer("Bash");
        assert_eq!(
            check(
                &tool,
                &serde_json::json!({ "command": "ls" }),
                &ctx_with(PermissionMode::Default)
            ),
            None,
            "命令类的检查在 Bash 分析层做，不在这里"
        );
    }

    #[test]
    fn 建议只给会话级() {
        let f = check(
            &PermTool::writer("Write"),
            &serde_json::json!({ "path": "/work/.git/config" }),
            &ctx_with(PermissionMode::Default),
        )
        .expect("有发现");

        match &f.suggestions[0] {
            PermissionUpdate::AddRule { scope, .. } => {
                assert_eq!(
                    *scope,
                    UpdateScope::Session,
                    "敏感操作的永久同意不该在一个弹窗里做完"
                );
            }
            other => panic!("{other:?}"),
        }
    }
}
