//! L3 黄金回放。
//!
//! 原理：agent 的行为是模型响应的函数。把模型响应固定下来，输出的事件序列
//! 就应该完全确定。任何一次改动导致序列变化，都会在这里显形。
//!
//! 用例格式：
//!
//! ```text
//! tests/golden/<case>/
//! ├── case.json        输入、模型、轮数上限、工具预设结果
//! ├── responses/       按请求顺序编号的模型响应
//! │   ├── 001.json
//! │   └── 002.json
//! └── expected.jsonl   期望的事件序列，每行一个 AgentEvent
//! ```
//!
//! 改坏了主循环之后想让测试变绿，可以跑：
//!
//! ```bash
//! UPDATE_GOLDEN=1 cargo test -p riot-core --test golden
//! ```
//!
//! `[约束]` 更新基准后**必须逐行读一遍 diff 再提交**。基准记录的是当前行为，
//! 不是正确行为。如果当前行为就是错的，把它录成基准之后，以后真正的修复
//! 反而会被这个测试挡住 —— 那时这层防线就从资产变成了负债。
//!
//! 见 docs/VERIFICATION.md §4

#![allow(clippy::disallowed_methods)] // 测试 harness 要读磁盘

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::StreamExt;
use riot_protocol::event::AgentEvent;
use riot_protocol::id::SessionId;
use riot_protocol::provider::ProviderEvent;
use tokio_util::sync::CancellationToken;

use riot_core::state::AgentState;
use riot_core::testing::{
    FakeCompactor, ScriptedProvider, ScriptedResult, ScriptedToolRunner, mock_deps_with,
};

#[derive(Debug, serde::Deserialize)]
struct CaseSpec {
    /// 用例在测什么。写清楚，失败时它是第一条线索。
    #[allow(dead_code)]
    description: String,
    prompt: String,
    #[serde(default = "default_model")]
    model: String,
    #[serde(default = "default_max_turns")]
    max_turns: u32,
    #[serde(default)]
    tools: HashMap<String, ScriptedResult>,
    /// 跑到第几个事件时触发中断。None = 不中断。
    #[serde(default)]
    cancel_after_events: Option<usize>,
    /// 前 n 次压缩返回失败。用来覆盖「压缩失败但仍继续重试」这条路径。
    #[serde(default)]
    compactor_fails_first: usize,
}

fn default_model() -> String {
    "test-model".into()
}
fn default_max_turns() -> u32 {
    8
}

struct Case {
    name: String,
    dir: PathBuf,
    spec: CaseSpec,
    responses: Vec<Vec<ProviderEvent>>,
}

impl Case {
    fn load(dir: &Path) -> Self {
        let name = dir
            .file_name()
            .expect("用例目录名")
            .to_string_lossy()
            .into_owned();

        let spec_path = dir.join("case.json");
        let spec: CaseSpec = serde_json::from_str(
            &std::fs::read_to_string(&spec_path)
                .unwrap_or_else(|e| panic!("读不到 {}: {e}", spec_path.display())),
        )
        .unwrap_or_else(|e| panic!("{} 格式错误: {e}", spec_path.display()));

        let mut files: Vec<PathBuf> = std::fs::read_dir(dir.join("responses"))
            .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
            .unwrap_or_default();
        files.sort();

        let responses = files
            .iter()
            .map(|f| {
                serde_json::from_str(
                    &std::fs::read_to_string(f).unwrap_or_else(|e| panic!("读不到 {f:?}: {e}")),
                )
                .unwrap_or_else(|e| panic!("{f:?} 不是合法的 ProviderEvent 数组: {e}"))
            })
            .collect();

        Self {
            name,
            dir: dir.to_path_buf(),
            spec,
            responses,
        }
    }

    fn expected(&self) -> Vec<AgentEvent> {
        let path = self.dir.join("expected.jsonl");
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Vec::new();
        };
        raw.lines()
            .filter(|l| !l.trim().is_empty())
            .enumerate()
            .map(|(i, line)| {
                serde_json::from_str(line).unwrap_or_else(|e| {
                    panic!("{} 第 {} 行不是合法事件: {e}", path.display(), i + 1)
                })
            })
            .collect()
    }

    async fn run(&self) -> Vec<AgentEvent> {
        let provider = Arc::new(ScriptedProvider::new(self.responses.clone()));
        let tools = Arc::new(ScriptedToolRunner::new(self.spec.tools.clone()));
        let deps = mock_deps_with(
            provider,
            tools,
            Arc::new(FakeCompactor::failing(self.spec.compactor_fails_first)),
        );

        let state = AgentState::new(SessionId::from_raw("golden"), &self.spec.model)
            .with_max_turns(self.spec.max_turns)
            .with_messages(vec![riot_core::testing::user_text(
                "msg_in",
                &self.spec.prompt,
            )]);

        let cancel = CancellationToken::new();
        let stream = riot_core::run_agent(state, deps, cancel.clone());
        futures::pin_mut!(stream);

        let mut out = Vec::new();
        let mut seen = 0usize;
        while let Some(ev) = stream.next().await {
            out.push(ev);
            seen += 1;
            if Some(seen) == self.spec.cancel_after_events {
                cancel.cancel();
            }
        }
        out
    }
}

