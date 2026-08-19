//! Hooks：在 agent 生命周期的固定检查点跑用户配置的脚本。
//!
//! # 配置
//!
//! ```text
//! <配置目录>/riot/hooks.json      全局，所有项目生效
//! <项目根>/.riot/hooks.json       项目级，两层拼接（都跑，不覆盖）
//! ```
//!
//! 结构与 Claude Code 的 settings.hooks 段同构，方便直接拷贝：
//!
//! ```json
//! {
//!   "PreToolUse": [
//!     { "matcher": "Bash|Write", "hooks": [ { "type": "command", "command": "./check.sh", "timeout": 30 } ] }
//!   ],
//!   "PostToolUse": [...], "Stop": [...], "UserPromptSubmit": [...]
//! }
//! ```
//!
//! 顶层可以直接是事件表，也可以整个包在 `{"hooks": {...}}` 里（CC 用户
//! 把 settings.json 的段落原样拷过来就能用）。
//!
//! # 执行协议（对齐 CC）
//!
//! - spawn `sh -c <command>`（Windows 是 `cmd /C`），工作目录 = 项目根；
//! - stdin 收一行 JSON：公共字段 `session_id` `cwd` `hook_event_name` +
//!   事件字段（PreToolUse: `tool_name/tool_input/tool_use_id`，PostToolUse:
//!   另加 `tool_response/is_error`，Stop: `stop_hook_active`，
//!   UserPromptSubmit: `prompt`）；
//! - exit 0 = 通过（stdout 以 `{` 开头则按 JSON 解析高级输出）；
//!   exit 2 = **阻断**（stderr 作为理由）；其它 = 非阻塞错误，只记日志；
//! - stdout JSON 认这些字段：`decision`（approve/block）、`reason`、
//!   `hookSpecificOutput.permissionDecision`（allow/deny/ask）、
//!   `hookSpecificOutput.permissionDecisionReason`、
//!   `hookSpecificOutput.additionalContext`；
//! - 同事件所有匹配的 hook **并行**跑；单个默认超时 60 秒（`timeout`
//!   字段按秒覆盖，夹在 1..=600）。超时/spawn 失败/坏 JSON 都算非阻塞
//!   错误 —— hook 坏了不该拦工具链路。
//!
//! 豁免理由：宿主层，读的是用户自己的配置文件、跑的是用户自己写的脚本。

#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

/// 单个 hook 的默认超时。CC 给 10 分钟 —— 桌面应用里一个卡死的检查
/// 脚本挂十分钟等于应用坏了，这里收紧到一分钟。
const DEFAULT_TIMEOUT_SECS: u64 = 60;
const TIMEOUT_RANGE: std::ops::RangeInclusive<u64> = 1..=600;

/// stdout/stderr 各自最多收多少字节。检查脚本的正常输出是几行字，
/// 超过这个数的基本是把构建日志整个倒出来了。
const MAX_CAPTURE_BYTES: usize = 64 * 1024;

