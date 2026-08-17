#!/usr/bin/env python3
"""变异测试。

往实现里注入真实可能被写出来的 bug，检查测试套件能不能抓住。
每个变异都是"看起来完全合理、code review 大概率放过"的那种写法。

抓不住的变异 = 测试覆盖的缺口。

用法：
    python3 scripts/mutate.py                  跑全部
    python3 scripts/mutate.py permissions      只跑某一层
    python3 scripts/mutate.py --check-anchors  只检查锚点还在不在
"""

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# 变异分层。key 是层名，value 是 (cargo 包名, 源码目录, 额外的 cargo 参数)
LAYERS = {
    "permissions": ("riot-permissions", ROOT / "crates/riot-permissions/src", []),
    "tools": ("riot-tools", ROOT / "crates/riot-tools/src", []),
    "runtime": ("riot-runtime", ROOT / "crates/riot-runtime/src", []),
    # 宿主层只跑 --lib。浏览器 e2e 要起 CEF 子进程，几十秒起步，而且几十个
    # 用例抢一个子进程时本身就会偶发超时 —— 一次偶发失败会被记成"变异被
    # 抓住"，那正是这个脚本最该避免的假绿。
    "gate": ("riot-host", ROOT / "src-tauri/src", ["--lib"]),
}

# (名字, 层, 文件, 原文, 替换, 这个 bug 会导致什么)
MUTANTS = [
    # ── 决策链 ────────────────────────────────────────
    (
        "bypass 排在安全检查前面",
        "permissions",
        "chain.rs",
        """    // ── 4. 内容级 ask 规则 + 安全检查 ─────────────────
    // 这一步对 bypass 免疫。""",
        """    if ctx.mode.get() == PermissionMode::BypassPermissions {
        return PermissionResult::Allow {
            updated_input: None,
            reason: DecisionReason::Mode { mode: PermissionMode::BypassPermissions },
        };
    }

    // ── 4. 内容级 ask 规则 + 安全检查 ─────────────────""",
        "bypass 模式下能改 .zshrc / .git/hooks —— agent 取得机器的持久化执行权",
    ),
    (
        "工具的 ask 一律就地短路",
        "permissions",
        "chain.rs",
        """        PermissionResult::Ask { reason, .. } if reason.yields_to_bypass() => {
            deferred_consent = Some(tool_says.clone());
        }""",
        """        PermissionResult::Ask { reason, .. } if reason.yields_to_bypass() => {
            return coerce_ask(tool_says, ctx);
        }""",
        "第 3 步吃掉后面四步 —— 开了「全部放行」还是每个域名都弹框（真实发生过）",
    ),
    # ── 权限闸：Auto 模式的判危 ────────────────────────
    (
        "分类器压过安全检查",
        "gate",
        "session.rs",
        """        if !reason.yields_to_bypass() {
            return None;
        }""",
        """        if false {
            return None;
        }""",
        "Auto 模式下小模型能自动放行写 SSH 密钥 / shell 启动脚本 —— 判危器成了绕过分层免疫的后门",
    ),
    (
        "判不准当成安全",
        "gate",
        "classifier.rs",
        """    if up.contains("UNSAFE") {
        return SafetyVerdict::Hold;
    }""",
        """    if up.contains("UNSAFE") && false {
        return SafetyVerdict::Hold;
    }""",
        '"UNSAFE" 里包含 "SAFE" —— 判定顺序一反，每一次拒绝都被静默读成放行',
    ),
    (
        "同意请求交给只读兜底",
        "permissions",
        "chain.rs",
        """    if let Some(ask) = deferred_consent {
        return coerce_ask(ask, ctx);
    }""",
        """    let _ = &deferred_consent;""",
        "WebFetch 的 is_read_only 是 true，mode_default 会在所有模式下静默放行全部抓取",
    ),
    (
        "无人应答时默认放行",
        "permissions",
        "chain.rs",
        """    if mode == PermissionMode::DontAsk || !ctx.can_prompt_user {
        return PermissionResult::Deny {
            message: format!("{message}（无法询问，已拒绝）"),
            reason,
        };
    }""",
        """    if mode == PermissionMode::DontAsk || !ctx.can_prompt_user {
        return PermissionResult::Allow {
            updated_input: None,
            reason,
        };
    }""",
        "无人值守场景成为绕过所有权限的后门",
    ),
    (
        "fd 复制只看操作符",
        "permissions",
        "bash/ast.rs",
        """    if is_fd_dup {
        return t.kind() == "number";
    }""",
        """    if is_fd_dup {
        return true;
    }""",
        "`ls >&out.txt` 在 bash 里是写文件，会被当成 `2>&1` 放行 —— 可写任意路径",
    ),
    (
        "/dev/null 用前缀匹配",
        "permissions",
        "bash/ast.rs",
        '''    t.utf8_text(src).is_ok_and(|s| s == "/dev/null")''',
        '''    t.utf8_text(src).is_ok_and(|s| s.starts_with("/dev/null"))''',
        "`> /dev/null.bak` 会被当成丢弃输出放行",
    ),
    (
        "通配符用朴素 glob",
        "permissions",
        "rules.rs",
        "    let crosses_meta = |s: &str| mode == MatchMode::Raw && s.contains(SHELL_META);",
        "    let crosses_meta = |s: &str| { let _ = s; false };",
        "`Bash(npm run *)` 会放行 `npm run test && rm -rf /`",
    ),
    (
        "规则按来源优先级取第一条",
        "permissions",
        "rules.rs",
        """    for want in [RuleDecision::Deny, RuleDecision::Ask, RuleDecision::Allow] {""",
        """    if let Some(r) = rules.iter().find(|r| r.tool == tool && r.pattern.is_none()) {
        return Some(MatchedRule {
            decision: r.decision,
            source: r.source,
            pattern: tool.to_owned(),
            content_level: false,
        });
    }
    for want in [RuleDecision::Deny, RuleDecision::Ask, RuleDecision::Allow] {""",
        "组织策略的 allow 会压过用户自己配的 deny —— 用户无法收紧自己的环境",
    ),
    (
        "敏感路径用子串匹配",
        "permissions",
        "safety.rs",
        "    path.split('/').any(|s| s == segment)",
        "    path.contains(segment)",
        "src/legit.git-helper.rs 被误判 —— 弹窗泛滥后用户不看内容直接点允许",
    ),
    (
        "只读工具跳过凭证检查",
        "permissions",
        "safety.rs",
        """    if looks_like_credentials(&normalized) {
        return Some(SafetyKind::Credentials);
    }
    if contains_segment(&normalized, ".ssh") {
        return Some(SafetyKind::SshConfig);
    }

    // 以下几类是"获得执行权"，只有写才危险
    if read_only {
        return None;
    }""",
        """    // 以下几类是"获得执行权"，只有写才危险
    if read_only {
        return None;
    }

    if looks_like_credentials(&normalized) {
        return Some(SafetyKind::Credentials);
    }
    if contains_segment(&normalized, ".ssh") {
        return Some(SafetyKind::SshConfig);
    }""",
        "agent 能静默读走 .env / SSH 私钥 / AWS 凭证",
    ),
    (
        "工具说 allow 就直接放行",
        "permissions",
        "chain.rs",
        """        PermissionResult::Ask { .. } => return coerce_ask(tool_says, ctx),
        PermissionResult::Allow { .. } | PermissionResult::Passthrough => {}""",
        """        PermissionResult::Ask { .. } => return coerce_ask(tool_says, ctx),
        PermissionResult::Allow { .. } => return tool_says,
        PermissionResult::Passthrough => {}""",
        "工具的 allow 绕过安全检查 —— Bash 判定 `echo x >> ~/.zshrc` 安全就直接执行",
    ),
    (
        "解析后的路径不查形状",
        "tools",
        "tools/path.rs",
        """    if let Some(r) = &resolved {
        fence::check_shape(r)?;
    }""",
        "",
        "symlink 指向 NUL 设备或带 ADS 的路径时检查失效",
    ),
    (
        "内容级 deny 排在 bypass 之后",
        "permissions",
        "chain.rs",
        """    if let Some(c) = content.as_deref()
        && let Some(r) = rules.content_rule(name, c, RuleDecision::Deny, MatchMode::Raw)
    {
        return PermissionResult::Deny {
            message: format!("`{name}` 的这次调用被规则禁止"),
            reason: rule_reason(r.source, r.pattern.as_deref().unwrap_or_default()),
        };
    }

""",
        "",
        "bypass 模式下内容级 deny 规则完全失效",
    ),
    (
        "规划模式改成询问",
        "permissions",
        "chain.rs",
        """        PermissionMode::Plan => PermissionResult::Deny {
            message: format!("规划模式下不能使用 `{}`。先退出规划模式。", tool.name()),
            reason: DecisionReason::Mode { mode },
        },""",
        """        PermissionMode::Plan => finish_ask(
            format!("是否允许 `{}`？", tool.name()),
            vec![allow_tool_suggestion(tool.name())],
            DecisionReason::Mode { mode },
            ctx,
        ),""",
        "规划模式下反复弹窗，用户点一次允许就动手改了代码",
    ),
    # ── Bash 分析 ─────────────────────────────────────
    (
        "AST 遍历只看 named 节点",
        "permissions",
        "bash/ast.rs",
        """        let allowed = if node.is_named() {
            ALLOWED_NODES.contains(&kind)
        } else {
            ALLOWED_ANON.contains(&kind)
        };""",
        """        let allowed = if node.is_named() {
            ALLOWED_NODES.contains(&kind)
        } else {
            true
        };""",
        "`npm test &` 与 `npm test` 无法区分 —— 后台进程逃出生命周期管理",
    ),
    (
        "白名单改成黑名单",
        "permissions",
        "bash/ast.rs",
        "            ALLOWED_NODES.contains(&kind)",
        '            !matches!(kind, "command_substitution" | "process_substitution")',
        "grammar 升级新增的节点类型静默放行",
    ),
    (
        "子命令提取不递归",
        "permissions",
        "bash/ast.rs",
        """    let mut cur = node.walk();
    for child in node.named_children(&mut cur) {
        collect_commands(child, src, out)?;
    }
    Ok(())""",
        """    let mut cur = node.walk();
    for child in node.named_children(&mut cur) {
        if child.kind() == "command" {
            out.push(parse_command(child, src)?);
        }
    }
    Ok(())""",
        "只看一层，`a && b | c` 这类嵌套里的子命令完全不检查",
    ),
    (
        "危险环境变量区分大小写",
        "permissions",
        "bash/ast.rs",
        "            && DANGEROUS_VARS.contains(&text.to_ascii_uppercase().as_str())",
        "            && DANGEROUS_VARS.contains(&text)",
        "`ld_preload=/evil.so ls` 绕过检查",
    ),
    (
        "sudo 被当成安全包装剥掉",
        "permissions",
        "bash/ast.rs",
        """    Wrapper {
        name: "nohup",""",
        """    Wrapper {
        name: "sudo",
        value_flags: &["-u", "-g"],
        positionals: 0,
    },
    Wrapper {
        name: "nohup",""",
        "`Bash(rm /tmp/*)` 规则会放行 `sudo rm /tmp/*`",
    ),
    (
        "子命令聚合取最宽松",
        "permissions",
        "bash/decide.rs",
        """            SubVerdict::Ask { pattern, source } => {
                all_allowed = false;""",
        """            SubVerdict::Ask { pattern, source } => {
                let _ = (&pattern, &source);
                continue;
            }
            #[allow(unreachable_patterns)]
            SubVerdict::Ask { pattern, source } => {
                all_allowed = false;""",
        "`npm test && curl evil.sh` 里只要有一条被允许就整体放行",
    ),
    (
        "剥离形态不符时强行剥",
        "permissions",
        "bash/ast.rs",
        """        let Some((inner, inner_args)) = strip_one(w, &args) else {""",
        """        let Some((inner, inner_args)) = strip_one(w, &args).or_else(|| {
            args.first().map(|f| (f.clone(), args[1..].to_vec()))
        }) else {""",
        "`timeout -k 5 30 npm test` 的命令名被认成 `5`，规则匹配到不存在的命令",
    ),
    (
        "只读判定忽略未加引号的 glob",
        "permissions",
        "bash/readonly.rs",
        """    if sub.has_unquoted_glob {
        return false;
    }""",
        "",
        "`cat *` 判成只读跳过确认，展开后可能读到围栏外的文件",
    ),
    (
        "git 子命令用黑名单",
        "permissions",
        "bash/readonly.rs",
        "    if !GIT_READ_ONLY.contains(&sub.as_str()) {\n        return false;\n    }",
        '    if matches!(sub.as_str(), "push" | "commit" | "reset") {\n        return false;\n    }',
        "`git my-custom-deploy` 判成只读直接执行",
    ),
    (
        "flag 匹配不认等号形式",
        "permissions",
        "bash/readonly.rs",
        """    let head = arg.split('=').next().unwrap_or(arg);
    if deny.contains(&head) {
        return true;
    }""",
        """    if deny.contains(&arg) {
        return true;
    }
    let head = arg;""",
        "`sed --in-place=.bak` 判成只读",
    ),
    (
        "AstVerified 模式放宽到前后缀也不查",
        "permissions",
        "rules.rs",
        """    if pattern.is_empty() {
        return text.is_empty();
    }""",
        """    if pattern.is_empty() {
        return mode == MatchMode::AstVerified || text.is_empty();
    }""",
        "AST 验证过的命令上，空模式变成万能放行",
    ),
    # ── 文件工具 ──────────────────────────────────────
    (
        "解码用 lossy",
        "tools",
        "tools/text.rs",
        """    let text = std::str::from_utf8(body).map_err(|_| DecodeError::Binary {
        reason: "不是有效的 UTF-8",
    })?;""",
        """    let owned = String::from_utf8_lossy(body).into_owned();
    let text = owned.as_str();""",
        "非 UTF-8 字节变成 U+FFFD，Edit 全量写回后原始内容永久丢失且不报错",
    ),
    (
        "写回不还原 CRLF",
        "tools",
        "tools/text.rs",
        """    let body = match newline {
        Newline::Crlf => content.replace('\\n', "\\r\\n"),
        Newline::Lf | Newline::Mixed => content.to_owned(),
    };""",
        """    let body = content.to_owned();""",
        "改一行让整个文件每一行都进 diff，真正的改动被淹没",
    ),
    (
        "写回丢掉 BOM",
        "tools",
        "tools/text.rs",
        """    if bom {
        out.extend_from_slice(BOM);
    }""",
        "",
        "某些 Windows 工具链不再认这个文件",
    ),
    (
        "Edit 多处匹配时改第一处",
        "tools",
        "tools/edit.rs",
        """    if n > 1 && !input.replace_all {""",
        """    if false && n > 1 && !input.replace_all {""",
        "模型以为改的是这一处，实际改了另一处 —— 代码悄悄坏掉，不报错",
    ),
    (
        "Partial 视图当成 Full",
        "tools",
        "tools/precondition.rs",
        """    if let FileView::Partial { offset, limit } = state.view {
        return Err(Staleness::PartialOnly { offset, limit });
    }""",
        "",
        "模型只读了半个文件就改，把'这个函数只出现一次'当成事实",
    ),
    (
        "写入前只查 mtime 不比对内容",
        "tools",
        "tools/precondition.rs",
        """    if current.content != expected {
        return Err(format!(
            "{} 在这次操作进行期间被修改了。请重新 Read 后再试。",
            resolved.display()
        ));
    }""",
        "",
        "mtime 精度只有 1 秒的文件系统上，同一秒内的用户改动被静默覆盖",
    ),
    (
        "Write 覆盖不要求先读",
        "tools",
        "tools/write.rs",
        """            let state = match check_fresh(&resolved, &ctx).await {
                Ok(s) => s,
                Err(stale) => return ToolOutcome::failed(stale.for_model(&parsed.path)),
            };""",
        """            let state = match check_fresh(&resolved, &ctx).await {
                Ok(s) => s,
                Err(_) => riot_protocol::tool::FileState {
                    content: String::new(),
                    mtime_ms: 0,
                    view: riot_protocol::tool::FileView::Full,
                },
            };""",
        "模型基于半小时前的印象重写整个文件，抹掉用户这期间的改动",
    ),
    (
        "Edit 的 call 依赖 validate_input 查过",
        "tools",
        "tools/edit.rs",
        """        let state = match check_fresh(&resolved, &ctx).await {
            Ok(s) => s,
            Err(stale) => return ToolOutcome::failed(stale.for_model(&parsed.path)),
        };""",
        """        let state = match check_fresh(&resolved, &ctx).await {
            Ok(s) => s,
            Err(_) => match ctx.file_state.get(&resolved) {
                Some(s) => s,
                None => return ToolOutcome::failed("读取失败"),
            },
        };""",
        "权限弹窗那段时间里文件被改，call 不再复查 —— TOCTOU 防线消失",
    ),
    (
        "Edit 的 call 不复查唯一性",
        "tools",
        "tools/edit.rs",
        """        if let Err(msg) = match_count_check(&state.content, &parsed) {
            return ToolOutcome::failed(msg);
        }""",
        "",
        "弹窗期间文件新增了一处同样的文本，改错地方且不报错",
    ),
    (
        "截断的 Read 仍标 Full 视图",
        "tools",
        "tools/read.rs",
        """        let view = if render.is_complete {
            FileView::Full
        } else {""",
        """        let view = if true {
            FileView::Full
        } else {""",
        "Edit 的'必须完整读过'防线失效 —— 模型没看到全文就动手改",
    ),
    # ── Bash ──────────────────────────────────────────
    (
        "用登录 shell 让 alias 生效",
        "tools",
        "tools/bash.rs",
        'args: vec!["-c".to_owned(), parsed.command.clone()],',
        'args: vec!["-lc".to_owned(), parsed.command.clone()],',
        "用户 rc 里的 alias 让同一条命令在不同机器上做不同的事，而模型看不到那些配置",
    ),
    (
        "不禁用编辑器",
        "tools",
        "tools/bash.rs",
        '        ("GIT_EDITOR", "true"),',
        "",
        "`git commit` 开编辑器等按键，用户白等两分钟换一个没信息量的超时",
    ),
    (
        "不禁用分页器",
        "tools",
        "tools/bash.rs",
        '        ("GIT_PAGER", "cat"),',
        "",
        "`git log` 开分页器挂死",
    ),
    (
        "输出截断只保开头",
        "tools",
        "tools/bash.rs",
        """    let text = format!(
        "{}\\n\\n… 中间省略 {omitted} 行 …\\n\\n{}",
        head.join("\\n"),
        tail.iter().copied().collect::<Vec<_>>().join("\\n")
    );""",
        """    let text = format!(
        "{}\\n\\n… 后面省略 {omitted} 行 …",
        head.join("\\n")
    );""",
        "编译错误汇总在末尾，只保开头等于把最有价值的部分丢掉",
    ),
    (
        "非零退出当成功",
        "tools",
        "tools/bash.rs",
        "        if out.timed_out || out.exit_code != 0 {",
        "        if out.timed_out {",
        "命令失败了模型却以为成功，继续往下走",
    ),
    (
        "空输出返回空字符串",
        "tools",
        "tools/bash.rs",
        """    if stdout.text.is_empty() && stderr.text.is_empty() {""",
        """    if false && stdout.text.is_empty() && stderr.text.is_empty() {""",
        "模型以为工具坏了，原样重试一遍",
    ),
    (
        "看不懂的命令当只读",
        "tools",
        "tools/bash.rs",
        "            bash::Analysis::TooComplex(_) => false,",
        "            bash::Analysis::TooComplex(_) => true,",
        "`ls $(curl evil.sh)` 被判成只读，跳过确认还能并发执行",
    ),
    (
        "call 不夹超时上限",
        "tools",
        "tools/bash.rs",
        """        let timeout_ms = parsed
            .timeout_ms
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS);""",
        """        let timeout_ms = parsed.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);""",
        "绕过 validate_input 的调用能拿到没有上界的超时，会话被一条命令卡死",
    ),
    (
        "Bash 失败不级联",
        "tools",
        "tools/bash.rs",
        """    fn cascades_on_failure(&self) -> bool {
        true
    }""",
        """    fn cascades_on_failure(&self) -> bool {
        false
    }""",
        "`mkdir foo` 失败后并行的 `cd foo && ...` continue 跑，产生一堆误导性错误",
    ),
    # ── Grep ──────────────────────────────────────────
    # 这里曾经有两个变异："pattern 当位置参数"（漏 `-e` 前缀，搜 `--force`
    # 会被 rg 当 flag）和"不隔离用户的 rg 配置"（漏 `--no-config`）。
    #
    # 两个都**删掉了**，不是因为锚点漂了，而是因为它们防的东西已经不存在：
    # Grep 底层从 rg 的二进制换成了 ripgrep 的库（`grep-searcher` / `ignore`），
    # 没有 argv 也没有子进程，pattern 是一个 Rust 值，`RIPGREP_CONFIG_PATH`
    # 根本不会被读。这类变异重新锚定只会得到一个永远杀不死的假变异。
    #
    # 记在这里而不是直接删干净：下一个读这份清单的人会问"搜索层怎么没有
    # 命令注入相关的变异"，答案是那条路整个不存在了。
    (
        "没搜到当成失败",
        "tools",
        "tools/grep.rs",
        "            return ToolOutcome::ok_text(no_match_text(&parsed));",
        "            return ToolOutcome::failed(no_match_text(&parsed));",
        "「没搜到」被报成搜索失败，模型会反复调参数重试，而正确的下一步是换个词",
    ),
    (
        "遍历被截断不告诉模型",
        "tools",
        "tools/grep.rs",
        "        found.cut_short |= walked.cut_short;",
        "        let _ = walked.cut_short;",
        "遍历撞上文件数上限后结果不完整，而模型以为它搜完了 —— 静默的错答案",
    ),
    (
        "不完整的结果不加提示",
        "tools",
        "tools/grep.rs",
        r"""    if cut_short {
        body.push_str(&format!(
            "\n\n<system-reminder>搜索没走完（超过 {}s 或文件太多），\
             上面只是已经找到的部分。用 `path` 缩小范围，或者加 `glob` \
             过滤文件类型。</system-reminder>",
            search::TIME_BUDGET_SECS
        ));
    }""",
        "",
        "超时或文件太多导致搜索提前收工，模型却拿着半份结果当全部 —— 不报错不崩，只是结论错",
    ),
    (
        "没走完却说没找到",
        "tools",
        "tools/grep.rs",
        "            if found.cut_short {",
        "            if false {",
        "一次超时的搜索被报成「没有找到」，模型据此断定这东西不存在 —— 把「没搜」说成「不存在」",
    ),
    (
        "Grep 搜索根不过围栏",
        "tools",
        "tools/grep.rs",
        """            Some(p) => match path::resolve(p, &ctx, true).await {
                Ok(r) => r,
                Err(e) => return ToolOutcome::failed(e.for_model()),
            },""",
        """            Some(p) => PathBuf::from(p),""",
        "`path: \"/etc\"` 就能翻工作目录外面的东西",
    ),
    # ── 真实进程执行器 ────────────────────────────────
    (
        "只在超时时才清理进程组",
        "runtime",
        "proc.rs",
        "        terminate_group(child.as_mut(), self.grace).await;",
        """        if !matches!(ended, Ended::Exited(_)) {
            terminate_group(child.as_mut(), self.grace).await;
        }""",
        "命令正常退出后它 spawn 的后台进程被 init 收养成孤儿，一直活到关机",
    ),
    (
        "顺序读两个管道",
        "runtime",
        "proc.rs",
        """        let h_out = tokio::spawn(drain(out_pipe, cap));
        let h_err = tokio::spawn(drain(err_pipe, cap));""",
        """        let h_out = tokio::spawn(async move { drain(out_pipe, cap).await });
        let first = join(h_out).await?;
        let h_err = tokio::spawn(drain(err_pipe, cap));
        let h_out = tokio::spawn(async move { Ok(first) });""",
        "输出超过管道缓冲（64KB）时死锁 —— 小输出全绿，某次编译警告一多就挂死",
    ),
    (
        "stdin 继承父进程",
        "runtime",
        "proc.rs",
        "            .stdin(Stdio::null())",
        "            .stdin(Stdio::inherit())",
        "`cat` 这类命令一直等输入，而那个 stdin 是内核的 JSON-RPC 通道",
    ),
    (
        "先收输出再杀进程组",
        "runtime",
        "proc.rs",
        """        terminate_group(child.as_mut(), self.grace).await;""",
        """        let _late = ();
        let (stdout0, c0) = join(h_out).await?;
        let (stderr0, c1) = join(h_err).await?;
        let h_out = tokio::spawn(async move { Ok((stdout0, c0)) });
        let h_err = tokio::spawn(async move { Ok((stderr0, c1)) });
        terminate_group(child.as_mut(), self.grace).await;""",
        "后台进程继承了 stdout，管道写端不关就永远等不到 EOF —— 整个会话卡死",
    ),
    (
        "不包进程组",
        "runtime",
        "proc.rs",
        "        wrap.wrap(process_wrap::tokio::ProcessGroup::leader());",
        "        let _ = &mut wrap;",
        "kill 只杀直接子进程，孙进程全部逃逸",
    ),
    (
        "上限之后继续读",
        "runtime",
        "proc.rs",
        """        if room == 0 {""",
        """        if false {""",
        "`yes` 这类命令把内存吃光",
    ),
    (
        "Grep 正则不预校验",
        "tools",
        "tools/grep.rs",
        """        if let Err(e) = regex_lite::Regex::new(&parsed.pattern) {
            return Err(ValidationError::rejected(format!(
                "`pattern` 不是合法的正则：{e}。搜索字面量时记得转义 \\
                 `.`、`(`、`[`、`*`、`+`、`?`、`|` 这些字符。"
            )));
        }
""",
        "",
        "模型收到一段 rg 内部诊断，而且白等一次进程启动",
    ),
]


