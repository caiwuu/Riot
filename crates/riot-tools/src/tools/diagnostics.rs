//! `Diagnostics`：跑项目的检查命令，把输出解成结构化的诊断清单。
//!
//! # 为什么值得单独做一个工具
//!
//! 模型也可以自己 `Bash("cargo check")`，但那样它拿到的是几千行原始输出：
//! 编译进度、依赖列表、多行渲染的错误片段、结尾的统计。真正有用的信息
//! （哪个文件第几行、什么错）散在里面，而这些行全都进上下文。
//!
//! 这个工具输出的是 `文件:行:列  级别  消息` 一行一条，按错误优先排序、
//! 有上限。对 Rust 项目尤其对症 —— 编译时间本来就是迭代瓶颈，不该再让
//! 编译输出去挤上下文。
//!
//! # 为什么它不是只读工具
//!
//! `[约束]` `cargo check` 会执行 `build.rs` 和过程宏，`tsc` 会加载
//! 配置里指定的插件 —— 这些都是**任意代码执行**。把它标成只读会让它在
//! 所有权限模式下自动放行，等于开了一条"跑构建脚本不用问"的路。
//!
//! 它交出 `classifier_input`，所以 Auto 模式下小模型可以自动放行它 ——
//! 这正是那一档存在的理由：例行、可重复、失败无痕的操作不该次次打断人。

use std::path::Path;

use async_trait::async_trait;
use riot_protocol::tool::{
    ProcessSpec, PromptContext, ResultBudget, Tool, ToolContext, ToolOutcome, ValidationError,
};
use serde::Deserialize;

/// 一次最多报多少条。
///
/// 超过这个数基本是"某个基础类型改了名，波及整个仓库"，这时前 60 条足够
/// 定位根因，剩下的修完第一条就消失了。全给反而把上下文顶掉。
const MAX_ITEMS: usize = 60;

/// 检查命令的超时。冷缓存下 `cargo check --workspace` 要几分钟。
const TIMEOUT_MS: u64 = 300_000;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    /// 只报路径里含这段子串的诊断（如 `src-tauri/src/session.rs` 或 `riot-tools`）。
    /// 省略 = 全项目。
    #[serde(default)]
    path: Option<String>,
    /// 只报错误，不报警告。改完一处想快速确认有没有编译断时用。
    #[serde(default)]
    errors_only: bool,
}

/// 一条诊断。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    file: String,
    line: u32,
    col: u32,
    /// 已归一成 `error` / `warning`。
    level: Level,
    /// 错误码（`E0382`、`TS2322`）。没有就是空串。
    code: String,
    message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    // Ord 用来排序：错误在前。判别顺序就是优先级顺序，别调。
    Error,
    Warning,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

/// 一套检查工具链：怎么判断项目用它、怎么跑、怎么解。
struct Toolchain {
    name: &'static str,
    /// 项目根下存在这个文件就算用了这套工具链。
    marker: &'static str,
    program: &'static str,
    args: &'static [&'static str],
    /// 诊断在 stdout 还是 stderr。cargo 的 JSON 走 stdout，tsc 走 stdout。
    parse: fn(&str) -> Vec<Item>,
}

const TOOLCHAINS: &[Toolchain] = &[
    Toolchain {
        name: "cargo",
        marker: "Cargo.toml",
        program: "cargo",
        // --all-targets 不能省:不带它测试代码不参与检查，而"改了公共结构体
        // 的字段"这类问题只在测试代码里炸。
        args: &["check", "--workspace", "--all-targets", "--message-format=json"],
        parse: parse_cargo,
    },
    Toolchain {
        name: "tsc",
        marker: "tsconfig.json",
        // 用项目本地的 tsc，不用全局的。版本不一致会报出一堆项目根本没有的错。
        program: "node_modules/.bin/tsc",
        args: &["--noEmit", "--pretty", "false"],
        parse: parse_tsc,
    },
];

pub struct Diagnostics;

/// 解析 `cargo --message-format=json` 的输出。
///
/// 每行一个 JSON。只要 `reason == "compiler-message"` 且有 primary span 的 ——
/// 没有 span 的多是"aborting due to N errors"这类汇总，重复且不指向代码。
fn parse_cargo(stdout: &str) -> Vec<Item> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue; // 非 JSON 行（cargo 偶尔混进人类可读的进度）
        };
        if v.get("reason").and_then(|r| r.as_str()) != Some("compiler-message") {
            continue;
        }
        let msg = &v["message"];
        let level = match msg.get("level").and_then(|l| l.as_str()) {
            Some("error") => Level::Error,
            Some("warning") => Level::Warning,
            // note / help 是上一条的附属说明，单独列出来只会重复。
            _ => continue,
        };
        let Some(span) = msg
            .get("spans")
            .and_then(|s| s.as_array())
            .and_then(|a| a.iter().find(|s| s["is_primary"].as_bool() == Some(true)))
        else {
            continue;
        };
        out.push(Item {
            file: span["file_name"].as_str().unwrap_or_default().to_owned(),
            line: span["line_start"].as_u64().unwrap_or(0) as u32,
            col: span["column_start"].as_u64().unwrap_or(0) as u32,
            level,
            code: msg["code"]["code"].as_str().unwrap_or_default().to_owned(),
            message: first_line(msg["message"].as_str().unwrap_or_default()),
        });
    }
    out
}

