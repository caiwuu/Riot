//! 权限测试替身。

use riot_protocol::permission::{
    PermissionContext, PermissionMode, PermissionModeState, PermissionResult, PermissionRule,
};
use riot_protocol::tool::{PromptContext, Tool, ToolContext, ToolOutcome};

use crate::rules::RuleSet;

/// 可配置只读性和 check_permissions 返回值的假工具。
pub struct PermTool {
    name: &'static str,
    read_only: bool,
    says: PermissionResult,
}

impl PermTool {
    pub fn read_only(name: &'static str) -> Self {
        Self {
            name,
            read_only: true,
            says: PermissionResult::Passthrough,
        }
    }

    pub fn writer(name: &'static str) -> Self {
        Self {
            name,
            read_only: false,
            says: PermissionResult::Passthrough,
        }
    }

    /// 配置 `check_permissions` 的返回值。
    pub fn says(mut self, r: PermissionResult) -> Self {
        self.says = r;
        self
    }
}

#[async_trait::async_trait]
impl Tool for PermTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::json_schema!({ "type": "object" })
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        String::new()
    }

    fn describe(&self, _input: &serde_json::Value) -> String {
        self.name.to_owned()
    }

    async fn call(&self, _input: serde_json::Value, _ctx: ToolContext) -> ToolOutcome {
        ToolOutcome::ok_text("ok")
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        self.read_only
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        self.read_only
    }

    fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &PermissionContext,
    ) -> PermissionResult {
        self.says.clone()
    }

    fn target_path(&self, input: &serde_json::Value) -> Option<std::path::PathBuf> {
        input
            .get("path")
            .and_then(|v| v.as_str())
            .map(std::path::PathBuf::from)
    }
}

pub fn ctx_with(mode: PermissionMode) -> PermissionContext {
    PermissionContext {
        mode: PermissionModeState(Some(mode)),
        rules: Vec::new(),
        sandboxed: false,
        can_prompt_user: true,
    }
}

pub fn rules_of(rules: Vec<PermissionRule>) -> RuleSet {
    RuleSet::new(rules)
}
