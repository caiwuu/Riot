//! Bash 工具。
//!
//! 权限判定不在这里 —— 命令分析、子命令拆分、规则匹配全部委托给
//! `riot_permissions::bash`。这个文件只管三件事:把命令交给子进程、
//! 不让它挂住、把输出裁剪成模型读得下的大小。

use async_trait::async_trait;
use riot_permissions::RuleSet;
use riot_permissions::bash;
use riot_protocol::permission::{DecisionReason, PermissionContext, PermissionResult, SafetyKind};
use riot_protocol::tool::{
    InterruptBehavior, ProcessSpec, PromptContext, Tool, ToolContext, ToolOutcome, UiPayload,
    ValidationError,
};
use serde::Deserialize;

/// 默认超时。
///
/// 够跑一次中等规模的测试套件。比这更长的命令应该用后台执行,
/// 而不是让模型干等 —— 用户看着一个转圈的 spinner 十分钟是最糟的体验。
const DEFAULT_TIMEOUT_MS: u64 = 120_000;

/// 超时上限。模型可以调大,但不能无限。
const MAX_TIMEOUT_MS: u64 = 600_000;

/// stdout / stderr 各自的字符上限。
const MAX_STREAM_CHARS: usize = 30_000;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    /// 要执行的 shell 命令。
    command: String,
    /// 这条命令在做什么，5-10 个字，用于向用户展示。
    ///
    /// 只在 schema 和 `describe()` 里用到 —— 但字段必须留着，
    /// `deny_unknown_fields` 会把没声明的参数当成错误拒掉。
    #[serde(default)]
    #[allow(dead_code)]
    description: Option<String>,
    /// 超时毫秒数。默认 120000，最大 600000。
    #[serde(default)]
    timeout_ms: Option<u64>,
    /// 长期服务（dev server、watch 编译）设 true：命令跑在用户的终端
    /// 面板里，立刻返回终端 id，不等它结束。
    #[serde(default)]
    background: bool,
}

pub struct Bash;