/// 只保留进 transcript 的事件。
///
/// `[约束]` 断言必须忽略 `Delta` 和 `Progress`。它们是渲染细节，把它们写进
/// 基准会让用例极其脆弱 —— 调整一下流式切分的分块大小，所有用例一起变红，
/// 而实际行为一点没变。这种"改对了也报错"的测试很快就会被所有人无视。
fn durable(events: &[AgentEvent]) -> Vec<AgentEvent> {
    events.iter().filter(|e| e.is_durable()).cloned().collect()
}

fn discover_cases() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("读不到用例目录 {}: {e}", root.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs
}

fn write_baseline(case: &Case, actual: &[AgentEvent]) {
    let mut out = String::new();
    for ev in actual {
        out.push_str(&serde_json::to_string(ev).expect("事件必须可序列化"));
        out.push('\n');
    }
    std::fs::write(case.dir.join("expected.jsonl"), out).expect("写基准");
}

#[tokio::test]
async fn golden_replay() {
    let update = std::env::var("UPDATE_GOLDEN").is_ok();
    let cases = discover_cases();
    assert!(!cases.is_empty(), "一个用例都没有，这层防线是空的");

    let mut failures = Vec::new();

    for dir in cases {
        let case = Case::load(&dir);
        let actual = durable(&case.run().await);

        if update {
            write_baseline(&case, &actual);
            continue;
        }

        let expected = case.expected();
        if actual != expected {
            failures.push(format_diff(&case.name, &expected, &actual));
        }
    }

    if update {
        // 刻意让更新模式失败。跑过 UPDATE_GOLDEN 之后测试变绿会让人误以为
        // 问题解决了，而实际上只是把当前行为录成了新基准。
        panic!("基准已更新。请逐行审阅 git diff 后再提交，然后不带 UPDATE_GOLDEN 重跑。");
    }

    assert!(failures.is_empty(), "\n\n{}", failures.join("\n\n"));
}

fn format_diff(name: &str, expected: &[AgentEvent], actual: &[AgentEvent]) -> String {
    let mut s = format!("用例 `{name}` 的事件序列变了：\n");
    let n = expected.len().max(actual.len());
    for i in 0..n {
        let e = expected.get(i).map(summarize);
        let a = actual.get(i).map(summarize);
        match (e, a) {
            (Some(e), Some(a)) if e == a => s.push_str(&format!("     {i:>2}  {e}\n")),
            (Some(e), Some(a)) => {
                s.push_str(&format!("  -  {i:>2}  {e}\n"));
                s.push_str(&format!("  +  {i:>2}  {a}\n"));
            }
            (Some(e), None) => s.push_str(&format!("  -  {i:>2}  {e}  （少了这个事件）\n")),
            (None, Some(a)) => s.push_str(&format!("  +  {i:>2}  {a}  （多了这个事件）\n")),
            (None, None) => {}
        }
    }
    s.push_str("\n  确认新行为正确后：UPDATE_GOLDEN=1 cargo test -p riot-core --test golden");
    s
}

/// 事件的一行摘要。完整 JSON 太长，diff 里看不清到底哪不一样。
fn summarize(ev: &AgentEvent) -> String {
    match ev {
        AgentEvent::RequestStart { turn, after, .. } => match after {
            Some(t) => format!("request_start turn={turn} after={t:?}"),
            None => format!("request_start turn={turn}"),
        },
        AgentEvent::Message(m) => match m {
            riot_protocol::message::Message::Assistant { content, .. } => {
                let kinds: Vec<&str> = content
                    .iter()
                    .map(|c| match c {
                        riot_protocol::message::AssistantContent::Text { .. } => "text",
                        riot_protocol::message::AssistantContent::Thinking { .. } => "thinking",
                        riot_protocol::message::AssistantContent::ToolUse { name, .. } => name,
                    })
                    .collect();
                format!("assistant [{}]", kinds.join(", "))
            }
            riot_protocol::message::Message::User { content, .. } => {
                let n = content
                    .iter()
                    .filter(|c| {
                        matches!(c, riot_protocol::message::UserContent::ToolResult { .. })
                    })
                    .count();
                format!("user [{n} 个 tool_result]")
            }
            riot_protocol::message::Message::System { level, text, .. } => {
                format!("system {level:?} {text:?}")
            }
        },
        AgentEvent::Done { reason } => format!("done {reason:?}"),
        AgentEvent::Compacted { strategy, .. } => format!("compacted {strategy:?}"),
        AgentEvent::PermissionRequest { detail, .. } => {
            format!("permission_request {}", detail.tool_name)
        }
        other => format!("{other:?}"),
    }
}
