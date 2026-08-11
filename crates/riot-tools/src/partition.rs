//! 分批调度。
//!
//! 把模型一次吐出的多个 `tool_use` 切成若干批：
//!
//! ```text
//! [read A, read B, edit C, read D, read E]
//!   → 并行批 [read A, read B]
//!   → 串行批 [edit C]
//!   → 并行批 [read D, read E]
//! ```
//!
//! # 唯一真正重要的约束
//!
//! `[约束]` **绝不重排。**把后面的 read 提到 edit 前面能提高并行度，
//! 但模型的工具顺序常常隐含依赖 —— "先写配置再读它"这种，重排之后
//! 读到的是旧内容。而且这类 bug 不会报错，只是结果偶尔不对。
//!
//! 所以这里只做「**连续的**安全工具合并」，一个跨越不安全工具的合并都不做。
//!
//! 见 ARCHITECTURE.md §7.2

use riot_protocol::runner::ToolCall;

use crate::registry::Registry;

/// 并行批的大小上限。
///
/// 10 是权衡：再高的话，一批 Read 就能把文件描述符和内存打满，
/// 而收益递减 —— 瓶颈早就从并发度转移到 IO 上了。
pub const DEFAULT_MAX_CONCURRENCY: usize = 10;

#[derive(Debug, Clone, PartialEq)]
pub enum Batch {
    /// 可以同时跑。
    Parallel(Vec<ToolCall>),
    /// 必须独占。
    Serial(ToolCall),
}

impl Batch {
    pub fn calls(&self) -> &[ToolCall] {
        match self {
            Batch::Parallel(v) => v,
            Batch::Serial(c) => std::slice::from_ref(c),
        }
    }

    pub fn len(&self) -> usize {
        self.calls().len()
    }

    pub fn is_empty(&self) -> bool {
        self.calls().is_empty()
    }
}

/// 判定一个调用能否并行。
///
/// `[约束]` 三种情况一律按**不安全**处理：
///
/// - 工具没注册（名字打错、版本不匹配）
/// - `is_concurrency_safe` panic 了
/// - schema 对不上（由工具自己在 `is_concurrency_safe` 里判断）
///
/// 这是 fail-closed：判断不了就串行执行。代价是慢一点，
/// 而反过来（判断不了就并行）的代价是并发写同一个文件。
fn is_safe(call: &ToolCall, registry: &Registry) -> bool {
    let Some(tool) = registry.get(&call.name) else {
        // 未注册的工具后面会被 scheduler 变成一条错误结果。
        // 这里当不安全处理，让它独占一批 —— 万一它其实是个写工具呢。
        return false;
    };

    // 工具是第三方可扩展的（MCP），panic 不能拖垮整个批次。
    // AssertUnwindSafe 在这里成立：我们只读 input，没有跨 unwind 边界的可变状态。
    let input = &call.input;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tool.is_concurrency_safe(input)
    }))
    .unwrap_or_else(|_| {
        tracing::error!(
            tool = %call.name,
            "is_concurrency_safe panic，按不安全处理"
        );
        false
    })
}