// ────────────────────────────────────────────────────────────
// 配置
// ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Default)]
pub struct HooksFile {
    #[serde(default, rename = "PreToolUse")]
    pub pre_tool_use: Vec<MatcherGroup>,
    #[serde(default, rename = "PostToolUse")]
    pub post_tool_use: Vec<MatcherGroup>,
    #[serde(default, rename = "Stop")]
    pub stop: Vec<MatcherGroup>,
    #[serde(default, rename = "UserPromptSubmit")]
    pub user_prompt_submit: Vec<MatcherGroup>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MatcherGroup {
    /// 工具名匹配：空/`*` = 全部；`A|B` = 精确列表；其它 = 正则。
    /// Stop / UserPromptSubmit 没有匹配对象，这个字段被忽略。
    #[serde(default)]
    pub matcher: String,
    #[serde(default)]
    pub hooks: Vec<HookDef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HookDef {
    /// 只支持 "command"。别的类型（prompt/agent/http）解析不报错，
    /// 执行时跳过 —— 拷贝过来的 CC 配置不该在这里炸。
    #[serde(default = "default_type", rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub command: String,
    /// 秒。缺省 60，夹在 1..=600。
    #[serde(default)]
    pub timeout: Option<u64>,
}

fn default_type() -> String {
    "command".into()
}

/// 配置文件解析问题（给设置页/日志）。
pub struct Problem {
    pub path: PathBuf,
    pub reason: String,
}

/// 设置页看的一条 hook。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookInfo {
    /// `PreToolUse` / `PostToolUse` / `Stop` / `UserPromptSubmit`。
    pub event: String,
    /// 空 = 匹配全部工具（或该事件没有匹配对象）。
    pub matcher: String,
    pub command: String,
    pub timeout_secs: u64,
    /// `global` / `project`。
    pub source: String,
    /// 配置文件级的问题（这条不是 hook，是一条错误提示）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 设置页的 hooks 清单：两层配置里配了什么，加上解析失败的文件。
pub fn list(project_root: Option<&Path>) -> Vec<HookInfo> {
    let mut problems = Vec::new();
    let mut out = Vec::new();
    let mut take = |f: HooksFile, source: &str| {
        for (event, groups) in [
            ("PreToolUse", f.pre_tool_use),
            ("PostToolUse", f.post_tool_use),
            ("Stop", f.stop),
            ("UserPromptSubmit", f.user_prompt_submit),
        ] {
            for g in groups {
                for h in g.hooks {
                    out.push(HookInfo {
                        event: event.to_owned(),
                        matcher: g.matcher.clone(),
                        command: h.command,
                        timeout_secs: h
                            .timeout
                            .unwrap_or(DEFAULT_TIMEOUT_SECS)
                            .clamp(*TIMEOUT_RANGE.start(), *TIMEOUT_RANGE.end()),
                        source: source.to_owned(),
                        error: None,
                    });
                }
            }
        }
    };
    take(load_file(&global_path(), &mut problems), "global");
    if let Some(root) = project_root {
        take(load_file(&project_path(root), &mut problems), "project");
    }
    out.extend(problems.into_iter().map(|p| HookInfo {
        event: String::new(),
        matcher: String::new(),
        command: p.path.display().to_string(),
        timeout_secs: 0,
        source: "global".into(),
        error: Some(p.reason),
    }));
    out
}

/// 全局 hooks.json 的路径。
pub fn global_path() -> PathBuf {
    crate::config::config_path()
        .parent()
        .unwrap_or(Path::new("."))
        .join("hooks.json")
}

/// 项目级 hooks.json 的路径。
pub fn project_path(root: &Path) -> PathBuf {
    root.join(".riot").join("hooks.json")
}

fn load_file(path: &Path, problems: &mut Vec<Problem>) -> HooksFile {
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return HooksFile::default(),
        Err(e) => {
            problems.push(Problem { path: path.to_path_buf(), reason: format!("读不出来：{e}") });
            return HooksFile::default();
        }
    };
    // 顶层允许 {"hooks": {...}}（CC settings.json 原样拷贝）或直接事件表。
    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            problems.push(Problem { path: path.to_path_buf(), reason: format!("JSON 解析失败：{e}") });
            return HooksFile::default();
        }
    };
    let table = match value.get("hooks") {
        Some(inner) => inner.clone(),
        None => value,
    };
    match serde_json::from_value(table) {
        Ok(f) => f,
        Err(e) => {
            problems.push(Problem { path: path.to_path_buf(), reason: format!("结构不对：{e}") });
            HooksFile::default()
        }
    }
}

fn merge(mut a: HooksFile, b: HooksFile) -> HooksFile {
    a.pre_tool_use.extend(b.pre_tool_use);
    a.post_tool_use.extend(b.post_tool_use);
    a.stop.extend(b.stop);
    a.user_prompt_submit.extend(b.user_prompt_submit);
    a
}

