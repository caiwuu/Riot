---
name: extend-riot
description: 用户想加技能、斜杠命令、hook、或项目约定时用。四种扩展点的目录、frontmatter 字段、以及一批模型猜不到的格式限制和优先级规则。
---

# 扩展 Riot

四个扩展点都是普通文件，改完**下一轮对话生效**，不用重启。全局的放配置
目录（设置 → 关于里能看到，macOS 是 `~/Library/Application Support/riot`），
项目级的放 `<项目>/.riot/`。

同名时**项目级赢**（和 git 的 local > global 同一直觉）。

## 技能 SKILL.md

```text
<配置目录>/skills/<名字>/SKILL.md     全局
<项目>/.riot/skills/<名字>/SKILL.md   项目级
```

```markdown
---
name: verify
description: 改完代码要验证时用。按分层防线依次跑。
disable-model-invocation: false
---
正文。可用 $ARGUMENTS 和 ${SKILL_DIR}。
```

限制（这些都是模型猜不到的）：

- **`description` 必填**，缺了不静默跳过，作为「有问题的技能」报给设置页。
  它是模型决定要不要加载的唯一依据。
- **`description` 硬顶 250 字符**。超了会被截断，模型就是在残句上做判断。
  写不下就说清「什么时候用」，别写做法——做法在正文里。
- frontmatter 只认 `key: value` **单行标量**。多行、列表、嵌套都不认。
- 正文 64 KB 封顶。数据文件放技能目录里让模型按需 Read，不要整个贴进来。
- `allowed-tools` / `model` / `context: inline|fork` **还不支持**，写了会被
  忽略而不是报错。
- **`disable-model-invocation: true`** = 只给用户 `/` 调，不进 Skill 工具的
  清单。这时它会**就地展开**成提示词（因为模型的清单里没有它，不展开就
  谁都跑不了）；普通技能反过来，只把名字发给模型，由它用 Skill 工具按需
  加载正文——几 KB 正文不该塞进用户可见的消息。
- 只认 `true`，写 `yes` / `1` 按「没关」算。

## 斜杠命令

```text
<配置目录>/commands/**/*.md
<项目>/.riot/commands/**/*.md
```

- frontmatter **可省略**（没有 `---` 开头就整个文件当正文，description 取
  正文第一行）。可写 `description`、`argument-hint`。
- 子目录变命名空间：`commands/git/pr.md` → `/git:pr`。
- `$ARGUMENTS` = 整段参数原文；`$1..$9` = 按空白拆的第 N 个（`"带 空格"`
  算一个，引号剥掉）。模板里一个占位符都没有而用户给了参数时，末尾追加
  `ARGUMENTS: <args>`——否则参数被静默扔掉，看起来像命令不认参数。
- 模板里可以写 `@路径`：展开后走普通发送那条路，`@` 引用会照常带上文件内容。
- **`` !`cmd` `` 嵌入执行不支持**，那是把「展开提示词」变成「执行任意命令」
  的口子。
- 优先级：**内置 > 项目命令 > 全局命令 > 技能**。自定义顶不掉内置
  （`/compact` 的行为要可预期），同名时命令压过技能。

技能和命令共用一条发现管道，所以**已经有一个技能了就别再写一个同名命令**
——那只是给自己制造一个优先级问题。

## Hooks

```text
<配置目录>/hooks.json
<项目>/.riot/hooks.json
```

两层**叠加**（都会跑），不是覆盖。

```json
{
  "PreToolUse":  [{ "matcher": "Bash|Write",
                    "hooks": [{ "type": "command", "command": "./check.sh", "timeout": 30 }] }],
  "PostToolUse": [{ "matcher": "Write", "hooks": [{ "type": "command", "command": "cargo fmt --check" }] }],
  "Stop":        [{ "hooks": [{ "type": "command", "command": "cargo test -q" }] }],
  "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "./scan-secrets.sh" }] }]
}
```

只有这四个检查点。协议：

- stdin 收一行事件 JSON（`hook_event_name`、`cwd`、`tool_name`、`tool_input`、
  `prompt`、`stop_hook_active`…）。
- **`exit 2` = 拦下**，stderr 是给模型看的理由。
- `exit 0` 的 stdout 作为补充上下文；若是 JSON 还能给
  `hookSpecificOutput.permissionDecision`（allow/deny/ask）和 `updatedInput`。
- **其它退出码只记日志**——检查脚本自己坏了不该拦住整条链路。
- 同事件并行跑、同命令去重，默认超时 60 秒。
- 只支持 `type: "command"`。

替用户写 hook 脚本时的坑（都因为「坏了只记日志」而**静默失效**，
用户只会觉得「hook 没生效」）：

- 脚本要有 shebang 且 `chmod +x`；
- 脚本里用到的命令（`jq`、`python3`……）先 `command -v` 确认装了，
  别假设用户机器和你想的一样；
- matcher 先松后紧：先不带 matcher 跑通，确认脚本本身对，再收紧。
  一上来就写复杂正则，分不清是没匹配上还是脚本坏了；
- 写完**真触发一次**那个事件验证，别写完就完。

一条安全边界：hook 的 `allow` 只能免掉例行询问，**压不过**安全检查（写 SSH
密钥、凭证文件、命令注入这些）和用户自己写的 ask 规则。否则 clone 一个带
`hooks.json` 的仓库就等于关掉整套防线。

## 项目约定 AGENTS.md

```text
<配置目录>/AGENTS.md          全局
<项目>/AGENTS.md              项目级（回退 CLAUDE.md，只取其一）
```

注入首条消息（不是 system prompt），压缩后会重注。支持 `@路径` 引用，递归
展开、深度 5、有循环检测，围栏和行内代码里的 `@` 会跳过。单文件 64 KB 硬截、
40 KB 警告。**不向上遍历父目录**（刻意与 Claude Code 不同）。

写什么：**代码里 grep 不到的东西**。「这个函数做什么」不要写，读代码就有；
「为什么不用另一条路」「这个约束破了会怎样」才值得写。

## 加完之后

设置页的对应板块会列出解析结果，包括**解析失败的原因**——加完去看一眼，
比在对话里发现「怎么没生效」快得多。