/// 解析 `tsc --pretty false` 的输出：`src/a.ts(12,5): error TS2322: 消息`。
fn parse_tsc(stdout: &str) -> Vec<Item> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        // 从右往左拆，因为 Windows 路径里有冒号，而文件名里可能有括号。
        let Some((loc, rest)) = line.split_once("): ") else {
            continue;
        };
        let Some((file, pos)) = loc.rsplit_once('(') else {
            continue;
        };
        let Some((l, c)) = pos.split_once(',') else {
            continue;
        };
        let (level, rest) = if let Some(r) = rest.strip_prefix("error ") {
            (Level::Error, r)
        } else if let Some(r) = rest.strip_prefix("warning ") {
            (Level::Warning, r)
        } else {
            continue;
        };
        // `TS2322: 消息`
        let (code, message) = rest.split_once(": ").unwrap_or(("", rest));
        out.push(Item {
            file: file.to_owned(),
            line: l.parse().unwrap_or(0),
            col: c.parse().unwrap_or(0),
            level,
            code: code.to_owned(),
            message: message.to_owned(),
        });
    }
    out
}

/// 只取第一行。cargo 的 message 偶尔带多行补充说明，那些在这份清单里
/// 是噪音 —— 要细节模型可以去 Read 那个位置。
fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or_default().trim().to_owned()
}

fn render(items: &[Item], truncated_from: Option<usize>, chain: &str) -> String {
    let errors = items.iter().filter(|i| i.level == Level::Error).count();
    let warnings = items.len() - errors;

    let mut s = if errors == 0 && warnings == 0 {
        format!("{chain}：没有诊断，干净。")
    } else {
        format!("{chain}：{errors} 个错误、{warnings} 个警告\n\n")
    };

    for i in items {
        let code = if i.code.is_empty() {
            String::new()
        } else {
            format!("[{}] ", i.code)
        };
        s.push_str(&format!(
            "{}:{}:{}  {}  {}{}\n",
            i.file,
            i.line,
            i.col,
            i.level.as_str(),
            code,
            i.message
        ));
    }

    if let Some(total) = truncated_from {
        s.push_str(&format!(
            "\n[共 {total} 条，只列了前 {}。多半是同一个根因波及一片 —— \
             修掉最前面几条再跑一次。]\n",
            items.len()
        ));
    }
    s
}

