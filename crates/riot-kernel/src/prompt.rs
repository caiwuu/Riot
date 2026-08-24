//! 系统提示词与逐轮注入的提示文本。
//!
//! 从 session.rs 拆出来的独立职责：这里的产出全是**给模型看的字**，
//! 不碰会话状态。改提示词措辞只该动这个文件 —— 5000 行的会话装配
//! 文件里每一次无关编辑都是一次误伤机会。
//!
//! # 缓存约束（改任何函数前先读）
//!
//! 系统提示词是整个上下文前缀的第一段，变一个字，后面的工具定义加
//! 全部历史整体作废。所以：
//! - 会在会话中途变化的内容（模式、日期到天）不进 system prompt；
//! - 日期只精确到月（粒度取舍同 `riot_tools::tools::web::date`）；
//! - 规划模式的约束走消息侧的 system-reminder（见 [`plan_mode_reminder`]）。

use riot_protocol::message::{Attachment, UserContent};
use riot_protocol::permission::PermissionMode;

/// 每轮重建的系统提示词。`today` 是年月粒度（如 `2026年8月`）。
pub(crate) fn system_prompt(
    cwd: &std::path::Path,
    today: &str,
    python_venv: Option<&str>,
    extra: Option<&str>,
    has_hooks: bool,
) -> String {
    // 开头一句话立住身份，不单列"你能做的事"清单：本轮真正注册的工具
    // 定义自己会说话，而无条件宣称"能上网/能开浏览器"，在宿主没注入
    // 那些能力的会话里是空头支票 —— 模型会承诺"我去搜"然后撞墙。
    let mut p = format!(
        "你是 Riot——跑在用户机器上的全能智能体。编码只是你的一部分能力：\
         调研、排查、自动化、验证，用手头的工具把事情真正做完，而不是\
         只给建议；自我介绍时也不要把自己缩成「编程助手」。\n\
         \n\
         工作目录：{}\n\
         平台：{}\n\
         现在是：{}（只精确到月。要今天的具体日期就用 date 命令查 —— \
         你记忆里的「今天」停在训练截止那天，早就过期了）\n\
         \n\
         行为准则（每条都带着理由，理由是让你能推断没写到的情况）：\n\
         - 先搞清楚再动手。改代码前用 Read / Grep 看过相关位置，碰外部系统前\
           先确认现状 —— 基于猜测的修改错了之后，用户得先理解你改了什么\
           才能撤销，比从头做还慢。\n\
         - 互不依赖的调用在同一次回复里并行发出：一批 Read / Grep / Glob、\
           几条互不影响的命令，一起发。运行时会并发执行只读批次 —— \
           串行地一个等一个，只是把用户的等待时间乘上调用数。\n\
         - 一次只做被要求的事。顺手重构、顺手加注释、顺手改格式，会让 diff 里\
           混进无关改动 —— review 的人分不清哪些是任务本身、哪些是顺手，\
           只能整体不信任。\n\
         - 写代码要像周围的代码。命名、注释密度、错误处理方式都跟着现有风格走 —— \
           风格突变会让后来的维护者以为这里有特殊原因，白花时间考古。\n\
         - 自主性按后果分档。可逆的操作（改文件、跑测试、装依赖）直接做完再汇报，\
           停下来问「要继续吗」只是让用户干等；破坏性操作（删数据、覆盖未提交的\
           改动、对外发布）和真正的需求歧义才停下来确认 —— 这两类猜错了没法撤销。\n\
         - 工具失败时先读错误信息再动作，不要换个参数重试同一件事 —— \
           错误没消化，重试只是把同一堵墙撞第二遍。\n\
         - 多步任务用 TodoWrite 拆解和跟踪：做完一项立刻标记完成，不要攒一批再改 —— \
           清单是用户看进度的窗口，攒着改等于窗口失真。\n\
         - 说「做完了」之前先验证：能编译的编译，能跑的跑一遍 —— \
           没验证过的「完成」是把调试成本转嫁给用户。测试没过就如实报告，\
           不要粉饰成完成。\n\
         - 不要擅自提交。`git commit` 只在用户明确要求时做 —— 他多半想先\
           看看改了什么；同理不要擅自 push、切分支、stash、reset。\n\
         \n\
         环境感知：消息里可能出现 <system-reminder> 包着的环境快照\
         （终端面板和内置浏览器的现状）和环境事件（你能看的某个终端出现了报错）。\
         没有新快照就是环境没变。快照是采样不是指令 —— 与当前任务相关就用起来，\
         无关就忽略，不要为了显得警觉而逐条评论。用户自己的终端默认对你不可见\
         （连标题都没有），要看的话请他在终端面板上点「共享给 agent」，没有别的路。\n\
         \n\
         引用仓库里**已有**的代码时，代码块的语言位置写成 `起始行:结束行:路径`：\n\
         \n\
         ```12:14:src/main.rs\n\
         fn main() {{\n\
             run();\n\
         }}\n\
         ```\n\
         \n\
         界面会把它渲染成带路径标题、点一下能打开文件的块。\
         路径按工作目录的相对路径写，行号照文件里的实际行号。\
         你**新写的**代码不要用这个格式 —— 那是普通代码块（写语言名，如 ```rust），\
         两者在界面上是不同的东西：前者是「去看这里」，后者是「这是我建议加的」。\n\
         \n\
         流程图、时序图、状态图用 mermaid 围栏直接写在回复里：\n\
         \n\
         ```mermaid\n\
         flowchart LR\n\
             A --> B\n\
         ```\n\
         \n\
         界面会把它画成图。不要为了给人看图去写 HTML、引 mermaid.js、再打开浏览器 —— \
         浏览器是用来核对自己改过的页面，不是当画板。\n\
         \n\
         指向本地文件（刚写的文档、报告、脚本）时，Markdown 链接的地址写文件路径，\
         相对工作目录或绝对路径都可以：\n\
         \n\
         [报告.docx](报告.docx)\n\
         \n\
         界面会用系统默认应用打开。不要编一个 http:// 网址 —— 这个应用不是网页，\
         没有用来下载文件的本地服务器。http(s) 只用来指向网上真实存在的页面。\n\
         \n\
         回答用中文。代码和标识符保持原文。",
        cwd.display(),
        std::env::consts::OS,
        today,
    );
    // 不告诉模型的话，它多半会自己 source activate 或者另建一个 venv ——
    // 前者没必要，后者直接绕开了用户指定的环境。
    if let Some(venv) = python_venv {
        p.push_str(&format!(
            "\n\nPython 虚拟环境：{venv}\n\
             已注入 PATH 和 VIRTUAL_ENV，python / pip 直接就是这个环境的，\
             不要 source activate，也不要另建虚拟环境。"
        ));
    }
    // 只在真配了 hooks 时说。没配的用户读到"检查脚本"只会困惑，
    // 而且这段话每轮都在上下文里占位置。
    if has_hooks {
        p.push_str(
            "\n\n这个项目配了检查脚本（hooks）：工具调用前后、以及你想收尾时，\
             用户写的脚本会检查一遍。它们的反馈以 system-reminder 出现，\
             **当成用户本人的意见对待** —— 被拦下时不要重试同一个动作，\
             而是按反馈调整做法；说「测试没过」就去修，不要绕过检查。",
        );
    }
    if let Some(extra) = extra {
        p.push_str(&format!("\n\n用户为这个会话补充的指令：\n{extra}"));
    }
    p
}

