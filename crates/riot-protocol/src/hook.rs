//! 工具执行的外部检查点（hooks）。
//!
//! 这里只有 **PostToolUse** 一个契约：工具跑完后让用户配置的脚本看一眼
//! （格式化检查、lint、审计留痕），反馈给模型。PreToolUse 不在这里 ——
//! 它是权限决策的一环，长在宿主的权限闸里；Stop hook 是主循环的收尾闸，
//! 契约在 riot-core（`StopGate`）。三个点三种生命周期，硬拢在一个 trait
//! 里的话，实现者要为用不上的方法写空身体，还得猜哪个会被谁调。
//!
//! 执行细节（脚本、超时、matcher）全在宿主实现里 —— 调度器只认这个
//! 窄接口，黄金回放注入 `NoToolHooks` 就能保持确定性。

use async_trait::async_trait;

/// PostToolUse 检查点。宿主实现（跑用户配置的脚本）；默认 [`NoToolHooks`]。
#[async_trait]
pub trait ToolHooks: Send + Sync {
    /// 有没有装任何 hook。调度器靠它跳过为 hook 准备参数的开销
    /// （input 克隆可能是整个文件）。
    fn enabled(&self) -> bool {
        true
    }

    /// 工具执行完毕后调用。返回给模型看的反馈段落，空 = 没意见。
    ///
    /// `[约束]` 实现者必须自己吞掉脚本失败/超时 —— hook 坏了不该拦
    /// 工具链路，最多少一条反馈。
    async fn post_tool_use(
        &self,
        tool: &str,
        input: &serde_json::Value,
        output_preview: &str,
        is_error: bool,
    ) -> Vec<String>;
}

/// 默认实现：没有 hooks（测试、子 agent、没配置的用户）。
pub struct NoToolHooks;

#[async_trait]
impl ToolHooks for NoToolHooks {
    fn enabled(&self) -> bool {
        false
    }

    async fn post_tool_use(
        &self,
        _tool: &str,
        _input: &serde_json::Value,
        _output_preview: &str,
        _is_error: bool,
    ) -> Vec<String> {
        Vec::new()
    }
}