// ────────────────────────────────────────────────────────────
// 引擎
// ────────────────────────────────────────────────────────────

/// 一次 hook 执行的结论。「通过没意见」不占变体 —— 没意见就是不出现。
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// 阻断（exit 2 或 decision: block / permissionDecision: deny）。
    Block { reason: String },
    /// PreToolUse 专用：明确放行（permissionDecision: allow / decision: approve）。
    Allow,
    /// PreToolUse 专用：强制询问用户。
    Ask { reason: String },
    /// PreToolUse 专用：改写工具输入（`updatedInput`）。
    ///
    /// 改写在**权限判定之前**生效 —— 判定必须看到最终会执行的那份输入，
    /// 否则就成了"按 A 授权、执行 B"。
    Rewrite { input: serde_json::Value },
    /// 附加上下文，给模型看的段落。
    Context { text: String },
}

/// 会话级 hooks 引擎。每轮现装（配置中途改了下一轮生效，和 caps 一条规矩）。
pub struct HookEngine {
    cfg: HooksFile,
    cwd: PathBuf,
    session_id: String,
}

impl HookEngine {
    /// 读全局 + 项目两层配置。问题记日志，不拦启动。
    pub fn load(cwd: &Path, session_id: &str) -> Self {
        let mut problems = Vec::new();
        let global = load_file(&global_path(), &mut problems);
        let project = load_file(&project_path(cwd), &mut problems);
        for p in &problems {
            tracing::warn!(path = %p.path.display(), reason = %p.reason, "hooks 配置有问题，跳过该文件");
        }
        Self {
            cfg: merge(global, project),
            cwd: cwd.to_path_buf(),
            session_id: session_id.to_owned(),
        }
    }

    /// 什么都没配的引擎。各检查点都会短路（`has_*` 全 false）。
    pub fn empty() -> Self {
        Self {
            cfg: HooksFile::default(),
            cwd: PathBuf::from("."),
            session_id: String::new(),
        }
    }

    /// 直接用给定配置建引擎，**不碰磁盘**。
    ///
    /// 测试专用：`load` 会读用户真实的 `~/.../riot/hooks.json`，
    /// 在测试里那既不可复现，还可能真的执行开发机上配的脚本。
    #[cfg(test)]
    pub(crate) fn from_config_json(cfg: serde_json::Value, cwd: &Path) -> Self {
        Self {
            cfg: serde_json::from_value(cfg).expect("测试 hooks 配置"),
            cwd: cwd.to_path_buf(),
            session_id: "test".into(),
        }
    }

    #[cfg(test)]
    fn from_config(cfg: HooksFile, cwd: &Path) -> Self {
        Self { cfg, cwd: cwd.to_path_buf(), session_id: "test".into() }
    }

    pub fn has_pre_tool_use(&self) -> bool {
        self.cfg.pre_tool_use.iter().any(|g| !g.hooks.is_empty())
    }

    pub fn has_post_tool_use(&self) -> bool {
        self.cfg.post_tool_use.iter().any(|g| !g.hooks.is_empty())
    }

    pub fn has_stop(&self) -> bool {
        self.cfg.stop.iter().any(|g| !g.hooks.is_empty())
    }

    pub fn has_user_prompt_submit(&self) -> bool {
        self.cfg.user_prompt_submit.iter().any(|g| !g.hooks.is_empty())
    }

