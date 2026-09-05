//! 工具名常量。
//!
//! 工具描述里要指名道姓地写分工（"按内容搜用 Grep"、"改一部分用 Edit"）。
//! 把名字写成字面量的话，改名时描述不会跟着改，模型会照着一个不存在的
//! 名字调用 —— `registry` 的「描述里提到的工具都真的存在」测试只挡得住
//! 封闭词表里的那几个，挡不住 `Browser*` 这类拼写漂移。
//!
//! 所以 [`Tool::name`](riot_protocol::tool::Tool::name) 和 `prompt()` 里的
//! 引用取同一个常量。Rust 2021 的内联格式参数让引用点几乎没有成本：
//! `use super::names::GREP;` 之后 `format!("... use {GREP} ...")` 即可。

// ── 文件与检索 ────────────────────────────────────────────
pub const READ: &str = "Read";
pub const WRITE: &str = "Write";
pub const EDIT: &str = "Edit";
pub const GREP: &str = "Grep";
pub const GLOB: &str = "Glob";

// ── 执行与终端 ────────────────────────────────────────────
pub const BASH: &str = "Bash";
pub const TERMINAL_OUTPUT: &str = "TerminalOutput";
pub const TERMINAL_KILL: &str = "TerminalKill";
pub const TERMINAL_LIST: &str = "TerminalList";

// ── 联网 ──────────────────────────────────────────────────
pub const WEB_SEARCH: &str = "WebSearch";
pub const WEB_FETCH: &str = "WebFetch";

// ── 会话与展示 ────────────────────────────────────────────
pub const TODO_WRITE: &str = "TodoWrite";
pub const ASK_USER_QUESTION: &str = "AskUserQuestion";
pub const DIAGNOSTICS: &str = "Diagnostics";
pub const PREVIEW_FILE: &str = "PreviewFile";
pub const SHOW_BROWSER: &str = "ShowBrowser";
pub const EXIT_PLAN_MODE: &str = "ExitPlanMode";
pub const TOOL_SEARCH: &str = "ToolSearch";

// ── 浏览器 ────────────────────────────────────────────────
//
// 共同前缀由 [`BROWSER_PREFIX`] 持有 —— 延迟加载的分组索引按它匹配。
pub const BROWSER_PREFIX: &str = "Browser";

pub const BROWSER_NAVIGATE: &str = "BrowserNavigate";
pub const BROWSER_SNAPSHOT: &str = "BrowserSnapshot";
pub const BROWSER_SCREENSHOT: &str = "BrowserScreenshot";
pub const BROWSER_VIEW: &str = "BrowserView";
pub const BROWSER_CONSOLE: &str = "BrowserConsole";
pub const BROWSER_PERF: &str = "BrowserPerf";
pub const BROWSER_SOURCE_OF: &str = "BrowserSourceOf";
pub const BROWSER_READ_TAB: &str = "BrowserReadTab";
pub const BROWSER_HAR: &str = "BrowserHar";
pub const BROWSER_CLICK: &str = "BrowserClick";
pub const BROWSER_TYPE: &str = "BrowserType";
pub const BROWSER_FILL_FORM: &str = "BrowserFillForm";
pub const BROWSER_KEY: &str = "BrowserKey";
pub const BROWSER_SCROLL: &str = "BrowserScroll";
pub const BROWSER_WAIT_FOR: &str = "BrowserWaitFor";
pub const BROWSER_HOVER: &str = "BrowserHover";
pub const BROWSER_SELECT: &str = "BrowserSelect";
pub const BROWSER_DRAG: &str = "BrowserDrag";
pub const BROWSER_GO: &str = "BrowserGo";
pub const BROWSER_TABS: &str = "BrowserTabs";
pub const BROWSER_EVALUATE: &str = "BrowserEvaluate";
pub const BROWSER_UPLOAD: &str = "BrowserUpload";
pub const BROWSER_COOKIES: &str = "BrowserCookies";
pub const BROWSER_NETWORK: &str = "BrowserNetwork";
pub const BROWSER_REPLAY: &str = "BrowserReplay";
pub const BROWSER_INTERCEPT: &str = "BrowserIntercept";
pub const BROWSER_SECRETS: &str = "BrowserSecrets";
pub const BROWSER_DISCOVER: &str = "BrowserDiscover";
pub const BROWSER_FUZZ: &str = "BrowserFuzz";
pub const BROWSER_CRAWL: &str = "BrowserCrawl";
pub const BROWSER_REPORT: &str = "BrowserReport";
pub const BROWSER_HANDOFF: &str = "BrowserHandoff";

/// 本会话到底装了哪些工具。
///
/// 分工声明（"按内容搜用 Grep"）只有在那个工具真的在场时才成立。子 agent
/// 拿的是裁剪过的工具集，对着一个缺席的工具指路，模型会照着调、失败、再
/// 换参数重试 —— `registry` 的「描述里提到的工具都真的存在」测试查的是
/// 内建全集，挡不住这种**按会话变化**的缺席。
///
/// `[约束]` `sibling_tools` 为空一律视为"未知，按在场处理"。空是测试和
/// 早期装配路径的默认值，把它当成"什么都没有"会让整段分工声明凭空消失，
/// 那比多说一句更糟。
pub struct Siblings<'a>(&'a [String]);