def sources(layer: str) -> Path:
    return LAYERS[layer][1]


def run_tests(layer: str) -> tuple[int, list[str]]:
    """返回 (失败数, 失败的测试名)。"""
    pkg = LAYERS[layer][0]
    try:
        r = subprocess.run(
            ["cargo", "test", "-p", pkg, *LAYERS[layer][2], "--", "--test-threads=4"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            # 有些变异的表现是挂死而不是失败 —— 去掉进程组清理、
            # 顺序读两个管道、stdin 不接 null，症状都是"永远不返回"。
            # 没有这个超时的话脚本会跟着一起卡住。
            timeout=600,
        )
    except subprocess.TimeoutExpired:
        return 1, ["超时（挂死也是一种被抓住）"]
    out = r.stdout + r.stderr
    if "error[" in out or ("error:" in out and "test result" not in out):
        return -1, ["编译失败"]
    failed = re.findall(r"^test (\S+) \.\.\. FAILED", out, re.M)
    return len(failed), failed


def selected(argv: list[str]) -> list[tuple]:
    layers = [a for a in argv[1:] if not a.startswith("-")]
    if not layers:
        return MUTANTS
    unknown = [l for l in layers if l not in LAYERS]
    if unknown:
        print(f"未知的层：{', '.join(unknown)}。可选：{', '.join(LAYERS)}")
        sys.exit(2)
    return [m for m in MUTANTS if m[1] in layers]


def check_anchors(mutants: list[tuple]) -> int:
    """只检查锚点还在不在，不跑测试。

    重构会让锚点静默失效。没有这个检查的话，表现是变异测试"全部通过" ——
    因为它什么都没改。这比测试失败危险得多：它给的是虚假的安全感。
    """
    stale = [
        (name, layer, filename)
        for name, layer, filename, old, _, _ in mutants
        if old not in (sources(layer) / filename).read_text()
    ]

    for name, layer, filename in stale:
        print(f"  ✗ {name}（{layer}/{filename}）：锚点失效")

    if stale:
        print(f"\n{len(stale)}/{len(mutants)} 个锚点失效。实现改过了，变异脚本要跟上。")
        return 1

    print(f"{len(mutants)}/{len(mutants)} 个锚点都在。")
    return 0


BACKUP_DIR = ROOT / ".mutate-backup"


def stash(path: Path, content: str) -> Path:
    """把原文落盘再改源码。

    `try/finally` 挡不住 SIGKILL —— 进程被强杀时源码会停在变异状态，
    而那个变异会安静地活在仓库里，直到某天有人发现测试莫名其妙变慢了。
    （真发生过：一次被中断的运行留下了"不包进程组"，全量测试从 0.8 秒
    变成 60 秒才暴露出来。）

    落盘的备份能扛住强杀，下次启动时 restore_stale 会捡回来。
    """
    BACKUP_DIR.mkdir(exist_ok=True)
    rel = path.relative_to(ROOT)
    b = BACKUP_DIR / str(rel).replace("/", "__")
    b.write_text(content)
    return b


def restore_stale() -> bool:
    """恢复上次异常退出留下的源码。"""
    if not BACKUP_DIR.exists():
        return False

    found = False
    for b in sorted(BACKUP_DIR.iterdir()):
        target = ROOT / b.name.replace("__", "/")
        if not target.exists():
            b.unlink()
            continue
        original = b.read_text()
        if target.read_text() != original:
            print(f"!! 恢复上次中断留下的变异：{target.relative_to(ROOT)}")
            target.write_text(original)
            found = True
        b.unlink()

    if BACKUP_DIR.exists() and not any(BACKUP_DIR.iterdir()):
        BACKUP_DIR.rmdir()
    return found


def main() -> int:
    if restore_stale():
        print()

    mutants = selected(sys.argv)

    if "--check-anchors" in sys.argv:
        return check_anchors(mutants)

    layers = sorted({m[1] for m in mutants})
    print("基线：", end=" ", flush=True)
    for layer in layers:
        n, _ = run_tests(layer)
        if n != 0:
            print(f"{layer} 基线就有 {n} 个失败，先修好再跑变异测试")
            return 1
    print("全绿\n")

    survived = []
    for name, layer, filename, old, new, impact in mutants:
        f = sources(layer) / filename
        original = f.read_text()
        if old not in original:
            print(f"  ?? {name}：找不到锚点，变异脚本要更新\n")
            survived.append((name, impact))
            continue

        backup = stash(f, original)
        f.write_text(original.replace(old, new, 1))
        try:
            n, names = run_tests(layer)
        finally:
            f.write_text(original)
            backup.unlink(missing_ok=True)

        if n > 0:
            print(f"  ✓ {name}")
            print(f"     抓到 {n} 个：{', '.join(t.split('::')[-1] for t in names[:3])}")
        elif n < 0:
            print(f"  ✓ {name}（编译失败，等于抓到）")
        else:
            print(f"  ✗ {name} —— 没有任何测试失败")
            print(f"     后果：{impact}")
            survived.append((name, impact))
        print()

    if survived:
        print(f"\n{len(survived)}/{len(mutants)} 个变异存活，测试有缺口：")
        for name, impact in survived:
            print(f"  - {name}：{impact}")
        return 1

    print(f"\n{len(mutants)}/{len(mutants)} 个变异全部被抓住。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