    /// PreToolUse：工具名匹配的全部 hook 并行跑。
    pub async fn pre_tool_use(
        &self,
        tool: &str,
        input: &serde_json::Value,
        tool_use_id: &str,
    ) -> Vec<Outcome> {
        let payload = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "session_id": self.session_id,
            "cwd": self.cwd.display().to_string(),
            "tool_name": tool,
            "tool_input": input,
            "tool_use_id": tool_use_id,
        });
        self.run_matching(&self.cfg.pre_tool_use, Some(tool), &payload).await
    }

    /// PostToolUse：反馈段落已按"给模型看"的口吻整理。
    pub async fn post_tool_use(
        &self,
        tool: &str,
        input: &serde_json::Value,
        output_preview: &str,
        is_error: bool,
    ) -> Vec<Outcome> {
        let payload = serde_json::json!({
            "hook_event_name": "PostToolUse",
            "session_id": self.session_id,
            "cwd": self.cwd.display().to_string(),
            "tool_name": tool,
            "tool_input": input,
            "tool_response": output_preview,
            "is_error": is_error,
        });
        self.run_matching(&self.cfg.post_tool_use, Some(tool), &payload).await
    }

    /// Stop：`stop_hook_active` 告诉脚本"这已经不是第一次拦了"，
    /// 让它有机会避免无限 block（内核另有硬熔断兜底）。
    pub async fn stop(&self, stop_hook_active: bool) -> Vec<Outcome> {
        let payload = serde_json::json!({
            "hook_event_name": "Stop",
            "session_id": self.session_id,
            "cwd": self.cwd.display().to_string(),
            "stop_hook_active": stop_hook_active,
        });
        self.run_matching(&self.cfg.stop, None, &payload).await
    }

    /// UserPromptSubmit：block = 这条消息不发；context = 附加给模型的段落。
    pub async fn user_prompt_submit(&self, prompt: &str) -> Vec<Outcome> {
        let payload = serde_json::json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": self.session_id,
            "cwd": self.cwd.display().to_string(),
            "prompt": prompt,
        });
        self.run_matching(&self.cfg.user_prompt_submit, None, &payload).await
    }

    async fn run_matching(
        &self,
        groups: &[MatcherGroup],
        subject: Option<&str>,
        payload: &serde_json::Value,
    ) -> Vec<Outcome> {
        let mut jobs = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for g in groups {
            if let Some(name) = subject
                && !matches_tool(&g.matcher, name)
            {
                continue;
            }
            for h in &g.hooks {
                if h.kind != "command" {
                    tracing::debug!(kind = %h.kind, "不支持的 hook 类型，跳过");
                    continue;
                }
                if h.command.trim().is_empty() {
                    continue;
                }
                // 同命令去重：全局和项目里配了同一条只跑一次。
                if !seen.insert(h.command.clone()) {
                    continue;
                }
                jobs.push(self.run_one(h.clone(), payload.clone()));
            }
        }
        futures::future::join_all(jobs)
            .await
            .into_iter()
            .flatten()
            .collect()
    }

    /// 跑单个 hook。所有失败模式（spawn 失败、超时、坏 JSON、非 0 非 2）
    /// 都收敛成"没意见 + 日志" —— hook 坏了不该拦链路。
    async fn run_one(&self, def: HookDef, payload: serde_json::Value) -> Vec<Outcome> {
        use tokio::io::AsyncWriteExt;

        let timeout = Duration::from_secs(
            def.timeout
                .unwrap_or(DEFAULT_TIMEOUT_SECS)
                .clamp(*TIMEOUT_RANGE.start(), *TIMEOUT_RANGE.end()),
        );

        let mut cmd = if cfg!(windows) {
            let mut c = tokio::process::Command::new("cmd");
            c.arg("/C").arg(&def.command);
            c
        } else {
            let mut c = tokio::process::Command::new("sh");
            c.arg("-c").arg(&def.command);
            c
        };
        cmd.current_dir(&self.cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        // Windows:不带 CREATE_NO_WINDOW 的话，打包后的 GUI 主程序每跑一个
        // hook 就闪一个黑色控制台窗。理由的完整版见 riot-runtime 的命令执行器。
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(command = %def.command, error = %e, "hook 起不来，跳过");
                return Vec::new();
            }
        };

        // stdin 一行 JSON。写失败（脚本没读就退了）不算错。
        if let Some(mut stdin) = child.stdin.take() {
            let line = format!("{payload}\n");
            let _ = stdin.write_all(line.as_bytes()).await;
            drop(stdin);
        }

        let out = match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => {
                tracing::warn!(command = %def.command, error = %e, "hook 执行失败，跳过");
                return Vec::new();
            }
            Err(_) => {
                tracing::warn!(command = %def.command, timeout_secs = timeout.as_secs(), "hook 超时，跳过（不算阻断）");
                return Vec::new();
            }
        };

        let stdout = truncated(&out.stdout);
        let stderr = truncated(&out.stderr);
        let code = out.status.code();

        match code {
            Some(0) => parse_success_output(&def.command, &stdout),
            Some(2) => {
                // exit 2 = 阻断，stderr 是给模型的理由（CC 协议）。
                let reason = if stderr.trim().is_empty() {
                    format!("被 hook 拦下（{}）", short(&def.command))
                } else {
                    stderr.trim().to_owned()
                };
                vec![Outcome::Block { reason }]
            }
            other => {
                tracing::warn!(
                    command = %def.command,
                    code = ?other,
                    stderr = %stderr.trim(),
                    "hook 非 0 非 2 退出，当非阻塞错误跳过"
                );
                Vec::new()
            }
        }
    }
}