#[async_trait]
impl Tool for Diagnostics {
    fn name(&self) -> &'static str {
        "Diagnostics"
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(Input)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "跑项目的类型/编译检查，拿回一份结构化的诊断清单（文件:行:列 + 级别 + 消息）。\n\
         \n\
         改完代码验证有没有编译断、类型错，**用这个而不是在 Bash 里跑 \
         cargo check / tsc**：那样你会收到几千行编译进度和多行渲染的错误片段，\
         而这里一条诊断就是一行。\n\
         \n\
         - 工具链按项目根的标记文件自动选（Cargo.toml → cargo check；\
           tsconfig.json → tsc）。两者都有就都跑。\n\
         - `path` 传一段路径子串只看那一块；`errors_only` 跳过警告。\n\
         - 冷缓存下 Rust 检查要几分钟，别拿它当「顺手看一眼」的工具。\n\
         - 要跑测试、格式检查、clippy 这些，仍然用 Bash —— 这个工具只管\
           编译和类型。"
            .to_owned()
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        match input.get("path").and_then(|v| v.as_str()) {
            Some(p) => format!("检查诊断（{p}）"),
            None => "检查诊断".to_owned(),
        }
    }

    /// `[约束]` 不是只读 —— `cargo check` 跑 build.rs 和过程宏，
    /// 那是任意代码执行。见模块文档。
    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        false
    }

    /// 检查命令之间会抢 target/ 目录的锁，并行跑只会互相等。
    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        false
    }

    /// 交给 Auto 模式的分类器判 —— 例行、可重复、失败无痕，正是该被
    /// 自动放行的那类操作。
    fn classifier_input(&self, _input: &serde_json::Value) -> Option<String> {
        Some("跑项目的编译/类型检查（cargo check / tsc），只读取诊断，不改文件".to_owned())
    }

    /// 清单已经自己限了条数，不需要再落盘。
    fn result_budget(&self) -> ResultBudget {
        ResultBudget::Unlimited
    }

    async fn validate_input(
        &self,
        input: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<(), ValidationError> {
        let _: Input = serde_json::from_value(input.clone())
            .map_err(|e| ValidationError::rejected(format!("参数不对：{e}")))?;

        if detect(&ctx.cwd, ctx).await.is_empty() {
            return Err(ValidationError::rejected(
                "这个项目根下没有 Cargo.toml 也没有 tsconfig.json，认不出检查命令。\
                 用 Bash 跑这个项目自己的检查方式。",
            ));
        }
        Ok(())
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let parsed: Input = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolOutcome::failed(format!("参数不对：{e}")),
        };

        let chains = detect(&ctx.cwd, &ctx).await;
        if chains.is_empty() {
            return ToolOutcome::failed(
                "认不出这个项目的检查命令（没有 Cargo.toml / tsconfig.json）。改用 Bash。",
            );
        }

        let mut sections = Vec::new();
        for chain in chains {
            let spec = ProcessSpec {
                program: chain.program.to_owned(),
                args: chain.args.iter().map(|a| (*a).to_owned()).collect(),
                cwd: ctx.cwd.clone(),
                env: Vec::new(),
                timeout_ms: Some(TIMEOUT_MS),
            };
            let out = match ctx.proc.run(spec, ctx.cancel.clone()).await {
                Ok(o) => o,
                Err(e) => {
                    sections.push(format!("{}：起不来（{e}）", chain.name));
                    continue;
                }
            };
            if out.timed_out {
                sections.push(format!(
                    "{}：{}s 内没跑完，已终止。缩小范围（传 path），或者先跑一次让缓存热起来。",
                    chain.name,
                    TIMEOUT_MS / 1000
                ));
                continue;
            }

            // 诊断可能在两个流里（cargo 的 JSON 在 stdout，但工具链没装
            // 这类硬错误在 stderr），两边都解一遍。
            let mut items = (chain.parse)(&out.stdout);
            items.extend((chain.parse)(&out.stderr));

            if parsed.errors_only {
                items.retain(|i| i.level == Level::Error);
            }
            if let Some(p) = parsed.path.as_deref() {
                items.retain(|i| i.file.contains(p));
            }
            // 同一条诊断会被 cargo 重复报（多个 target 编译同一个文件）。
            items.sort_by(|a, b| {
                a.level
                    .cmp(&b.level)
                    .then_with(|| a.file.cmp(&b.file))
                    .then_with(|| a.line.cmp(&b.line))
                    .then_with(|| a.message.cmp(&b.message))
            });
            items.dedup();

            let total = items.len();
            let truncated = (total > MAX_ITEMS).then_some(total);
            items.truncate(MAX_ITEMS);

            // 没有诊断但退出码非零 = 检查本身失败了（工具链没装、
            // Cargo.toml 有语法错）。这时候报"干净"是撒谎。
            if items.is_empty() && out.exit_code != 0 {
                sections.push(format!(
                    "{}：检查本身失败了（退出码 {}），没有解出诊断。原始 stderr：\n{}",
                    chain.name,
                    out.exit_code,
                    tail(&out.stderr, 20)
                ));
                continue;
            }

            sections.push(render(&items, truncated, chain.name));
        }

        ToolOutcome::ok_text(sections.join("\n"))
    }
}

/// 项目用了哪几套工具链。
async fn detect(root: &Path, ctx: &ToolContext) -> Vec<&'static Toolchain> {
    let mut out = Vec::new();
    for chain in TOOLCHAINS {
        if ctx.fs.metadata(&root.join(chain.marker)).await.is_ok() {
            out.push(chain);
        }
    }
    out
}