#[async_trait]
impl Tool for Bash {
    fn name(&self) -> &'static str {
        "Bash"
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(Input)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        format!(
            "在工作目录下执行一条 shell 命令。\n\
             \n\
             - 每次调用都是**独立的一次执行**。`cd` 只在这一条命令内有效，\
             不会影响下一次调用。需要切目录就写成 `cd sub && cmd`，或者直接用相对\
             工作目录的路径。\n\
             - 环境是非交互的：没有 stdin，编辑器和分页器都被禁用。需要输入的命令\
             会失败而不是挂住，请改用非交互参数（例如 `git commit -m`）。\
             `git push` / `ssh` 要凭证时走宿主的 GIT_ASKPASS，不会去开 /dev/tty。\n\
             - 默认 {}s 超时，最长 {}s。\n\
             - **长期服务**（dev server、watch、任何不会自己结束的东西）必须\
             设 `background: true`：它会跑在用户的终端面板里，立刻返回终端 id。\
             不设的话命令要么卡到超时、要么在收尾时连同整个进程组被杀掉。\
             起完用 TerminalOutput 看日志、TerminalKill 停。\n\
             - 输出过长时会保留开头和结尾，中间省略。\n\
             - 查找文件用 Glob、搜索内容用 Grep，它们比 `find` 和 `grep` 更快，\
             也不会被输出上限截断。\n\
             - 读文件用 Read，不要用 `cat`。",
            DEFAULT_TIMEOUT_MS / 1000,
            MAX_TIMEOUT_MS / 1000,
        )
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        if let Some(d) = input.get("description").and_then(|v| v.as_str())
            && !d.trim().is_empty()
        {
            return d.to_owned();
        }
        match input.get("command").and_then(|v| v.as_str()) {
            Some(c) => clamp_chars(c.trim(), 60),
            None => "执行命令".to_owned(),
        }
    }

    /// 按命令内容判定,不是按工具判定。
    ///
    /// `ls -la` 和 `rm -rf /` 是同一个工具,但只有前者能和别的工具并行。
    /// 判定逻辑复用权限层的只读白名单 —— 两处用不同标准的话,会出现
    /// "权限层认为要确认、调度器认为可以并发"这种自相矛盾的状态。
    fn is_read_only(&self, input: &serde_json::Value) -> bool {
        let Some(cmd) = input.get("command").and_then(|v| v.as_str()) else {
            return false;
        };
        match bash::analyze(cmd) {
            bash::Analysis::Simple(subs) => bash::is_read_only(&subs),
            // 结构都没看懂,不敢说它只读
            bash::Analysis::TooComplex(_) => false,
        }
    }

    fn is_concurrency_safe(&self, input: &serde_json::Value) -> bool {
        self.is_read_only(input)
    }

    fn is_destructive(&self, input: &serde_json::Value) -> bool {
        !self.is_read_only(input)
    }

    /// `[约束]` 必须是 `true`。见 ARCHITECTURE.md §7.4
    ///
    /// 命令之间常有隐式依赖 —— `mkdir foo` 失败之后并行跑着的
    /// `cd foo && ...` 已经没有意义,继续跑只会产生一堆误导性的错误。
    fn cascades_on_failure(&self) -> bool {
        true
    }

    fn interrupt_behavior(&self) -> InterruptBehavior {
        InterruptBehavior::Cancel
    }

    fn classifier_input(&self, input: &serde_json::Value) -> Option<String> {
        input
            .get("command")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
    }

    fn check_permissions(
        &self,
        input: &serde_json::Value,
        ctx: &PermissionContext,
    ) -> PermissionResult {
        let Some(cmd) = input.get("command").and_then(|v| v.as_str()) else {
            // schema 校验会先拦住这种输入。走到这里说明有人绕过了管线,
            // 那就按最保守的处理。
            return PermissionResult::Deny {
                message: "Bash 缺少 `command` 参数".to_owned(),
                reason: DecisionReason::SafetyCheck {
                    safety: SafetyKind::UnparseableCommand,
                },
            };
        };
        bash::decide(cmd, ctx, &RuleSet::new(ctx.rules.clone()))
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        let parsed: Input = serde_json::from_value(input.clone())
            .map_err(|e| ValidationError::rejected(schema_hint(&e)))?;

        if parsed.command.trim().is_empty() {
            return Err(ValidationError::rejected(
                "`command` 不能为空。请提供要执行的命令。",
            ));
        }
        if let Some(t) = parsed.timeout_ms
            && t > MAX_TIMEOUT_MS
        {
            return Err(ValidationError::rejected(format!(
                "`timeout_ms` 最大 {MAX_TIMEOUT_MS}（{} 分钟）。更长的任务请拆成几步，\
                 或者先跑一个能快速失败的子集。",
                MAX_TIMEOUT_MS / 60_000
            )));
        }
        Ok(())
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let parsed: Input = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolOutcome::failed(schema_hint(&e)),
        };

        if parsed.background {
            return spawn_service(&parsed, &ctx).await;
        }

        let timeout_ms = parsed
            .timeout_ms
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS);

        let spec = ProcessSpec {
            program: shell_program(),
            // -c 之后不加 -l / -i。登录 shell 会读用户的 rc 文件,
            // 那里可能有 alias 和交互式设置,让同一条命令在不同机器上
            // 表现不同 —— 而模型看不到那些配置。
            args: vec!["-c".to_owned(), parsed.command.clone()],
            cwd: ctx.cwd.clone(),
            env: non_interactive_env(),
            timeout_ms: Some(timeout_ms),
        };

        let out = match ctx.proc.run(spec, ctx.cancel.clone()).await {
            Ok(o) => o,
            Err(e) => return ToolOutcome::failed(spawn_hint(&e)),
        };

        let stdout = clamp_stream(&out.stdout);
        let stderr = clamp_stream(&out.stderr);
        let body = render(&stdout, &stderr, out.exit_code, out.timed_out, timeout_ms);

        let ui = Some(UiPayload::BashOutput {
            stdout: stdout.text.clone(),
            stderr: stderr.text.clone(),
            exit_code: out.exit_code,
            duration_ms: out.duration_ms,
        });

        // 非零退出算失败,但输出照给 —— 模型要靠输出才能诊断。
        //
        // 措辞保持中性:`grep` 没匹配到返回 1、`diff` 有差异返回 1,
        // 这些都是正常结果。说成"命令执行失败"会诱导模型去"修"一个
        // 根本没坏的东西。
        if out.timed_out || out.exit_code != 0 {
            return ToolOutcome::Failed {
                error_for_model: body,
                ui_payload: ui,
            };
        }

        ToolOutcome::Ok {
            model_content: riot_protocol::message::ToolResultContent::text(body),
            ui_payload: ui,
            side_messages: Vec::new(),
        }
    }
}

