//! Skill 工具：把用户预先写好的流程文档按需加载进上下文。
//!
//! # 渐进披露（Claude Code Agent Skills 的核心机制）
//!
//! 技能清单（名字 + 一句话描述）进工具的 prompt；**正文只在被调用时**
//! 才进上下文。十个技能全文可能有几万 token，而模型每轮真正需要的
//! 通常是零或一个 —— 清单是目录，不是内容。
//!
//! # 职责边界
//!
//! 这个工具是**纯数据**的：技能从哪些目录发现、frontmatter 怎么解析，
//! 都是宿主的事（真实文件系统属于宿主层）。宿主每轮扫描一次、把结果
//! 装进来 —— 用户改了 SKILL.md，下一轮就生效，不用重启。

use async_trait::async_trait;
use serde::Deserialize;
use std::path::PathBuf;

use riot_protocol::tool::{PromptContext, Tool, ToolContext, ToolOutcome, UiPayload};

/// 一个可用的技能。
#[derive(Debug, Clone)]
pub struct SkillCard {
    pub name: String,
    /// 一句话描述：什么时候该用它。进清单，是模型选择的唯一依据。
    pub description: String,
    /// SKILL.md 所在目录。正文里的相对路径以它为基准。
    pub dir: PathBuf,
    /// SKILL.md 去掉 frontmatter 之后的正文。
    pub body: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct Input {
    /// 要加载的技能名，来自工具描述里的清单。
    name: String,
    /// 传给技能的参数。技能正文里的 `$ARGUMENTS` 会被替换成它。
    #[serde(default)]
    args: Option<String>,
}

pub struct SkillTool {
    skills: Vec<SkillCard>,
}

impl SkillTool {
    pub fn new(skills: Vec<SkillCard>) -> Self {
        Self { skills }
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        "Skill"
    }

    fn input_schema(&self) -> schemars::Schema {
        schemars::schema_for!(Input)
    }

    fn prompt(&self, _ctx: &PromptContext) -> String {
        let mut p = String::from(
            "加载一个技能。技能是用户预先写好的流程文档或领域知识，\
             当用户的请求命中某个技能的描述时，先加载它再动手 —— \
             里面有用户希望你遵循的具体做法。\n\n可用技能：\n",
        );
        for s in &self.skills {
            // 描述截断进清单：这里的预算是每轮都付的，一个啰嗦的描述
            // 会永久占用上下文（AGENT_DESIGN 给的硬顶是 250 字符）。
            let desc: String = s.description.chars().take(250).collect();
            p.push_str(&format!("- {}: {desc}\n", s.name));
        }
        p.push_str("\n正文只在加载时进入上下文，所以不要凭名字猜内容 —— 觉得相关就加载。");
        p
    }

