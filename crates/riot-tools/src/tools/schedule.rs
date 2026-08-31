//! 定时任务工具：模型替用户创建/管理"到点自动跑一轮"的任务。
//!
//! 真正的调度在宿主（riot_protocol::schedule 模块头），这一层只负责
//! 参数校验和把结果说成人话。时间的解析与计算**全在宿主** —— 这里
//! 不做时区运算，报错原样转给模型让它自纠。

use std::sync::Arc;

use async_trait::async_trait;
use riot_protocol::schedule::{ScheduleAccess, ScheduleSpec, ScheduledTask, WhenSpec};
use riot_protocol::tool::{PromptContext, Tool, ToolContext, ToolOutcome};
use serde::Deserialize;

/// 名字和提示词的长度上限。超出的多半是模型把整段对话塞了进来。
const MAX_NAME: usize = 60;
const MAX_PROMPT: usize = 4_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Action {
    Create,
    List,
    Pause,
    Resume,
    Delete,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ScheduleInput {
    /// 要做什么。
    action: Action,
    /// create 必填：任务名（通知和列表显示用，≤60 字）。
    #[serde(default)]
    name: Option<String>,
    /// create 必填：到点注入的提示词。像写给未来的自己 —— 要自带全部
    /// 背景（做什么、看哪里、产出什么），那时不一定有现在的上下文。
    #[serde(default)]
    prompt: Option<String>,
    /// create 必填：什么时候跑。
    #[serde(default)]
    when: Option<WhenSpec>,
    /// create 可选（默认 false）：true = 到点在**当前会话**里续跑，
    /// 上下文都在；false = 每次运行新开一个会话。
    #[serde(default)]
    in_this_session: Option<bool>,
    /// pause / resume / delete 必填：任务 id（list 或创建时返回的）。
    #[serde(default)]
    id: Option<String>,
}

/// 定时任务工具。构造时注入宿主访问端（会话装配时挂远程代理）。
pub struct ScheduleTool {
    access: Arc<dyn ScheduleAccess>,
}

impl ScheduleTool {
    pub fn new(access: Arc<dyn ScheduleAccess>) -> Self {
        Self { access }
    }
}

#[async_trait]
impl Tool for ScheduleTool {
    fn name(&self) -> &'static str {
        "Schedule"
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(ScheduleInput)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        "创建和管理定时任务：到点后你的这段 prompt 会自动作为新消息发出，\
         跑一整轮（工具都能用），跑完通知用户。\n\
         \n\
         什么时候用：用户说「每天早上八点给我份晨报」「下午三点半盯一下收盘」\
         「90 分钟后看看 CI 过没过」；或者你自己承诺了稍后跟进（「要不要我\
         收盘后再扫一次？」用户答应了），就用它把承诺落实 —— 不设任务的话，\
         这句话只是空话。\n\
         \n\
         时间怎么给（when 参数）：\n\
         - `{\"kind\":\"after\",\"minutes\":90}` —— 90 分钟后跑一次。\
           **拿不准当前日期时间就用这个**，相对时间不会错。\n\
         - `{\"kind\":\"once\",\"at\":\"2026-09-01 15:30\"}` —— 指定本地时间跑一次。\
           时间已过会被拒绝并告诉你现在几点，照着改就行。\n\
         - `{\"kind\":\"daily\",\"time\":\"08:00\"}` / `{\"kind\":\"weekdays\",...}` —— \
           每天 / 工作日。\n\
         - `{\"kind\":\"weekly\",\"weekday\":5,\"time\":\"16:00\"}` —— 每周五 16:00\
           （1=周一 … 7=周日）。\n\
         \n\
         in_this_session 怎么选：\n\
         - true：到点在**当前会话**接着跑，上下文都在。适合「这个话题下午\
           再跟进一次」这类一次性跟进。\n\
         - false（默认）：每次新开会话。适合周期性简报 —— 每次独立成篇，\
           不把一个会话越堆越长。\n\
         \n\
         prompt 要自带全部背景：新会话里没有现在的上下文，写清楚做什么、\
         看哪里、产出什么。续跑的任务可以短一些，但也要点明是接着什么说。\n\
         \n\
         创建后向用户复述一遍（任务名、时间、下次运行），确认符合预期。\
         用户想改时间或取消：先 list 拿 id，再 pause / resume / delete。"
            .to_owned()
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let name = input.get("name").and_then(|v| v.as_str());
        match input.get("action").and_then(|v| v.as_str()) {
            Some("create") => match name {
                Some(n) => format!("创建定时任务「{n}」"),
                None => "创建定时任务".to_owned(),
            },
            Some("list") => "查看定时任务".to_owned(),
            Some("pause") => "暂停定时任务".to_owned(),
            Some("resume") => "恢复定时任务".to_owned(),
            Some("delete") => "删除定时任务".to_owned(),
            _ => "管理定时任务".to_owned(),
        }
    }

    fn is_read_only(&self, input: &serde_json::Value) -> bool {
        input.get("action").and_then(|v| v.as_str()) == Some("list")
    }

    fn is_concurrency_safe(&self, input: &serde_json::Value) -> bool {
        self.is_read_only(input)
    }

    async fn call(&self, input: serde_json::Value, ctx: ToolContext) -> ToolOutcome {
        let parsed: ScheduleInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolOutcome::failed(format!("参数不对：{e}")),
        };
        match parsed.action {
            Action::Create => self.create(parsed, &ctx).await,
            Action::List => self.list().await,
            Action::Pause => self.toggle(parsed.id.as_deref(), false).await,
            Action::Resume => self.toggle(parsed.id.as_deref(), true).await,
            Action::Delete => self.delete(parsed.id.as_deref()).await,
        }
    }
}

