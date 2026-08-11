//! Glob 工具。按**文件名**找文件。
//!
//! 和 [`super::grep`] 的分工：Grep 搜内容，Glob 搜路径。没有这个工具的话，
//! "项目里的 Python 文件在哪"只能靠 `Bash(find ...)`—— 那要过一遍命令权限
//! 弹窗，还绕开了 .gitignore。
//!
//! 底层同样是 ripgrep（`--files`），理由和 Grep 一致：gitignore 处理和
//! 并行遍历不值得自己写一遍。参数走 argv，不经过 shell。

use std::path::PathBuf;

use async_trait::async_trait;
use riot_protocol::permission::{PermissionContext, PermissionResult};
use riot_protocol::tool::{
    ProcessSpec, PromptContext, ResultBudget, Tool, ToolContext, ToolOutcome, UiPayload,
    ValidationError,
};
use serde::Deserialize;

use super::path;

/// 遍历超时。理由同 Grep：ripgrep 很快，超时基本意味着走进了网络盘。
const TIMEOUT_MS: u64 = 30_000;

/// 返回给模型的路径条数上限。
///
/// 比 Grep 的 500 行小：路径列表的信息密度低得多，300 条已经足够
/// 判断"东西大概在哪儿"，再多只是烧上下文。
const MAX_RESULTS: usize = 300;

/// 愿意为排序花掉的 stat 次数。
///
/// 超过这个数就退回字典序 —— 几千次 stat 的耗时会盖过 ripgrep 本身，
/// 而结果多到这个程度时模型该做的是缩小 pattern，不是仔细读列表。
const STAT_BUDGET: usize = 1000;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    /// 文件名匹配模式，如 `**/*.rs` 或 `src/**/test_*.py`。
    pattern: String,
    /// 搜索的根目录。省略则搜索工作目录。
    #[serde(default)]
    path: Option<String>,
    /// 最多返回几条。
    #[serde(default)]
    head_limit: Option<usize>,
}

pub struct Glob;

#[async_trait]
impl Tool for Glob {
    fn name(&self) -> &'static str {
        "Glob"
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(Input)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "按文件名找文件，返回路径列表，最近修改的排在前面。\n\
         \n\
         - 想按**文件名**找文件用这个；想按**文件内容**搜用 Grep。\n\
         - 优先用这个而不是 `Bash(find ...)`：不用等命令授权，也会自动\
         跳过 .gitignore 里的文件和 .git 目录。\n\
         - `pattern` 是 glob 不是正则：`**/*.rs` 匹配所有 Rust 文件，\
         `src/**/*.ts` 只看 src 下面。\n\
         - 不确定项目结构时，先用 `**/*.{扩展名}` 摸清范围再逐个 Read。"
            .to_owned()
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let pat = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("...");
        match input.get("path").and_then(|v| v.as_str()) {
            Some(p) => format!("在 {p} 里查找 {pat}"),
            None => format!("查找 {pat}"),
        }
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    /// 结果在工具内部就截断到 [`MAX_RESULTS`] 了，不需要外层的落盘机制。
    fn result_budget(&self) -> ResultBudget {
        ResultBudget::Unlimited
    }

    fn target_path(&self, input: &serde_json::Value) -> Option<PathBuf> {
        input.get("path").and_then(|v| v.as_str()).map(PathBuf::from)
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &PermissionContext,
    ) -> PermissionResult {
        // 交给通用决策链。理由同 Grep：列出 `~/.ssh` 下的文件名同样是
        // 一次读取，这里表态会绕过 safety 层对凭证目录的拦截。
        PermissionResult::Passthrough
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        let parsed: Input = serde_json::from_value(input.clone())
            .map_err(|e| ValidationError::rejected(schema_hint(&e)))?;

        if parsed.pattern.trim().is_empty() {
            return Err(ValidationError::rejected(
                "`pattern` 不能为空。要列出目录下所有文件用 `**/*`。",
            ));
        }
        Ok(())
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let parsed: Input = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolOutcome::failed(schema_hint(&e)),
        };

        // 搜索根要过围栏。不查的话 `path: "../../"` 就能列出工作目录外面。
        let root = match &parsed.path {
            Some(p) => match path::resolve(p, &ctx, true).await {
                Ok(r) => r,
                Err(e) => return ToolOutcome::failed(e.for_model()),
            },
            None => ctx.cwd.clone(),
        };

        let spec = ProcessSpec {
            program: "rg".to_owned(),
            args: build_args(&parsed, &root),
            cwd: ctx.cwd.clone(),
            env: Vec::new(),
            timeout_ms: Some(TIMEOUT_MS),
        };

        let out = match ctx.proc.run(spec, ctx.cancel.clone()).await {
            Ok(o) => o,
            Err(e) => return ToolOutcome::failed(spawn_hint(&e)),
        };

        if out.timed_out {
            return ToolOutcome::failed(format!(
                "遍历超过 {}s 未完成。请用 `path` 缩小范围。",
                TIMEOUT_MS / 1000
            ));
        }