/// exit 0 的 stdout：以 `{` 开头按 JSON 高级输出解析；否则纯文本当
/// 附加上下文（空的忽略）。
fn parse_success_output(command: &str, stdout: &str) -> Vec<Outcome> {
    let t = stdout.trim();
    if t.is_empty() {
        return Vec::new();
    }
    if !t.starts_with('{') {
        return vec![Outcome::Context { text: t.to_owned() }];
    }
    let v: serde_json::Value = match serde_json::from_str(t) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(command = %command, error = %e, "hook 的 JSON 输出解析失败，忽略");
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    let reason = |v: &serde_json::Value, key: &str| -> String {
        v.get(key)
            .and_then(|r| r.as_str())
            .unwrap_or("hook 未给出理由")
            .to_owned()
    };

    // hookSpecificOutput.permissionDecision 优先于顶层 decision（CC 的新旧两代接口）。
    if let Some(spec) = v.get("hookSpecificOutput") {
        match spec.get("permissionDecision").and_then(|d| d.as_str()) {
            Some("deny") => out.push(Outcome::Block { reason: reason(spec, "permissionDecisionReason") }),
            Some("allow") => out.push(Outcome::Allow),
            Some("ask") => out.push(Outcome::Ask { reason: reason(spec, "permissionDecisionReason") }),
            _ => {}
        }
        if let Some(updated) = spec.get("updatedInput")
            && updated.is_object()
        {
            out.push(Outcome::Rewrite { input: updated.clone() });
        }
        if let Some(ctx) = spec.get("additionalContext").and_then(|c| c.as_str())
            && !ctx.trim().is_empty()
        {
            out.push(Outcome::Context { text: ctx.trim().to_owned() });
        }
    }

    if out.is_empty() {
        match v.get("decision").and_then(|d| d.as_str()) {
            Some("block") => out.push(Outcome::Block { reason: reason(&v, "reason") }),
            Some("approve") => out.push(Outcome::Allow),
            _ => {}
        }
    }
    out
}

/// 工具名匹配：空/`*` = 全部；纯名字或 `A|B` = 精确；其它 = 正则
///（编译失败当不匹配 + 告警，坏正则不该放行也不该拦全部）。
fn matches_tool(matcher: &str, tool: &str) -> bool {
    let m = matcher.trim();
    if m.is_empty() || m == "*" {
        return true;
    }
    if m.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '|') {
        return m.split('|').any(|p| p == tool);
    }
    match regex_lite::Regex::new(m) {
        Ok(re) => re.is_match(tool),
        Err(e) => {
            tracing::warn!(matcher = %m, error = %e, "matcher 正则编译失败，按不匹配处理");
            false
        }
    }
}