impl ScheduleTool {
    async fn create(&self, input: ScheduleInput, _ctx: &ToolContext) -> ToolOutcome {
        let Some(name) = input.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
            return ToolOutcome::failed("create 需要 name：给任务起个短名字。");
        };
        let Some(prompt) = input
            .prompt
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return ToolOutcome::failed(
                "create 需要 prompt：到点自动发出的那段话。写清楚做什么、看哪里、产出什么。",
            );
        };
        let Some(when) = input.when else {
            return ToolOutcome::failed(
                "create 需要 when：什么时候跑。例：{\"kind\":\"after\",\"minutes\":90} 或 \
                 {\"kind\":\"daily\",\"time\":\"08:00\"}。",
            );
        };
        if name.chars().count() > MAX_NAME {
            return ToolOutcome::failed(format!("name 太长了（上限 {MAX_NAME} 字），起个短名。"));
        }
        if prompt.chars().count() > MAX_PROMPT {
            return ToolOutcome::failed(format!(
                "prompt 太长了（上限 {MAX_PROMPT} 字）。提炼要点，别把整段对话塞进来。"
            ));
        }

        let spec = ScheduleSpec {
            name: name.to_owned(),
            prompt: prompt.to_owned(),
            when,
            in_this_session: input.in_this_session.unwrap_or(false),
        };
        match self.access.create(spec).await {
            Ok(task) => ToolOutcome::ok_text(format!(
                "已创建定时任务「{}」（id: {}）。\n{}\n\
                 向用户复述一遍时间安排，确认符合预期。",
                task.name,
                task.id,
                render_line(&task),
            )),
            Err(e) => ToolOutcome::failed(e.0),
        }
    }

    async fn list(&self) -> ToolOutcome {
        match self.access.list().await {
            Ok(tasks) if tasks.is_empty() => {
                ToolOutcome::ok_text("现在没有任何定时任务。用 action: \"create\" 创建。")
            }
            Ok(tasks) => {
                let lines: Vec<String> = tasks
                    .iter()
                    .map(|t| format!("[{}]「{}」{}", t.id, t.name, render_line(t)))
                    .collect();
                ToolOutcome::ok_text(lines.join("\n"))
            }
            Err(e) => ToolOutcome::failed(e.0),
        }
    }

    async fn toggle(&self, id: Option<&str>, enabled: bool) -> ToolOutcome {
        let verb = if enabled { "resume" } else { "pause" };
        let Some(id) = id.map(str::trim).filter(|s| !s.is_empty()) else {
            return ToolOutcome::failed(format!("{verb} 需要 id。先用 list 查任务 id。"));
        };
        match self.access.set_enabled(id, enabled).await {
            Ok(task) if enabled => ToolOutcome::ok_text(format!(
                "已恢复「{}」。{}",
                task.name,
                render_line(&task)
            )),
            Ok(task) => ToolOutcome::ok_text(format!("已暂停「{}」。恢复用 resume。", task.name)),
            Err(e) => ToolOutcome::failed(e.0),
        }
    }

    async fn delete(&self, id: Option<&str>) -> ToolOutcome {
        let Some(id) = id.map(str::trim).filter(|s| !s.is_empty()) else {
            return ToolOutcome::failed("delete 需要 id。先用 list 查任务 id。");
        };
        match self.access.delete(id).await {
            Ok(()) => ToolOutcome::ok_text(format!("已删除定时任务 {id}。")),
            Err(e) => ToolOutcome::failed(e.0),
        }
    }
}