        // ripgrep 的退出码：0 有匹配，1 无匹配，2 出错。
        //
        // `[约束]` 1 不是失败。"没找到"是一个有效答案 —— 报成失败会让模型
        // 换个参数把同一件事再做一遍，而正确的下一步是换个思路或接受它。
        match out.exit_code {
            1 => return ToolOutcome::ok_text(no_match_text(&parsed)),
            0 => {}
            _ => return ToolOutcome::failed(rg_error_hint(&out.stderr)),
        }

        let mut files: Vec<String> = out
            .stdout
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect();

        if files.is_empty() {
            return ToolOutcome::ok_text(no_match_text(&parsed));
        }

        let total = files.len();
        let by_mtime = sort_by_mtime(&mut files, &ctx).await;

        let limit = parsed.head_limit.unwrap_or(MAX_RESULTS).min(MAX_RESULTS);
        files.truncate(limit);
        let shown = files.len();

        let listing = files.join("\n");
        let mut body = listing.clone();

        if shown < total {
            body.push_str(&format!(
                "\n\n<system-reminder>共 {total} 个文件，这里显示前 {shown} 个（{}）。\
                 让 `pattern` 更具体，或用 `path` 缩小到某个子目录。</system-reminder>",
                if by_mtime {
                    "按最近修改排序"
                } else {
                    "文件太多，按路径排序"
                }
            ));
        }

        ToolOutcome::Ok {
            model_content: riot_protocol::message::ToolResultContent::text(body),
            ui_payload: Some(UiPayload::Plain { text: listing }),
            side_messages: Vec::new(),
        }
    }
}

/// 按最近修改排序，返回是否真的排成了。
///
/// 最近改过的文件几乎总是当前任务相关的那些 —— 结果被截断时，这个顺序
/// 决定了模型看到的是不是有用的那一批。文件太多就退回字典序：
/// 那时 stat 的开销盖过收益，而且字典序至少是稳定可预期的。
async fn sort_by_mtime(files: &mut [String], ctx: &ToolContext) -> bool {
    if files.len() > STAT_BUDGET {
        files.sort();
        return false;
    }

    let mut stamped: Vec<(u64, &String)> = Vec::with_capacity(files.len());
    for f in files.iter() {
        // stat 失败（刚被删掉、权限不够）当作最旧，排在后面。
        // 为一个拿不到时间的文件放弃整次排序不划算。
        let mtime = ctx
            .fs
            .metadata(std::path::Path::new(f))
            .await
            .map(|m| m.mtime_ms)
            .unwrap_or(0);
        stamped.push((mtime, f));
    }

    // 次序按路径兜底：同一秒写出的一批文件（生成代码、checkout）
    // 否则顺序不稳，同样的调用两次给出不同结果。
    stamped.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));

    let sorted: Vec<String> = stamped.into_iter().map(|(_, f)| f.clone()).collect();
    files.clone_from_slice(&sorted);
    true
}

fn build_args(input: &Input, root: &std::path::Path) -> Vec<String> {
    vec![
        // 只列文件，不搜内容。
        "--files".into(),
        // `[约束]` --no-config 不能省。用户的 RIPGREP_CONFIG_PATH 里可能有
        // 改变遍历行为的开关，那会让同一次查找在不同机器上给出不同结果。
        "--no-config".into(),
        "--color=never".into(),
        // 点开头的目录也要进。`.github/workflows/*.yml`、`.cargo/config.toml`
        // 都是用户会问起的文件，默认跳过它们这个工具会显得时灵时不灵。
        "--hidden".into(),
        "--glob".into(),
        input.pattern.clone(),
        // `[约束]` 排除 .git 必须排在用户 pattern **之后**。ripgrep 里后写的
        // glob 优先级更高 —— 顺序反过来的话 `**/*` 会把 .git 重新放进来，
        // 结果是一堆 object 文件淹没真正的答案。
        "--glob".into(),
        "!.git/".into(),
        // `--` 之后全是路径。搜索根可能以 `-` 开头。
        "--".into(),
        root.to_string_lossy().into_owned(),
    ]
}

fn no_match_text(input: &Input) -> String {
    let where_ = input.path.as_deref().unwrap_or("工作目录");
    format!(
        "在{where_}里没有找到匹配 `{}` 的文件。\n\
         注意 .gitignore 里的文件和 .git 目录不会被列出。\n\
         `pattern` 是 glob 不是正则 —— 找所有 Rust 文件应该写 `**/*.rs`。",
        input.pattern
    )
}

fn spawn_hint(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => {
            "找不到 ripgrep（rg）。请先安装它，或者改用 Bash 里的 find。".to_owned()
        }
        _ => format!("启动查找失败：{e}"),
    }
}

fn rg_error_hint(stderr: &str) -> String {
    let first = stderr.lines().next().unwrap_or("").trim();
    if first.is_empty() {
        return "查找失败，没有更多信息。".to_owned();
    }
    format!("查找失败：{first}")
}

fn schema_hint(e: &serde_json::Error) -> String {
    let raw = e.to_string();
    if raw.contains("missing field `pattern`") {
        return "缺少必需参数 `pattern`。请提供文件名匹配模式，如 `**/*.rs`。".to_owned();
    }
    if raw.contains("unknown field") {
        return format!(
            "Glob 接受的参数是 `pattern`、`path`、`head_limit`。\
             搜索文件内容请改用 Grep。（{raw}）"
        );
    }
    format!("参数格式不对：{raw}。请检查参数类型。")
}