fn truncated(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    if s.len() <= MAX_CAPTURE_BYTES {
        return s.into_owned();
    }
    let mut t: String = s.chars().take(MAX_CAPTURE_BYTES).collect();
    t.push_str("\n[输出超长已截断]");
    t
}

fn short(command: &str) -> String {
    let c = command.trim();
    if c.chars().count() <= 60 { c.to_owned() } else { c.chars().take(60).collect::<String>() + "…" }
}

// ────────────────────────────────────────────────────────────
// 三个检查点的适配器
// ────────────────────────────────────────────────────────────

/// PostToolUse 检查点（调度器的 [`riot_protocol::hook::ToolHooks`]）。
pub struct HookToolHooks(pub std::sync::Arc<HookEngine>);

#[async_trait::async_trait]
impl riot_protocol::hook::ToolHooks for HookToolHooks {
    fn enabled(&self) -> bool {
        self.0.has_post_tool_use()
    }

    async fn post_tool_use(
        &self,
        tool: &str,
        input: &serde_json::Value,
        output_preview: &str,
        is_error: bool,
    ) -> Vec<String> {
        self.0
            .post_tool_use(tool, input, output_preview, is_error)
            .await
            .into_iter()
            .filter_map(|o| match o {
                // 阻断和上下文对 PostToolUse 是一回事：都是给模型的反馈
                // 段落（工具已经跑完了，"拦"不了什么，只能让模型补救）。
                Outcome::Block { reason } => Some(format!("PostToolUse hook 检查未通过：{reason}")),
                Outcome::Context { text } => Some(text),
                _ => None,
            })
            .collect()
    }
}

/// Stop 检查点（内核的 [`riot_core::state::StopGate`]）。
pub struct HookStopGate(pub std::sync::Arc<HookEngine>);