impl<'a> Siblings<'a> {
    pub fn of(ctx: &'a riot_protocol::tool::PromptContext) -> Self {
        Self(&ctx.sibling_tools)
    }

    pub fn has(&self, name: &str) -> bool {
        self.0.is_empty() || self.0.iter().any(|n| n == name)
    }

    /// `name` 在场就给出 `text`，缺席给空串。
    ///
    /// 专门用于「禁止一个行为的同时给出替代行为」那类句子 —— 替代工具
    /// 不在场时，整句话（连同那条禁令）都不该出现，否则模型只收到禁令，
    /// 剩下的它自己发挥。
    pub fn line<S: Into<String>>(&self, name: &str, text: S) -> String {
        if self.has(name) {
            text.into()
        } else {
            String::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 常量和 `Tool::name()` 不能各说各话。
    ///
    /// 这个测试是整套「工具名做成变量」约定的兜底：谁要是把某个
    /// `fn name()` 改回字面量、或者改了字面量忘了改常量，这里当场红。
    #[test]
    fn 常量与内建工具的_name_一致() {
        let registered: Vec<String> = crate::tools::builtin()
            .iter()
            .map(|t| t.name().to_owned())
            .collect();

        for want in [
            READ,
            WRITE,
            EDIT,
            GREP,
            GLOB,
            BASH,
            WEB_SEARCH,
            WEB_FETCH,
            TODO_WRITE,
            TERMINAL_OUTPUT,
            TERMINAL_KILL,
            TERMINAL_LIST,
            ASK_USER_QUESTION,
            DIAGNOSTICS,
            PREVIEW_FILE,
            SHOW_BROWSER,
            BROWSER_NAVIGATE,
            BROWSER_SNAPSHOT,
            BROWSER_SCREENSHOT,
            BROWSER_VIEW,
            BROWSER_CONSOLE,
            BROWSER_PERF,
            BROWSER_SOURCE_OF,
            BROWSER_READ_TAB,
            BROWSER_HAR,
            BROWSER_CLICK,
            BROWSER_TYPE,
            BROWSER_FILL_FORM,
            BROWSER_KEY,
            BROWSER_SCROLL,
            BROWSER_WAIT_FOR,
            BROWSER_HOVER,
            BROWSER_SELECT,
            BROWSER_DRAG,
            BROWSER_GO,
            BROWSER_TABS,
            BROWSER_EVALUATE,
            BROWSER_UPLOAD,
            BROWSER_COOKIES,
            BROWSER_NETWORK,
            BROWSER_REPLAY,
            BROWSER_INTERCEPT,
            BROWSER_SECRETS,
            BROWSER_DISCOVER,
            BROWSER_FUZZ,
            BROWSER_CRAWL,
            BROWSER_REPORT,
            BROWSER_HANDOFF,
        ] {
            assert!(
                registered.iter().any(|n| n == want),
                "常量 `{want}` 没有对应的已注册工具 —— 名字漂移了"
            );
        }
    }

    /// 主循环的待办提醒按名字认 TodoWrite（riot-core 不能依赖这里，
    /// 只能自己写一份）。两边漂移的话提醒永远不触发，而且没人会发现。
    #[test]
    fn 与主循环里的_todo_write_名字一致() {
        assert_eq!(TODO_WRITE, riot_core::todo_nudge::TODO_WRITE);
    }

    fn ctx(siblings: &[&str]) -> riot_protocol::tool::PromptContext {
        riot_protocol::tool::PromptContext {
            cwd: "/work".into(),
            platform: "macos".to_owned(),
            sandboxed: false,
            sibling_tools: siblings.iter().map(|s| (*s).to_owned()).collect(),
            today: "2026年9月".to_owned(),
        }
    }

    #[test]
    fn 空的_siblings_按全在场处理() {
        // 空是测试和早期装配路径的默认值。当成"什么都没有"的话，整段
        // 分工声明会凭空消失，那比多说一句更糟。
        let c = ctx(&[]);
        let s = Siblings::of(&c);
        assert!(s.has(GREP));
        assert_eq!(s.line(GREP, "search with Grep"), "search with Grep");
    }

    #[test]
    fn 缺席的工具不进分工声明() {
        // 子 agent 拿的是裁剪过的工具集。对着缺席的工具指路，模型会
        // 照着调、失败、再换参数重试。
        let c = ctx(&[READ, GREP]);
        let s = Siblings::of(&c);
        assert!(s.has(GREP));
        assert!(!s.has(BASH));
        assert_eq!(s.line(BASH, "run it with Bash"), "");
    }

    #[test]
    fn 浏览器常量都带统一前缀() {
        for name in [BROWSER_NAVIGATE, BROWSER_CLICK, BROWSER_HANDOFF] {
            assert!(
                name.starts_with(BROWSER_PREFIX),
                "{name} 不带 {BROWSER_PREFIX} 前缀，延迟加载的分组索引会漏掉它"
            );
        }
    }
}