/// 分批。**保持原始顺序。**
pub fn partition(calls: Vec<ToolCall>, registry: &Registry, max_concurrency: usize) -> Vec<Batch> {
    // 0 会让并行批永远装不下东西，退化成死循环或空批。
    // 夹到 1 = 全串行，是个安全的降级。
    let max = max_concurrency.max(1);

    let mut batches = Vec::new();
    let mut current: Vec<ToolCall> = Vec::new();

    for call in calls {
        if is_safe(&call, registry) {
            current.push(call);
            if current.len() >= max {
                batches.push(Batch::Parallel(std::mem::take(&mut current)));
            }
        } else {
            // 遇到不安全的：先把攒着的并行批收掉，保证它排在前面
            if !current.is_empty() {
                batches.push(Batch::Parallel(std::mem::take(&mut current)));
            }
            batches.push(Batch::Serial(call));
        }
    }

    if !current.is_empty() {
        batches.push(Batch::Parallel(current));
    }

    debug_assert!(
        batches.iter().all(|b| !b.is_empty()),
        "空批会让 scheduler 吐出一个没有结果的批次"
    );

    batches
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeTool;
    use riot_protocol::id::ToolUseId;
    use riot_protocol::tool::Tool;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;

    fn registry() -> Registry {
        Registry::new(vec![
            Arc::new(FakeTool::read_only("Read")) as Arc<dyn Tool>,
            Arc::new(FakeTool::read_only("Grep")),
            Arc::new(FakeTool::writer("Edit")),
            Arc::new(FakeTool::writer("Write")),
            // Bash 按输入判定：命令以 "ls" / "cat" 开头算只读
            Arc::new(FakeTool::conditional("Bash")),
        ])
        .expect("注册表")
    }

    fn call(id: &str, name: &str) -> ToolCall {
        ToolCall {
            id: ToolUseId::from_raw(id),
            name: name.into(),
            input: serde_json::json!({}),
        }
    }

    fn bash(id: &str, cmd: &str) -> ToolCall {
        ToolCall {
            id: ToolUseId::from_raw(id),
            name: "Bash".into(),
            input: serde_json::json!({ "command": cmd }),
        }
    }

    /// 把分批结果压成好读的形式：["A+B", "C", "D+E"]
    fn shape(batches: &[Batch]) -> Vec<String> {
        batches
            .iter()
            .map(|b| {
                b.calls()
                    .iter()
                    .map(|c| c.id.as_str())
                    .collect::<Vec<_>>()
                    .join("+")
            })
            .collect()
    }

    #[test]
    fn 连续只读合并成一批() {
        let batches = partition(
            vec![call("a", "Read"), call("b", "Grep"), call("c", "Read")],
            &registry(),
            DEFAULT_MAX_CONCURRENCY,
        );
        assert_eq!(shape(&batches), vec!["a+b+c"]);
    }

    #[test]
    fn 写工具独占一批() {
        let batches = partition(
            vec![
                call("a", "Read"),
                call("b", "Read"),
                call("c", "Edit"),
                call("d", "Read"),
                call("e", "Read"),
            ],
            &registry(),
            DEFAULT_MAX_CONCURRENCY,
        );
        assert_eq!(shape(&batches), vec!["a+b", "c", "d+e"]);
    }

    #[test]
    fn 绝不重排() {
        // 把 d、e 提到 c 前面能多并行一批，但模型的顺序常常隐含依赖 ——
        // "先写配置再读它"重排之后读到的是旧内容，而且不报错。
        let batches = partition(
            vec![
                call("c", "Edit"),
                call("d", "Read"),
                call("e", "Read"),
                call("f", "Write"),
            ],
            &registry(),
            DEFAULT_MAX_CONCURRENCY,
        );
        assert_eq!(shape(&batches), vec!["c", "d+e", "f"]);

        // 展平后的顺序必须与输入完全一致
        let flat: Vec<&str> = batches
            .iter()
            .flat_map(|b| b.calls())
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(flat, vec!["c", "d", "e", "f"]);
    }

    #[test]
    fn 连续的写工具各自独占() {
        let batches = partition(
            vec![call("a", "Edit"), call("b", "Edit")],
            &registry(),
            DEFAULT_MAX_CONCURRENCY,
        );
        assert_eq!(
            shape(&batches),
            vec!["a", "b"],
            "两个写操作可能改同一个文件，合批就是并发写"
        );
    }

    #[test]
    fn bash_按命令内容判定() {
        // 同一个工具，ls 可以并行，rm 必须独占
        let batches = partition(
            vec![
                bash("a", "ls -la"),
                bash("b", "cat foo"),
                bash("c", "rm -rf /tmp/x"),
                bash("d", "ls"),
            ],
            &registry(),
            DEFAULT_MAX_CONCURRENCY,
        );
        assert_eq!(shape(&batches), vec!["a+b", "c", "d"]);
    }

    #[test]
    fn 并行批有上限() {
        let calls: Vec<ToolCall> = (0..25).map(|i| call(&format!("t{i}"), "Read")).collect();
        let batches = partition(calls, &registry(), DEFAULT_MAX_CONCURRENCY);

        assert_eq!(batches.len(), 3, "25 个应该切成 10+10+5");
        assert_eq!(batches[0].len(), 10);
        assert_eq!(batches[1].len(), 10);
        assert_eq!(batches[2].len(), 5);
    }

    #[test]
    fn 上限为零时退化成全串行而不是死循环() {
        let batches = partition(vec![call("a", "Read"), call("b", "Read")], &registry(), 0);
        assert_eq!(shape(&batches), vec!["a", "b"]);
    }

    #[test]
    fn 未注册的工具按不安全处理() {
        // 名字打错或版本不匹配。万一它其实是个写工具呢 —— fail-closed。
        let batches = partition(
            vec![call("a", "Read"), call("b", "Unknown"), call("c", "Read")],
            &registry(),
            DEFAULT_MAX_CONCURRENCY,
        );
        assert_eq!(shape(&batches), vec!["a", "b", "c"]);
    }

    #[test]
    fn 判定函数_panic_按不安全处理() {
        // 工具是第三方可扩展的（MCP），一个 panic 不该拖垮整批。
        let reg = Registry::new(vec![
            Arc::new(FakeTool::read_only("Read")) as Arc<dyn Tool>,
            Arc::new(FakeTool::panicking("Evil")),
        ])
        .expect("注册表");

        let batches = partition(
            vec![call("a", "Read"), call("b", "Evil"), call("c", "Read")],
            &reg,
            DEFAULT_MAX_CONCURRENCY,
        );
        assert_eq!(
            shape(&batches),
            vec!["a", "b", "c"],
            "判断不了就串行 —— 代价是慢一点，反过来的代价是并发写同一个文件"
        );
    }

    #[test]
    fn 空输入产出空批次() {
        assert!(partition(vec![], &registry(), DEFAULT_MAX_CONCURRENCY).is_empty());
    }

    #[test]
    fn 不产生空批() {
        // 空批会让 scheduler 吐出一个没有结果的批次
        let batches = partition(
            vec![call("a", "Edit"), call("b", "Edit"), call("c", "Read")],
            &registry(),
            DEFAULT_MAX_CONCURRENCY,
        );
        assert!(batches.iter().all(|b| !b.is_empty()));
    }
}