#[async_trait::async_trait]
impl riot_core::state::StopGate for HookStopGate {
    async fn check(&self, prior_blocks: u32) -> riot_core::state::StopDecision {
        let outcomes = self.0.stop(prior_blocks > 0).await;
        let reasons: Vec<String> = outcomes
            .into_iter()
            .filter_map(|o| match o {
                Outcome::Block { reason } => Some(reason),
                _ => None,
            })
            .collect();
        if reasons.is_empty() {
            riot_core::state::StopDecision::Allow
        } else {
            riot_core::state::StopDecision::Block { reason: reasons.join("\n") }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(json: serde_json::Value) -> HooksFile {
        serde_json::from_value(json).expect("测试配置")
    }

    #[test]
    fn matcher_规则() {
        assert!(matches_tool("", "Bash"));
        assert!(matches_tool("*", "Read"));
        assert!(matches_tool("Bash", "Bash"));
        assert!(!matches_tool("Bash", "Read"));
        assert!(matches_tool("Bash|Write", "Write"));
        assert!(matches_tool("Web.*", "WebFetch"), "正则要能用");
        assert!(!matches_tool("Web.*", "Bash"));
        assert!(!matches_tool("(", "Bash"), "坏正则按不匹配处理，不放行也不拦全部");
    }

    #[test]
    fn 配置文件兼容_cc_包一层的写法() {
        let dir = tempfile::tempdir().expect("目录");
        let p = dir.path().join("hooks.json");
        std::fs::write(
            &p,
            r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "true"}]}]}}"#,
        )
        .expect("写配置");
        let mut problems = Vec::new();
        let f = load_file(&p, &mut problems);
        assert!(problems.is_empty());
        assert_eq!(f.stop.len(), 1);

        std::fs::write(
            &p,
            r#"{"Stop": [{"hooks": [{"type": "command", "command": "true"}]}]}"#,
        )
        .expect("写配置");
        let f = load_file(&p, &mut problems);
        assert!(problems.is_empty());
        assert_eq!(f.stop.len(), 1, "不包一层也认");
    }

    #[test]
    fn 坏配置只记问题不拦启动() {
        let dir = tempfile::tempdir().expect("目录");
        let p = dir.path().join("hooks.json");
        std::fs::write(&p, "{ not json").expect("写配置");
        let mut problems = Vec::new();
        let f = load_file(&p, &mut problems);
        assert_eq!(problems.len(), 1);
        assert!(f.pre_tool_use.is_empty());
    }

    #[test]
    fn json_高级输出解析() {
        let got = parse_success_output(
            "c",
            r#"{"hookSpecificOutput":{"permissionDecision":"deny","permissionDecisionReason":"不行"}}"#,
        );
        assert_eq!(got, vec![Outcome::Block { reason: "不行".into() }]);

        let got = parse_success_output("c", r#"{"decision":"approve"}"#);
        assert_eq!(got, vec![Outcome::Allow]);

        let got = parse_success_output(
            "c",
            r#"{"hookSpecificOutput":{"permissionDecision":"ask","permissionDecisionReason":"敏感","additionalContext":"注意备份"}}"#,
        );
        assert_eq!(
            got,
            vec![
                Outcome::Ask { reason: "敏感".into() },
                Outcome::Context { text: "注意备份".into() }
            ]
        );

        assert_eq!(
            parse_success_output("c", "记得跑测试"),
            vec![Outcome::Context { text: "记得跑测试".into() }],
            "纯文本 stdout 当附加上下文"
        );
        assert!(parse_success_output("c", "").is_empty());
        assert!(parse_success_output("c", "{ 坏 json").is_empty(), "坏 JSON 忽略不阻断");
    }

    // 引擎测试跑真实 shell，只在 unix 上执行（CI 的 Windows 宿主没有 sh；
    // 引擎本身在 Windows 走 cmd /C，逻辑同一条路）。
    #[cfg(unix)]
    mod engine {
        use super::*;

        fn engine(json: serde_json::Value, dir: &Path) -> HookEngine {
            HookEngine::from_config(cfg(json), dir)
        }

        #[tokio::test]
        async fn exit2_算阻断_stderr_是理由() {
            let t = tempfile::tempdir().expect("目录");
            let e = engine(
                serde_json::json!({
                    "PreToolUse": [{"matcher": "Bash", "hooks": [
                        {"type": "command", "command": "echo '危险命令' >&2; exit 2"}
                    ]}]
                }),
                t.path(),
            );
            let got = e.pre_tool_use("Bash", &serde_json::json!({}), "tu1").await;
            assert_eq!(got, vec![Outcome::Block { reason: "危险命令".into() }]);
        }

        #[tokio::test]
        async fn matcher_不中就不跑() {
            let t = tempfile::tempdir().expect("目录");
            let e = engine(
                serde_json::json!({
                    "PreToolUse": [{"matcher": "Write", "hooks": [
                        {"type": "command", "command": "exit 2"}
                    ]}]
                }),
                t.path(),
            );
            assert!(e.pre_tool_use("Bash", &serde_json::json!({}), "tu1").await.is_empty());
        }

        #[tokio::test]
        async fn stdin_能读到事件_json() {
            let t = tempfile::tempdir().expect("目录");
            // 脚本把 stdin 里的 tool_name 抄给 stderr 再 exit 2 —— 验证载荷送达。
            let e = engine(
                serde_json::json!({
                    "PreToolUse": [{"hooks": [
                        {"type": "command", "command": "grep -o '\"tool_name\":\"[^\"]*\"' >&2; exit 2"}
                    ]}]
                }),
                t.path(),
            );
            let got = e.pre_tool_use("Bash", &serde_json::json!({"command": "ls"}), "tu1").await;
            assert_eq!(got, vec![Outcome::Block { reason: r#""tool_name":"Bash""#.into() }]);
        }

        #[tokio::test]
        async fn 超时当非阻塞错误() {
            let t = tempfile::tempdir().expect("目录");
            let e = engine(
                serde_json::json!({
                    "Stop": [{"hooks": [
                        {"type": "command", "command": "sleep 5; exit 2", "timeout": 1}
                    ]}]
                }),
                t.path(),
            );
            let start = std::time::Instant::now();
            assert!(e.stop(false).await.is_empty(), "超时不算阻断");
            assert!(start.elapsed() < Duration::from_secs(3), "超时要真的掐掉");
        }

        #[tokio::test]
        async fn 非0非2当非阻塞错误() {
            let t = tempfile::tempdir().expect("目录");
            let e = engine(
                serde_json::json!({
                    "Stop": [{"hooks": [{"type": "command", "command": "exit 1"}]}]
                }),
                t.path(),
            );
            assert!(e.stop(false).await.is_empty());
        }

        #[tokio::test]
        async fn stop_闸把_block_合成一个决定() {
            use riot_core::state::{StopDecision, StopGate};
            let t = tempfile::tempdir().expect("目录");
            let e = std::sync::Arc::new(engine(
                serde_json::json!({
                    "Stop": [{"hooks": [
                        {"type": "command", "command": "echo '测试没跑' >&2; exit 2"},
                        {"type": "command", "command": "exit 0"}
                    ]}]
                }),
                t.path(),
            ));
            let gate = HookStopGate(e);
            match gate.check(0).await {
                StopDecision::Block { reason } => assert_eq!(reason, "测试没跑"),
                StopDecision::Allow => panic!("该拦"),
            }
        }

        #[tokio::test]
        async fn stop_hook_active_传给脚本() {
            let t = tempfile::tempdir().expect("目录");
            // 第一次拦、发现 stop_hook_active=true 之后放行 —— CC 手册里
            // 防死循环的标准写法，验证我们把状态带到了。
            let e = engine(
                serde_json::json!({
                    "Stop": [{"hooks": [
                        {"type": "command", "command": "grep -q '\"stop_hook_active\":true' && exit 0; echo '再检查一次' >&2; exit 2"}
                    ]}]
                }),
                t.path(),
            );
            assert_eq!(e.stop(false).await, vec![Outcome::Block { reason: "再检查一次".into() }]);
            assert!(e.stop(true).await.is_empty(), "脚本读到 active 后放行");
        }

        #[tokio::test]
        async fn post_tool_use_的反馈给模型() {
            use riot_protocol::hook::ToolHooks;
            let t = tempfile::tempdir().expect("目录");
            let e = std::sync::Arc::new(engine(
                serde_json::json!({
                    "PostToolUse": [{"matcher": "Write", "hooks": [
                        {"type": "command", "command": "echo '格式不对，跑一下 fmt' >&2; exit 2"}
                    ]}]
                }),
                t.path(),
            ));
            let hooks = HookToolHooks(e);
            assert!(hooks.enabled());
            let got = hooks.post_tool_use("Write", &serde_json::json!({}), "ok", false).await;
            assert_eq!(got, vec!["PostToolUse hook 检查未通过：格式不对，跑一下 fmt".to_owned()]);
        }

        #[tokio::test]
        async fn 同命令跨层去重() {
            let t = tempfile::tempdir().expect("目录");
            let out = t.path().join("count");
            let cmd = format!("echo x >> {}; exit 0", out.display());
            // 同一条命令配了两遍（模拟全局和项目重复）——只跑一次。
            let e = engine(
                serde_json::json!({
                    "Stop": [
                        {"hooks": [{"type": "command", "command": cmd}]},
                        {"hooks": [{"type": "command", "command": cmd}]}
                    ]
                }),
                t.path(),
            );
            let _ = e.stop(false).await;
            let content = std::fs::read_to_string(&out).unwrap_or_default();
            assert_eq!(content.lines().count(), 1, "同命令该去重");
        }
    }
}
