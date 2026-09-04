//! 工具注册表。
//!
//! 职责比听起来多一点：除了按名字查工具，它还负责**在启动时就把配置错误
//! 暴露出来**。重名、别名撞车这类问题如果拖到运行时才发现，表现是
//! "某个工具偶尔调用到另一个实现"，那种 bug 查起来很痛苦。
//!
//! 所以 [`Registry::new`] 返回 `Result`，构造失败就起不来。

use std::collections::HashMap;
use std::sync::Arc;

use riot_protocol::provider::ToolSpec;
use riot_protocol::tool::{PromptContext, Tool};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    #[error("工具名 `{name}` 重复注册")]
    DuplicateName { name: String },

    #[error("别名 `{alias}`（来自 `{tool}`）与已有的 `{conflicts_with}` 冲突")]
    AliasConflict {
        alias: String,
        tool: String,
        conflicts_with: String,
    },
}

pub struct Registry {
    tools: Vec<Arc<dyn Tool>>,
    /// 名字 → 下标。别名也在里面。
    index: HashMap<String, usize>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field(
                "tools",
                &self.tools.iter().map(|t| t.name()).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl Registry {
    /// 建注册表。重名或别名冲突直接失败。
    pub fn new(tools: Vec<Arc<dyn Tool>>) -> Result<Self, RegistryError> {
        let mut index: HashMap<String, usize> = HashMap::new();

        // 两趟：先占正式名，再占别名。
        //
        // 顺序很重要 —— 反过来的话，一个工具的别名可能先占住另一个工具的
        // 正式名，于是正式名注册时报"重复"，而真正的问题是别名起错了。
        // 报错信息指错方向比不报错还糟。
        for (i, t) in tools.iter().enumerate() {
            if index.insert(t.name().to_owned(), i).is_some() {
                return Err(RegistryError::DuplicateName {
                    name: t.name().to_owned(),
                });
            }
        }

        for (i, t) in tools.iter().enumerate() {
            for alias in t.aliases() {
                if let Some(&existing) = index.get(*alias) {
                    return Err(RegistryError::AliasConflict {
                        alias: (*alias).to_owned(),
                        tool: t.name().to_owned(),
                        conflicts_with: tools[existing].name().to_owned(),
                    });
                }
                index.insert((*alias).to_owned(), i);
            }
        }

        Ok(Self { tools, index })
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.index.get(name).map(|&i| &self.tools[i])
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn Tool>> {
        self.tools.iter()
    }

    /// 生成进 API 请求的工具声明。
    ///
    /// `[约束]` 顺序必须稳定。工具块是 prompt cache 的一部分，顺序抖一下
    /// 整块缓存就失效 —— 而抖动在 HashMap 迭代下是随机发生的，表现为
    /// "缓存命中率时高时低"，没人能定位。
    ///
    /// 这里按注册顺序输出（`tools` 是 Vec，不是 HashMap），
    /// provider 那边还会再按名字排一次序。两层都保证，因为这个
    /// 属性的破坏是静默的。
    pub fn specs(&self, ctx: &PromptContext) -> Vec<ToolSpec> {
        self.tools
            .iter()
            .map(|t| ToolSpec {
                name: t.name().to_owned(),
                description: t.prompt(ctx),
                input_schema: serde_json::to_value(t.input_schema())
                    .unwrap_or_else(|_| serde_json::json!({ "type": "object" })),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeTool;
    use pretty_assertions::assert_eq;

    fn reg(tools: Vec<Arc<dyn Tool>>) -> Registry {
        Registry::new(tools).expect("注册表构造成功")
    }

    /// 浏览器工具要真的出现在发给模型的清单里。
    ///
    /// `[约束]` 少了它们，模型不是报错，而是**自己想办法**:去 shell 里
    /// `screencapture` 截整个屏幕、用 osascript 找窗口，然后拿着一张截错的
    /// 图言之凿凿。那种失败看起来像模型笨，而根因是工具没装上 —— 真实发生
    /// 过一次，排查方向整个跑偏。
    #[test]
    fn 浏览器工具在发给模型的清单里() {
        let r = Registry::new(crate::tools::builtin()).expect("内建工具集");
        let ctx = PromptContext {
            cwd: std::path::PathBuf::from("/work"),
            platform: "macos".to_owned(),
            sandboxed: false,
            sibling_tools: Vec::new(),
            today: "2026-08".to_owned(),
        };
        let names: Vec<String> = r.specs(&ctx).into_iter().map(|s| s.name).collect();
        for want in [
            "BrowserNavigate",
            "BrowserSnapshot",
            "BrowserScreenshot",
            "BrowserConsole",
        ] {
            assert!(
                names.contains(&want.to_owned()),
                "{want} 不在清单里：{names:?}"
            );
        }
    }

    #[test]
    fn 内建工具集能构造() {
        // Registry::new 会拒绝重名和别名撞车。这个测试的意义是：
        // 以后往 builtin() 里加工具时，撞名会在这里当场红，而不是等到
        // 进程启动才 panic。
        let r = Registry::new(crate::tools::builtin()).expect("内建工具集不能有冲突");

        for name in ["Read", "Write", "Edit"] {
            assert_eq!(r.get(name).expect("已注册").name(), name);
        }
    }

    #[test]
    fn 工具描述里提到的工具都真的存在() {
        // 真实事故：Bash 的描述写着"查找文件用 Glob"、Grep 的报错写着
        // "要列出文件请用 Glob"，而 Glob 根本没注册。模型照做，然后对着
        // "没有名为 Glob 的工具"换着参数重试了五次。
        //
        // 描述里指向一个不存在的工具不会有任何编译期或启动期报错，
        // 只会在用户面前变成一串失败的调用。
        let tools = crate::tools::builtin();
        let r = Registry::new(tools.clone()).expect("内建工具集不能有冲突");

        let ctx = riot_protocol::tool::PromptContext {
            cwd: std::path::PathBuf::from("/work"),
            platform: "linux".to_owned(),
            sandboxed: false,
            sibling_tools: Vec::new(),
            today: "2026年8月".to_owned(),
        };

        // 只在这个封闭词表里找。扫"所有大写开头的词"会把 Rust、OpenAI
        // 这类普通名词也算进来。这里列的是我们自己的工具名，加上几个
        // 模型最常凭空捏造的（它们出现在描述里同样是 bug）。
        const TOOL_WORDS: &[&str] = &[
            "Read",
            "Write",
            "Edit",
            "Bash",
            "Grep",
            "Glob",
            "LS",
            "Task",
            "WebFetch",
            "NotebookEdit",
        ];

        // 按词边界比对。`LS`、`Task` 这类短词做子串匹配会被 `FAILS`、
        // `Tasks` 之类的普通英文词误命中。
        fn mentions(prompt: &str, word: &str) -> bool {
            prompt
                .match_indices(word)
                .any(|(at, _)| {
                    let before = prompt[..at].chars().next_back();
                    let after = prompt[at + word.len()..].chars().next();
                    let boundary = |c: Option<char>| {
                        c.is_none_or(|c| !c.is_alphanumeric() && c != '_')
                    };
                    boundary(before) && boundary(after)
                })
        }

        for t in &tools {
            let prompt = t.prompt(&ctx);
            for word in TOOL_WORDS {
                assert!(
                    !mentions(&prompt, word) || r.get(word).is_some(),
                    "{} 的描述里让模型用 `{word}`，但它没有注册",
                    t.name()
                );
            }
        }
    }

    #[test]
    fn 按名字查找() {
        let r = reg(vec![
            Arc::new(FakeTool::read_only("Read")),
            Arc::new(FakeTool::read_only("Grep")),
        ]);
        assert_eq!(r.get("Read").expect("有 Read").name(), "Read");
        assert!(r.get("Nope").is_none());
    }

    #[test]
    fn 别名能解析到同一个工具() {
        // 旧 transcript 里的名字还要能被解析，否则历史会话打不开
        let t = FakeTool::read_only("Grep").with_aliases(&["Search", "RipGrep"]);
        let r = reg(vec![Arc::new(t)]);

        assert_eq!(r.get("Search").expect("别名可解析").name(), "Grep");
        assert_eq!(r.get("RipGrep").expect("别名可解析").name(), "Grep");
    }

    #[test]
    fn 重名在构造时就失败() {
        // 拖到运行时才发现的话，表现是"某个工具偶尔调到另一个实现"
        let err = Registry::new(vec![
            Arc::new(FakeTool::read_only("Read")),
            Arc::new(FakeTool::read_only("Read")),
        ])
        .expect_err("应该拒绝");

        assert_eq!(
            err,
            RegistryError::DuplicateName {
                name: "Read".into()
            }
        );
    }

    #[test]
    fn 别名撞上正式名时报错指向别名() {
        // 报错必须指出是别名起错了，而不是说"Read 重复注册"——
        // 后者会让人去查 Read 的注册点，那里根本没问题。
        let err = Registry::new(vec![
            Arc::new(FakeTool::read_only("Read")),
            Arc::new(FakeTool::read_only("Grep").with_aliases(&["Read"])),
        ])
        .expect_err("应该拒绝");

        assert_eq!(
            err,
            RegistryError::AliasConflict {
                alias: "Read".into(),
                tool: "Grep".into(),
                conflicts_with: "Read".into(),
            }
        );
    }

    #[test]
    fn specs_按注册顺序输出() {
        let r = reg(vec![
            Arc::new(FakeTool::read_only("Write")),
            Arc::new(FakeTool::read_only("Bash")),
            Arc::new(FakeTool::read_only("Read")),
        ]);
        let ctx = PromptContext {
            cwd: "/tmp".into(),
            platform: "test".into(),
            sandboxed: false,
            sibling_tools: vec![],
            today: "2026年8月".into(),
        };

        let names: Vec<String> = r.specs(&ctx).into_iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            vec!["Write", "Bash", "Read"],
            "顺序必须稳定 —— 工具块是 prompt cache 的一部分"
        );

        // 跑两次结果一致（HashMap 迭代顺序不参与）
        let again: Vec<String> = r.specs(&ctx).into_iter().map(|s| s.name).collect();
        assert_eq!(names, again);
    }

    #[test]
    fn 别名不进_specs() {
        // 别名只用于解析历史 transcript。声明给模型会让它以为有两个工具，
        // 然后随机挑一个用。
        let r = reg(vec![Arc::new(
            FakeTool::read_only("Grep").with_aliases(&["Search"]),
        )]);
        let ctx = PromptContext {
            cwd: "/tmp".into(),
            platform: "test".into(),
            sandboxed: false,
            sibling_tools: vec![],
            today: "2026年8月".into(),
        };

        assert_eq!(r.specs(&ctx).len(), 1);
    }
}