/// 一个任务的一行描述：重复规则 + 下次运行 + 在哪跑 + 状态。
fn render_line(t: &ScheduledTask) -> String {
    use riot_protocol::schedule::Repeat;
    let repeat = match &t.repeat {
        Repeat::Once => "一次性".to_owned(),
        Repeat::Daily { time } => format!("每天 {time}"),
        Repeat::Weekdays { time } => format!("工作日 {time}"),
        Repeat::Weekly { weekday, time } => format!("每{} {time}", weekday_name(*weekday)),
    };
    let next = match (&t.next_run_local, t.enabled) {
        (_, false) => "已暂停".to_owned(),
        (Some(local), true) => format!("下次运行 {local}"),
        (None, true) => "不再运行".to_owned(),
    };
    let target = match &t.session_id {
        Some(_) => "在原会话续跑",
        None => "每次新开会话",
    };
    format!("{repeat}，{next}，{target}。")
}

fn weekday_name(d: u8) -> &'static str {
    match d {
        1 => "周一",
        2 => "周二",
        3 => "周三",
        4 => "周四",
        5 => "周五",
        6 => "周六",
        7 => "周日",
        _ => "周?",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use riot_protocol::schedule::{Repeat, ScheduleError};

    use super::*;

    /// 记录调用并按脚本应答的替身。
    #[derive(Default)]
    struct FakeAccess {
        created: Mutex<Vec<ScheduleSpec>>,
        tasks: Mutex<Vec<ScheduledTask>>,
    }

    fn task(id: &str, name: &str) -> ScheduledTask {
        ScheduledTask {
            id: id.into(),
            name: name.into(),
            prompt: "p".into(),
            repeat: Repeat::Daily {
                time: "08:00".into(),
            },
            session_id: None,
            root: "/w".into(),
            enabled: true,
            next_run_ms: Some(1_000),
            next_run_local: Some("2026-09-01 08:00".into()),
            last_run_ms: None,
            last_run_local: None,
            last_session_id: None,
            created_at_ms: 1,
        }
    }

    #[async_trait]
    impl ScheduleAccess for FakeAccess {
        async fn create(&self, spec: ScheduleSpec) -> Result<ScheduledTask, ScheduleError> {
            self.created.lock().expect("锁").push(spec.clone());
            Ok(task("t1", &spec.name))
        }
        async fn list(&self) -> Result<Vec<ScheduledTask>, ScheduleError> {
            Ok(self.tasks.lock().expect("锁").clone())
        }
        async fn set_enabled(
            &self,
            id: &str,
            enabled: bool,
        ) -> Result<ScheduledTask, ScheduleError> {
            let mut t = task(id, "晨报");
            t.enabled = enabled;
            Ok(t)
        }
        async fn delete(&self, _id: &str) -> Result<(), ScheduleError> {
            Ok(())
        }
    }

    fn ctx() -> ToolContext {
        let id = riot_protocol::id::ToolUseId::from_raw("t1");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        ToolContext {
            session_id: riot_protocol::id::SessionId::from_raw("s1"),
            tool_use_id: id.clone(),
            cwd: "/work".into(),
            artifacts_dir: "/artifacts".into(),
            cancel: tokio_util::sync::CancellationToken::new(),
            progress: riot_protocol::tool::ProgressSink::new(id, tx),
            file_state: Arc::new(crate::testing::NullFileState),
            fs: Arc::new(crate::tools::memfs::MemFs::new()),
            proc: Arc::new(crate::testing::NullProc),
            web: Arc::new(riot_protocol::web::NoWeb),
            browser: Arc::new(riot_protocol::browser::NoBrowser),
            terminal: Arc::new(riot_protocol::terminal::NoTerminal),
            vision: Arc::new(riot_protocol::vision::NoVision),
            clock: Arc::new(crate::testing::FixedClock::default()),
        }
    }

    #[tokio::test]
    async fn 创建成功要提醒复述给用户() {
        let access = Arc::new(FakeAccess::default());
        let tool = ScheduleTool::new(Arc::clone(&access) as Arc<dyn ScheduleAccess>);
        let out = tool
            .call(
                serde_json::json!({
                    "action": "create",
                    "name": "晨报",
                    "prompt": "给我一份晨间简报",
                    "when": {"kind": "daily", "time": "08:00"},
                }),
                ctx(),
            )
            .await;
        let ToolOutcome::Ok { model_content, .. } = out else {
            panic!("该成功：{out:?}");
        };
        let text = format!("{model_content:?}");
        assert!(text.contains("t1"), "要带 id，之后暂停/删除靠它：{text}");
        assert!(text.contains("复述"), "要引导模型向用户确认：{text}");
        let created = access.created.lock().expect("锁");
        assert_eq!(created.len(), 1);
        assert!(!created[0].in_this_session, "没说就是新会话");
    }

    #[tokio::test]
    async fn 缺参数的失败要指路而不是空泛报错() {
        let tool = ScheduleTool::new(Arc::new(FakeAccess::default()));
        let out = tool
            .call(serde_json::json!({"action": "create", "name": "x"}), ctx())
            .await;
        let ToolOutcome::Failed {
            error_for_model, ..
        } = out
        else {
            panic!("缺 prompt 该失败：{out:?}");
        };
        assert!(error_for_model.contains("prompt"), "{error_for_model}");

        let out = tool.call(serde_json::json!({"action": "pause"}), ctx()).await;
        let ToolOutcome::Failed {
            error_for_model, ..
        } = out
        else {
            panic!("缺 id 该失败：{out:?}");
        };
        assert!(error_for_model.contains("list"), "要指路先查 id：{error_for_model}");
    }

    #[tokio::test]
    async fn 列表为空时指路创建() {
        let tool = ScheduleTool::new(Arc::new(FakeAccess::default()));
        let out = tool.call(serde_json::json!({"action": "list"}), ctx()).await;
        let ToolOutcome::Ok { model_content, .. } = out else {
            panic!("空列表也该成功：{out:?}");
        };
        assert!(
            format!("{model_content:?}").contains("create"),
            "空结果要指路（空串会被部分模型当成任务结束）"
        );
    }

    #[tokio::test]
    async fn 没接宿主时明说用不了() {
        let tool = ScheduleTool::new(Arc::new(riot_protocol::schedule::NoSchedule));
        let out = tool.call(serde_json::json!({"action": "list"}), ctx()).await;
        let ToolOutcome::Failed {
            error_for_model, ..
        } = out
        else {
            panic!("该失败：{out:?}");
        };
        assert!(error_for_model.contains("定时任务"), "{error_for_model}");
    }
}