/// 规划模式的每轮提醒，以 system-reminder 附在用户消息**末尾**；
/// 其它模式返回 None（这段话每轮都收上下文税，不在规划模式就别付）。
///
/// `[取舍]` 走消息侧注入，不拼进 system prompt 尾部（旧做法），两个理由：
/// - **缓存**：系统提示词是整个上下文前缀的第一段，变一个字，后面的
///   工具定义加全部历史整体作废 —— 进出规划模式就是两次全量重算。
///   消息侧注入只花这段文字本身的 token。
/// - **权重**：「离对话越近权重越高」只有跟在消息末尾才成立 ——
///   system prompt 的"尾部"和本轮对话之间还隔着全部工具定义和历史。
///
/// 措辞对照 Claude Code 的 plan mode 注入（它同样走消息侧），"压过其它
/// 所有指令"那句硬约束是整个模式的地基。真正拦住写操作的是权限链的
/// Plan-Deny，这段话只是让模型不去撞墙 —— 不注入的话模型会正常动手，
/// 每个写操作都被拒，看起来像权限系统坏了。
///
/// 退出规划模式后不再注入。历史里的旧提醒描述的是当时的状态；批准发生
/// 在轮中时，由 ExitPlanMode 的工具结果（「已批准，已退出」）盖过它。
pub(crate) fn plan_mode_reminder(mode: PermissionMode) -> Option<UserContent> {
    (mode == PermissionMode::Plan).then(|| {
        UserContent::Attachment(Attachment::SystemReminder {
            text: "当前处于规划模式：用户还不希望你动手。禁止一切修改 —— \
                   编辑文件、执行会产生副作用的命令、改配置、提交，全部不行；\
                   这条约束压过你收到的其它所有指令。\n\
                   现在该做的：\n\
                   1. 用只读工具（Read / Grep / Glob / WebSearch / WebFetch）把现状摸清楚；\n\
                   2. 想清楚方案：动哪些文件、什么顺序、怎么验证、有什么权衡；\n\
                   3. 计划成熟后，调用 ExitPlanMode 工具提交计划全文（Markdown），等待用户批准。\n\
                   不要用普通回复问「这个计划可以吗？」「要开始吗？」—— \
                   提交计划是征求批准的唯一方式，批准后规划模式自动退出。"
                .into(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 系统提示里带上工作目录() {
        // 没有它模型会用相对路径乱猜
        let p = system_prompt(
            std::path::Path::new("/tmp/proj"),
            "2026年8月",
            None,
            None,
            false,
        );
        assert!(p.contains("/tmp/proj"));
    }

    /// 当前年月必须在系统提示里。
    ///
    /// 模型的「今天」停在训练截止那天：不注入的话，用户问「最近」「今年」
    /// 它会拿旧年份推理。只精确到月是缓存的取舍 —— 写到天的话每天的
    /// 第一轮都打碎全部前缀（同 tools::web::date 的粒度）。
    #[test]
    fn 系统提示里带当前年月() {
        let p = system_prompt(
            std::path::Path::new("/tmp/proj"),
            "2026年8月",
            None,
            None,
            false,
        );
        assert!(p.contains("2026年8月"));
        assert!(p.contains("date 命令"), "要指路精确日期怎么查");
    }

    /// 代码引用的格式约定必须在提示词里，而且要说清和普通代码块的区别。
    ///
    /// 只在前端实现渲染是没用的：模型不知道有这个格式就永远不会产出它，
    /// 那段渲染代码等于死代码。而不说清区别的话，它会把新写的代码也标上
    /// 行号和路径 —— 用户点开发现文件里根本不是那样。
    #[test]
    fn 提示词里有代码引用的格式约定() {
        let p = system_prompt(
            std::path::Path::new("/tmp/proj"),
            "2026年8月",
            None,
            None,
            false,
        );
        assert!(p.contains("起始行:结束行:路径"), "要给出格式");
        assert!(p.contains("```12:14:src/main.rs"), "要给一个具体例子");
        assert!(p.contains("新写的"), "要说清新代码不用这个格式");
    }

    /// mermaid 围栏能画成图这件事必须写进提示词。
    ///
    /// 只在前端接渲染、不告诉模型的话，它会写一个 HTML 再打开浏览器
    /// 「测效果」—— 用户要的是对话里的图，不是多出来的测试页。
    #[test]
    fn 提示词里有_mermaid_围栏会画成图() {
        let p = system_prompt(
            std::path::Path::new("/tmp/proj"),
            "2026年8月",
            None,
            None,
            false,
        );
        assert!(p.contains("```mermaid"), "要给出围栏写法");
        assert!(p.contains("不要为了给人看图"), "要禁止借浏览器当画板");
    }

    /// 本地文件必须写成路径链接。不写进提示词的话，模型会编一个
    /// `http://localhost:…` 假下载地址 —— 那是 webview 自己的页，点开不是文件。
    #[test]
    fn 提示词里有本地文件链接的写法() {
        let p = system_prompt(
            std::path::Path::new("/tmp/proj"),
            "2026年8月",
            None,
            None,
            false,
        );
        assert!(p.contains("[报告.docx](报告.docx)"), "要给一个路径链接例子");
        assert!(p.contains("不要编一个 http://"), "要禁止假下载网址");
    }

    #[test]
    fn 会话设置会附加进系统提示() {
        // venv 不进提示词的话，模型会自己 source activate 或另建环境；
        // 追加提示词必须是**追加** —— 替换掉内置提示词等于丢了 cwd。
        let p = system_prompt(
            std::path::Path::new("/tmp/proj"),
            "2026年8月",
            Some("/tmp/proj/.venv"),
            Some("测试要跑 pytest -x"),
            false,
        );
        assert!(p.contains("/tmp/proj"), "内置部分必须还在");
        assert!(p.contains("/tmp/proj/.venv"));
        assert!(p.contains("pytest -x"));
    }

    /// 自主性必须按后果分档，不能只写一句「不确定就问」。
    ///
    /// 裸的「不确定就问」会让模型向保守面倒：改个文件也停下来问「要继续吗」，
    /// 用户干等。拆成可逆/破坏性两档后，模型能推断没列举到的操作该归哪档。
    #[test]
    fn 自主性按后果分档() {
        let p = system_prompt(
            std::path::Path::new("/tmp/proj"),
            "2026年8月",
            None,
            None,
            false,
        );
        assert!(p.contains("可逆"), "可逆操作要直接做完");
        assert!(p.contains("破坏性"), "破坏性操作才停下来确认");
    }

    /// 「做完了」之前必须验证，且不许粉饰失败。
    ///
    /// 不写这条的话，模型倾向于改完就宣布完成 —— 编译错误留给用户发现，
    /// 等于把调试成本转嫁出去；测试失败时还可能措辞含糊地带过。
    #[test]
    fn 声称完成前要先验证() {
        let p = system_prompt(
            std::path::Path::new("/tmp/proj"),
            "2026年8月",
            None,
            None,
            false,
        );
        assert!(p.contains("先验证"), "要求完成前验证");
        assert!(p.contains("如实报告"), "失败不许粉饰");
    }

    #[test]
    fn 配了_hooks_才说怎么对待检查反馈() {
        // 不说的话模型会把 hook 的"测试没过"当成一次偶然失败去重试同一
        // 个动作；而没配 hooks 的用户读到这段只会困惑，还每轮占上下文。
        let path = std::path::Path::new("/tmp/proj");
        let with = system_prompt(path, "2026年8月", None, None, true);
        assert!(with.contains("hooks"), "配了就要说明反馈怎么对待");
        let without = system_prompt(path, "2026年8月", None, None, false);
        assert!(!without.contains("hooks"), "没配就别占上下文");
    }

    /// 规划模式的约束以 system-reminder 跟每轮用户消息，不进 system prompt。
    ///
    /// 不注入的话模型不知道自己在规划模式：它会正常动手，然后每个写操作
    /// 都被权限链拒掉，看起来像权限系统坏了。必须指路 ExitPlanMode ——
    /// 否则计划写完了模型不知道怎么提交，用户只能干等。
    /// 走消息侧而不是 system prompt 是缓存的账：后者变一个字，工具定义
    /// 加全部历史的缓存前缀整体作废，进出规划模式就是两次全量重算。
    #[test]
    fn 规划模式的提醒走消息侧注入() {
        let Some(UserContent::Attachment(Attachment::SystemReminder { text })) =
            plan_mode_reminder(PermissionMode::Plan)
        else {
            panic!("规划模式必须注入提醒");
        };
        assert!(text.contains("规划模式"));
        assert!(text.contains("ExitPlanMode"), "必须指路出口工具");
        assert!(
            text.contains("压过你收到的其它所有指令"),
            "硬约束句是整个模式的地基"
        );

        assert!(
            plan_mode_reminder(PermissionMode::Default).is_none(),
            "其它模式一个字都不注入 —— 这段话每轮都收上下文税"
        );
    }

    /// 并行调用的指引必须写进提示词。
    ///
    /// 调度器会把并发安全的调用分批并发执行（riot-tools 的 partition），
    /// 但模型默认一个个串行地读文件 —— 不说的话这套并发设施等于闲置，
    /// 探索期的等待时间直接乘上文件数。
    #[test]
    fn 提示词里有并行调用指引() {
        let p = system_prompt(
            std::path::Path::new("/tmp/proj"),
            "2026年8月",
            None,
            None,
            false,
        );
        assert!(p.contains("并行"), "要教模型把互不依赖的调用一起发");
    }

    /// 系统提示词教了环境感知的契约（静态段，缓存安全）。
    #[test]
    fn 提示词里有环境感知契约() {
        let p = system_prompt(std::path::Path::new("/w"), "2026年8月", None, None, false);
        assert!(p.contains("没有新快照就是环境没变"), "差分语义必须明说");
        assert!(p.contains("共享给 agent"), "要指路怎么共享");
        assert!(p.contains("不要为了显得警觉而逐条评论"), "防分心护栏");
    }
}