/// Bash 的可执行文件。
#[cfg(not(windows))]
fn shell_program() -> String {
    "bash".to_owned()
}

/// Bash 的可执行文件。
///
/// # 为什么不能把 `"bash"` 直接交给 PATH
///
/// 只要启用过 WSL 功能，`C:\Windows\System32\bash.exe` 就在 PATH 里而且排
/// 得很前 —— 那是 WSL 的启动器，不是 bash。它会跳到 Linux 那侧的文件系统
/// 里执行，工作目录 `D:\…` 在那边并不存在，于是报 `execvpe(/bin/bash)
/// failed: No such file or directory`：看着像"没装 bash"，其实是"找错了
/// bash"。
///
/// 所以显式去找 Git for Windows 自带的那个。装了 git 就有，而这个应用本来
/// 就依赖 git。
#[cfg(windows)]
fn shell_program() -> String {
    static RESOLVED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    RESOLVED.get_or_init(resolve_windows_bash).clone()
}

#[cfg(windows)]
fn resolve_windows_bash() -> String {
    use std::path::{Path, PathBuf};

    // 逃生舱：装在非常规位置，或者用户就是想指定某一个。
    if let Some(p) = std::env::var_os("RIOT_BASH") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return p.to_string_lossy().into_owned();
        }
    }

    // Git for Windows：git.exe 在 `<root>\cmd\`，bash 在 `<root>\bin\`。
    // 顺着 PATH 里的 git 反推比猜安装目录准 —— 用户可能装在别的盘。
    for git in path_lookup("git.exe") {
        if let Some(found) = git.parent().and_then(Path::parent).and_then(git_bash_under) {
            return found;
        }
    }

    // 常见安装位置。git 不在 PATH 上时（只装了 GUI 客户端）还能捞回来。
    for var in [
        "ProgramFiles",
        "ProgramW6432",
        "ProgramFiles(x86)",
        "LOCALAPPDATA",
    ] {
        let Some(base) = std::env::var_os(var) else {
            continue;
        };
        for sub in ["Git", r"Programs\Git"] {
            if let Some(found) = git_bash_under(&Path::new(&base).join(sub)) {
                return found;
            }
        }
    }

    // PATH 里别的 bash（MSYS2、Cygwin）。跳过 WSL 启动器。
    for cand in path_lookup("bash.exe") {
        if !is_wsl_launcher(&cand) {
            return cand.to_string_lossy().into_owned();
        }
    }

    // 都没找到。保持裸名字 —— spawn 失败时的系统报错比这里编一个更准确。
    "bash".to_owned()
}

/// `<root>\bin\bash.exe` 排在 `usr\bin` 前面：前者是 Git for Windows 的
/// 包装器，会把 MSYS 环境准备好。
#[cfg(windows)]
fn git_bash_under(root: &std::path::Path) -> Option<String> {
    [r"bin\bash.exe", r"usr\bin\bash.exe"]
        .iter()
        .map(|rel| root.join(rel))
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
}

#[cfg(windows)]
fn path_lookup(exe: &str) -> Vec<std::path::PathBuf> {
    std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join(exe))
                .filter(|p| p.is_file())
                .collect()
        })
        .unwrap_or_default()
}