/// 末尾若干行。检查本身崩掉时，有用的信息总在最后。
fn tail(s: &str, lines: usize) -> String {
    let all: Vec<&str> = s.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 解析_cargo_的_json_诊断() {
        let out = r#"{"reason":"compiler-artifact","target":{"name":"x"}}
{"reason":"compiler-message","message":{"level":"error","message":"borrow of moved value: `spec.reason`","code":{"code":"E0382"},"spans":[{"is_primary":true,"file_name":"src-tauri/src/session.rs","line_start":2167,"column_start":64}]}}
{"reason":"compiler-message","message":{"level":"warning","message":"unused variable: `x`","code":{"code":"unused_variables"},"spans":[{"is_primary":true,"file_name":"src/a.rs","line_start":10,"column_start":5}]}}
{"reason":"build-finished","success":false}"#;
        let items = parse_cargo(out);
        assert_eq!(items.len(), 2, "只要 compiler-message：{items:?}");
        assert_eq!(items[0].file, "src-tauri/src/session.rs");
        assert_eq!(items[0].line, 2167);
        assert_eq!(items[0].col, 64);
        assert_eq!(items[0].level, Level::Error);
        assert_eq!(items[0].code, "E0382");
        assert_eq!(items[1].level, Level::Warning);
    }

    /// note / help 是上一条错误的附属说明，单独列出来只是重复。
    #[test]
    fn cargo_的_note_和_没有_span_的汇总都跳过() {
        let out = r#"{"reason":"compiler-message","message":{"level":"note","message":"这是补充说明","spans":[{"is_primary":true,"file_name":"a.rs","line_start":1,"column_start":1}]}}
{"reason":"compiler-message","message":{"level":"error","message":"aborting due to 3 previous errors","spans":[]}}"#;
        assert!(parse_cargo(out).is_empty(), "{:?}", parse_cargo(out));
    }

    #[test]
    fn 非_json_行不会让解析崩掉() {
        // cargo 偶尔往 stdout 混人类可读的东西。整段解析不能因此归零。
        let out = "   Compiling riot-core v0.1.0\n{\"reason\":\"compiler-message\",\"message\":{\"level\":\"error\",\"message\":\"boom\",\"spans\":[{\"is_primary\":true,\"file_name\":\"a.rs\",\"line_start\":3,\"column_start\":1}]}}\nnot json at all";
        let items = parse_cargo(out);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].message, "boom");
    }

    #[test]
    fn 解析_tsc_的文本诊断() {
        let out = "src/components/Markdown.tsx(81,11): error TS2322: Type 'string | undefined' is not assignable to type 'string'.\n\
                   src/a.ts(5,1): warning TS6133: 'x' is declared but never used.\n\
                   Found 2 errors.";
        let items = parse_tsc(out);
        assert_eq!(items.len(), 2, "{items:?}");
        assert_eq!(items[0].file, "src/components/Markdown.tsx");
        assert_eq!(items[0].line, 81);
        assert_eq!(items[0].col, 11);
        assert_eq!(items[0].code, "TS2322");
        assert_eq!(items[0].level, Level::Error);
        assert_eq!(items[1].level, Level::Warning);
    }

    /// 消息里带冒号和括号是常态（泛型、路径）。从右往左拆才对。
    #[test]
    fn tsc_消息里的冒号不会切错() {
        let out = "src/a.ts(1,2): error TS1: Type 'Record<string, unknown>' is not assignable.";
        let items = parse_tsc(out);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].file, "src/a.ts");
        assert!(items[0].message.contains("Record<string, unknown>"), "{:?}", items[0].message);
    }

    #[test]
    fn 错误排在警告前面() {
        let mut items = [
            Item {
                file: "b.rs".into(),
                line: 1,
                col: 1,
                level: Level::Warning,
                code: String::new(),
                message: "warn".into(),
            },
            Item {
                file: "a.rs".into(),
                line: 9,
                col: 1,
                level: Level::Error,
                code: String::new(),
                message: "err".into(),
            },
        ];
        items.sort_by(|a, b| a.level.cmp(&b.level).then_with(|| a.file.cmp(&b.file)));
        assert_eq!(items[0].level, Level::Error, "错误必须在前 —— 警告先出会把根因压下去");
    }

    #[test]
    fn 干净的项目要明确说干净() {
        let s = render(&[], None, "cargo");
        assert!(s.contains("干净"), "{s}");
        assert!(!s.contains("0 个错误"), "没有诊断时不该输出一份空清单：{s}");
    }

    #[test]
    fn 超上限时说清总数和该怎么办() {
        let items: Vec<Item> = (0..3)
            .map(|i| Item {
                file: format!("a{i}.rs"),
                line: 1,
                col: 1,
                level: Level::Error,
                code: String::new(),
                message: "x".into(),
            })
            .collect();
        let s = render(&items, Some(120), "cargo");
        assert!(s.contains("共 120 条"), "{s}");
        assert!(s.contains("再跑一次"), "要告诉模型下一步做什么：{s}");
    }

    /// 这个工具标成只读就等于开了一条"跑构建脚本不用问"的路 ——
    /// cargo check 会执行 build.rs 和过程宏。
    #[test]
    fn 不能是只读工具() {
        let d = Diagnostics;
        assert!(!d.is_read_only(&serde_json::json!({})));
        assert!(
            d.classifier_input(&serde_json::json!({})).is_some(),
            "要交判定文本，否则 Auto 模式下永远得手动点"
        );
    }
}