    fn describe(&self, input: &serde_json::Value) -> String {
        let name = input.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        format!("加载技能 {name}")
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn call(&self, input: serde_json::Value, _ctx: ToolContext) -> ToolOutcome {
        let input: Input = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return ToolOutcome::failed(format!("参数不对：{e}")),
        };

        let Some(skill) = self.skills.iter().find(|s| s.name == input.name) else {
            let names: Vec<&str> = self.skills.iter().map(|s| s.name.as_str()).collect();
            return ToolOutcome::failed(format!(
                "没有叫「{}」的技能。可用的是：{}。从里面挑一个，不要发明名字。",
                input.name,
                names.join("、"),
            ));
        };

        let mut body = skill.body.clone();
        // 占位符替换。没提供参数时留一句说明而不是空串 ——
        // 空串会让"$ARGUMENTS 应该出现的位置"变成一个悄无声息的洞。
        if body.contains("$ARGUMENTS") {
            let args = input.args.as_deref().unwrap_or("（调用时没有提供参数）");
            body = body.replace("$ARGUMENTS", args);
        }
        // 内置技能（编进二进制的那些）没有目录，替换成空串只会把正文弄坏 ——
        // 而正文里出现这个 token 时，最可能的情况是它在**讲解**这个占位符
        // （「扩展 Riot」那个内置技能就是）。留着原样比换成空串诚实。
        let no_dir = skill.dir.as_os_str().is_empty();
        if !no_dir {
            body = body.replace("${SKILL_DIR}", &skill.dir.display().to_string());
        }

        let text = if no_dir {
            format!("# 技能：{}\n\n{}", skill.name, body.trim())
        } else {
            format!(
                "# 技能：{}\n（目录：{}。正文里的相对路径以它为基准，需要时用 Read 读取。）\n\n{}",
                skill.name,
                skill.dir.display(),
                body.trim(),
            )
        };
        ToolOutcome::Ok {
            ui_payload: Some(UiPayload::Plain {
                text: format!("已加载技能「{}」（{} 字符）", skill.name, text.chars().count()),
            }),
            model_content: riot_protocol::message::ToolResultContent::text(text),
            side_messages: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FixedClock, NullFileState, NullFs, NullProc};
    use riot_protocol::id::{SessionId, ToolUseId};
    use riot_protocol::tool::{ProgressSink, ToolContext};
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    fn card(name: &str, desc: &str, body: &str) -> SkillCard {
        SkillCard {
            name: name.into(),
            description: desc.into(),
            dir: PathBuf::from("/tmp/skills").join(name),
            body: body.into(),
        }
    }

    fn ctx() -> ToolContext {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let id = ToolUseId::from_raw("t1");
        ToolContext {
            session_id: SessionId::from_raw("s1"),
            tool_use_id: id.clone(),
            cwd: "/work".into(),
            artifacts_dir: "/artifacts".into(),
            cancel: CancellationToken::new(),
            progress: ProgressSink::new(id, tx),
            file_state: Arc::new(NullFileState),
            fs: Arc::new(NullFs),
            proc: Arc::new(NullProc),
            web: Arc::new(riot_protocol::web::NoWeb),
            browser: Arc::new(riot_protocol::browser::NoBrowser),
            terminal: Arc::new(riot_protocol::terminal::NoTerminal),
            vision: Arc::new(riot_protocol::vision::NoVision),
            clock: Arc::new(FixedClock::default()),
        }
    }

    fn prompt_ctx() -> PromptContext {
        PromptContext {
            cwd: PathBuf::from("/w"),
            platform: "test".into(),
            sibling_tools: Vec::new(),
            today: "2026年8月".into(),
        }
    }

    #[tokio::test]
    async fn 清单进_prompt_正文不进() {
        // 渐进披露的全部意义：十个技能全文几万 token，每轮真正需要的
        // 是零或一个。正文出现在 prompt 里就是每轮都付的税。
        let t = SkillTool::new(vec![card("发布", "发布新版本时用", "第一步：跑全部测试……")]);
        let p = t.prompt(&prompt_ctx());
        assert!(p.contains("发布"), "清单要有名字");
        assert!(p.contains("发布新版本时用"), "清单要有描述");
        assert!(!p.contains("第一步"), "正文绝不能进 prompt");
    }

    #[tokio::test]
    async fn 加载返回正文并带目录说明() {
        let t = SkillTool::new(vec![card("发布", "d", "按 checklist 走。")]);
        let out = t
            .call(serde_json::json!({ "name": "发布" }), ctx())
            .await;
        match out {
            ToolOutcome::Ok { model_content, .. } => {
                let text = format!("{model_content:?}");
                assert!(text.contains("按 checklist 走"));
                assert!(text.contains("/tmp/skills/发布"), "要告诉模型基准目录");
            }
            other => panic!("该成功：{other:?}"),
        }
    }

    #[tokio::test]
    async fn 参数替换进占位符() {
        let t = SkillTool::new(vec![card("查", "d", "查询目标：$ARGUMENTS，配置在 ${SKILL_DIR}/conf.json")]);
        let out = t
            .call(serde_json::json!({ "name": "查", "args": "example.com" }), ctx())
            .await;
        let ToolOutcome::Ok { model_content, .. } = out else {
            panic!("该成功")
        };
        let text = format!("{model_content:?}");
        assert!(text.contains("查询目标：example.com"));
        assert!(text.contains("/tmp/skills/查/conf.json"), "${{SKILL_DIR}} 要替换成真实目录");
    }

    /// 内置技能（编进二进制的那些）没有目录，这时不该做替换。
    ///
    /// 换成空串会把正文弄坏，而正文里出现这个 token 时最可能的情况是它在
    /// **讲解**这个占位符 —— 「扩展 Riot」那个内置技能就是。同理也不该再
    /// 输出「目录：」那一行，它会变成一个空路径。
    #[tokio::test]
    async fn 没有目录的技能不替换占位符() {
        let mut c = card("扩展", "d", "占位符写成 ${SKILL_DIR}，指技能自己的目录。");
        c.dir = std::path::PathBuf::new();
        let t = SkillTool::new(vec![c]);
        let out = t.call(serde_json::json!({ "name": "扩展" }), ctx()).await;
        let ToolOutcome::Ok { model_content, .. } = out else {
            panic!("该成功：{out:?}")
        };
        let text = format!("{model_content:?}");
        assert!(
            text.contains("${SKILL_DIR}"),
            "没有目录时该原样留着，而不是换成空串：{text}"
        );
        assert!(!text.contains("目录："), "没有目录就别输出那一行：{text}");
    }

    #[tokio::test]
    async fn 没提供参数时占位符不留空洞() {
        let t = SkillTool::new(vec![card("查", "d", "目标：$ARGUMENTS")]);
        let out = t.call(serde_json::json!({ "name": "查" }), ctx()).await;
        let ToolOutcome::Ok { model_content, .. } = out else {
            panic!("该成功")
        };
        assert!(
            format!("{model_content:?}").contains("没有提供参数"),
            "空串替换会留下一个悄无声息的洞"
        );
    }

    #[tokio::test]
    async fn 不存在的技能报名单() {
        let t = SkillTool::new(vec![card("a", "d", "x"), card("b", "d", "y")]);
        let out = t.call(serde_json::json!({ "name": "c" }), ctx()).await;
        match out {
            ToolOutcome::Failed { error_for_model, .. } => {
                assert!(error_for_model.contains('a') && error_for_model.contains('b'),
                    "报错要带可用名单，否则模型只会换个错名字再试：{error_for_model}");
            }
            other => panic!("该失败：{other:?}"),
        }
    }

    #[test]
    fn 超长描述在清单里被截断() {
        let long = "长".repeat(500);
        let t = SkillTool::new(vec![card("x", &long, "body")]);
        let p = t.prompt(&prompt_ctx());
        assert!(
            !p.contains(&"长".repeat(300)),
            "描述超过 250 字符要截断 —— 清单是每轮都付的上下文税"
        );
    }
}