/// `System32\bash.exe`（32 位视图下是 `SysWOW64`）是 WSL 启动器。
#[cfg(windows)]
fn is_wsl_launcher(p: &std::path::Path) -> bool {
    let s = p.to_string_lossy().to_ascii_lowercase();
    s.contains(r"\windows\system32\") || s.contains(r"\windows\syswow64\")
}

/// 让子进程跑在非交互模式下。
///
/// 交互式命令是 agent 执行 shell 时最常见的挂死原因:`git commit` без `-m`
/// 会开编辑器,`git log` 会开分页器,两者都等一个永远不会来的按键。超时能
/// 兜底,但那意味着用户白等两分钟才拿到一个没有信息量的失败。
fn non_interactive_env() -> Vec<(String, String)> {
    [
        // 编辑器:`true` 是一个立即成功退出的程序,于是 git 认为
        // "编辑器已保存退出",按已有内容继续。
        ("GIT_EDITOR", "true"),
        ("EDITOR", "true"),
        ("VISUAL", "true"),
        // 分页器
        ("GIT_PAGER", "cat"),
        ("PAGER", "cat"),
        // ANSI 转义序列对模型是纯噪音,还占 token
        ("NO_COLOR", "1"),
        // 没有控制终端。不关的话 git 会去开 /dev/tty，报
        // "Device not configured"。关了之后走 GIT_ASKPASS（宿主已装），
        // 助手不在就立刻失败，而不是挂死。
        ("GIT_TERMINAL_PROMPT", "0"),
        // OpenSSH 默认"有 tty 才提问"。这里没有 tty，force 让它走 SSH_ASKPASS。
        ("SSH_ASKPASS_REQUIRE", "force"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

struct Clamped {
    text: String,
    /// 原始行数。截断时要告诉模型省了多少。
    total_lines: usize,
    truncated: bool,
}

/// 截断保留**头和尾**。
///
/// `[约束]` 不能只保留开头。命令输出里最有价值的部分通常在末尾:编译器的
/// "error: aborting due to 3 previous errors"、测试框架的失败汇总、脚本的
/// 最后一条日志。只保头部的话,模型看到的是一堆无关的编译进度,而真正的
/// 错误原因被丢掉了。
fn clamp_stream(s: &str) -> Clamped {
    if s.chars().count() <= MAX_STREAM_CHARS {
        return Clamped {
            text: s.to_owned(),
            total_lines: s.lines().count(),
            truncated: false,
        };
    }

    let lines: Vec<&str> = s.lines().collect();
    let total_lines = lines.len();

    // 头 60% 尾 40%。头部多一点是因为它常包含命令的上下文(在做什么),
    // 但尾部的密度更高,所以不能太少。
    let head_budget = MAX_STREAM_CHARS * 6 / 10;
    let tail_budget = MAX_STREAM_CHARS - head_budget;

    let mut head = Vec::new();
    let mut used = 0usize;
    for l in &lines {
        let n = l.chars().count() + 1;
        if used + n > head_budget {
            break;
        }
        used += n;
        head.push(*l);
    }

    let mut tail = std::collections::VecDeque::new();
    let mut used = 0usize;
    for l in lines.iter().rev() {
        let n = l.chars().count() + 1;
        if used + n > tail_budget {
            break;
        }
        used += n;
        tail.push_front(*l);
    }

    let omitted = total_lines.saturating_sub(head.len() + tail.len());
    if omitted == 0 {
        return Clamped {
            text: s.to_owned(),
            total_lines,
            truncated: false,
        };
    }

    let text = format!(
        "{}\n\n… 中间省略 {omitted} 行 …\n\n{}",
        head.join("\n"),
        tail.iter().copied().collect::<Vec<_>>().join("\n")
    );

    Clamped {
        text,
        total_lines,
        truncated: true,
    }
}

fn render(
    stdout: &Clamped,
    stderr: &Clamped,
    exit_code: i32,
    timed_out: bool,
    timeout_ms: u64,
) -> String {
    let mut parts = Vec::new();

    if timed_out {
        // 超时前的输出照样给。它往往正好指出卡在哪一步 ——
        // 只说"超时了"等于让模型从零开始猜。
        // 不给下一步的话，模型最常见的反应是原样重跑一遍，然后再超时一次。
        // 三条出路按适用场景排：长期服务、慢但会结束、以及缩小范围。
        parts.push(format!(
            "命令在 {}s 后超时，已被终止。\n\
             下一步选一条，别原样重跑：\n\
             - 这是长期服务（dev server、watch）→ 用 `background: true` 重跑，\
               然后用 TerminalOutput 的 `wait_for` 等它就绪；\n\
             - 确实需要跑更久 → 调大 `timeout_ms`（上限 10 分钟）；\n\
             - 只是范围太大 → 缩小它（跑单个测试、单个包）。\n\
             以下是超时前的输出，它通常直接指出卡在哪一步：",
            timeout_ms / 1000
        ));
    }

    if !stdout.text.is_empty() {
        parts.push(stdout.text.clone());
    }
    if !stderr.text.is_empty() {
        // 标出来源。混在一起的话,模型无法判断一段文字是正常输出还是错误 ——
        // 很多工具把进度信息写到 stderr。
        parts.push(format!("stderr:\n{}", stderr.text));
    }

    if stdout.text.is_empty() && stderr.text.is_empty() {
        // `[约束]` 空输出必须显式说明。返回空字符串的话模型会
        // 以为工具坏了,然后原样重试一遍。
        parts.push(if timed_out {
            "（超时前没有任何输出）".to_owned()
        } else if exit_code == 0 {
            "命令执行成功，没有输出。".to_owned()
        } else {
            format!("命令退出码 {exit_code}，没有输出。")
        });
    } else if !timed_out && exit_code != 0 {
        parts.push(format!("命令退出码 {exit_code}。"));
    }

    let mut notes = Vec::new();
    if stdout.truncated {
        notes.push(format!(
            "stdout 共 {} 行，已省略中间部分",
            stdout.total_lines
        ));
    }
    if stderr.truncated {
        notes.push(format!(
            "stderr 共 {} 行，已省略中间部分",
            stderr.total_lines
        ));
    }
    if !notes.is_empty() {
        parts.push(format!(
            "<system-reminder>{}。需要完整输出请把结果重定向到文件后用 Read 分段查看。\
             </system-reminder>",
            notes.join("；")
        ));
    }

    parts.join("\n")
}

fn clamp_chars(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_owned();
    }
    format!("{}…", s.chars().take(n).collect::<String>())
}

fn spawn_hint(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => {
            "找不到 bash。这台机器上没有可用的 shell，无法执行命令。".to_owned()
        }
        std::io::ErrorKind::PermissionDenied => "没有执行命令的权限。".to_owned(),
        _ => format!("启动命令失败：{e}"),
    }
}

/// 把长期服务交给终端面板。
///
/// 不能走 `ctx.proc`：那条路收尾时会清掉整个进程组（见 riot-runtime 的
/// proc），dev server 活不过这一次调用。而模型为了让它活下来只能 `setsid`
/// 逃出去 —— 那就成了谁都管不着的幽灵进程。放进终端面板，用户看得见、
/// 能 Ctrl-C，模型能读能停。
async fn spawn_service(parsed: &Input, ctx: &ToolContext) -> ToolOutcome {
    let title = parsed
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map_or_else(|| clamp_chars(parsed.command.trim(), 30), str::to_owned);

    match ctx.terminal.spawn(&parsed.command, &title).await {
        Ok(id) => ToolOutcome::Ok {
            model_content: riot_protocol::message::ToolResultContent::text(format!(
                "已在终端 {id} 起了：{}\n\
                 它在用户的终端面板里跑着（用户能看到、也能自己停）。\
                 用 TerminalOutput(id={id}) 读输出，TerminalKill(id={id}) 停掉。\n\
                 服务通常要几秒才就绪，别立刻去读。",
                parsed.command.trim()
            )),
            ui_payload: None,
            side_messages: Vec::new(),
        },
        Err(e) => ToolOutcome::failed(e.0),
    }
}

fn schema_hint(e: &serde_json::Error) -> String {
    let raw = e.to_string();
    if raw.contains("missing field `command`") {
        return "缺少必需参数 `command`。请提供要执行的 shell 命令。".to_owned();
    }
    if raw.contains("unknown field") {
        return format!("Bash 只接受 `command`、`description`、`timeout_ms` 三个参数。（{raw}）");
    }
    format!("参数格式不对：{raw}。请检查参数类型。")
}
