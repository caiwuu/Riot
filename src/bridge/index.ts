/**
 * Bridge 层 —— 前端与 Tauri 宿主之间唯一的通道。
 *
 * [约束] 这是整个 `src/` 里唯一允许 import `@tauri-apps/api` 的目录。
 * 其余代码只能 import 本模块导出的函数。违反这条会让前端无法在浏览器里
 * 单独跑起来（调试、Storybook、组件测试全部失效），也让 mock 无处下手。
 *
 * 这条约束由 eslint 强制，见 eslint.config.js：静态 import 归
 * no-restricted-imports，`await import(...)` 归 no-restricted-syntax ——
 * 只配前者的话，逃逸会从动态那半漏过去（曾漏进过一处系统通知）。
 */

import { Channel, invoke } from "@tauri-apps/api/core";

import type {
  AgentEvent,
  ApiProtocol,
  FileChange,
  GitChanges,
  ImageInput as GeneratedImageInput,
  McpServerStatus as GeneratedMcpServerStatus,
  Message,
  PendingAsk,
  PermissionAsk,
  PermissionMode,
  PermissionResponse,
  QueuedSummary as GeneratedQueuedSummary,
  ThinkingEffort,
  ThinkingPolicy as GeneratedThinkingPolicy,
} from "./generated";

export type {
  AgentEvent,
  FileChange,
  GitChanges,
  Message,
  PendingAsk,
  PermissionAsk,
  PermissionMode,
  PermissionResponse,
};

/** 服务方协议。决定请求格式、认证头和哪些采样参数可发送。
 * 生成类型的别名，不是第二份定义 —— 手写一份的话，Rust 侧加变体时
 * 这边不会有任何报错，只会在运行时变成一个"未知协议"。 */
export type Protocol = ApiProtocol;

/**
 * 采样参数。每个字段三态，和宿主的 `config::Sampling` 逐字段对应：
 *
 * - 键不在（`undefined`）= 继承下一层（会话 → 模型 → 服务方 → 不发）
 * - `null` = 显式不发这个参数，用模型自己的默认值
 * - 数字 = 就用这个值
 *
 * `[约束]` `undefined` 和 `null` 在这里含义相反，不能用 `??` 抹平 ——
 * 抹平之后"用模型默认"就退化成"继承"，上层设的值会重新漏下来。
 */
export interface Sampling {
  temperature?: number | null;
  topP?: number | null;
  /** 仅 Anthropic 协议发送。 */
  topK?: number | null;
  maxOutputTokens?: number | null;
}

/** 思考力度档。取值与 OpenAI 的 reasoning_effort 对齐，DeepSeek / GLM 也认。
 * 生成类型（riot-protocol 的 ThinkingEffort）的别名，同 Protocol 的理由。 */
export type ThinkingLevel = ThinkingEffort;

/**
 * 会话级思考策略。生成类型的别名（同 Protocol 的理由）：
 * - default：不发任何思考参数，端点默认行为；
 * - adaptive：首请求中档、工具续轮低档；
 * - disabled：显式关闭思考（部分端点不支持，如 GLM-5.3 / OpenAI 官方）；
 * - fixed：每次请求固定档位。
 */
export type ThinkingPolicy = GeneratedThinkingPolicy;

/** 一个模型服务方。**不含 API key** —— 密钥存宿主侧的 auth.json。 */
export interface ProviderConfig {
  id: string;
  name: string;
  protocol: Protocol;
  /** 接口主机，可以带前缀路径。 */
  baseUrl: string;
  /**
   * 接口路径，如 `/v1/chat/completions`。空 = 按主机猜。
   *
   * 可配置的理由:各家的根路径对不上。智谱的对话在
   * `/api/paas/v4/chat/completions`（带 /v1 就 404），而它的完整模型清单偏偏在
   * `/api/paas/v4/v1/models`。猜错的表现是一个 404，报错里没有任何线索指向路径。
   */
  apiPath?: string;
  /** 读 key 的环境变量名，同时是 auth.json 里的存储键。 */
  apiKeyEnv: string;
  /** 已添加的模型（手动或从 /models 接口挑的）。 */
  models: ModelConfig[];
  fallbackModel?: string | null;
  /** 这个服务方的采样参数。会话可以临时覆盖单个字段。 */
  sampling: Sampling;
}

/**
 * 一个模型的配置。
 *
 * 能力和采样参数属于模型，不属于服务方 —— 同一家同时有视觉模型和纯文本模型
 * 是常态（智谱的 glm-4.6v 能看图、glm-5.2 不能）。按服务方记的话，为了把前者
 * 配成视觉兼容模型就得给整家打开，于是和后者聊天时截图也会被当成图片发出去，
 * 服务方回一句「messages.content.type 参数非法」——而那句话完全不指向截图。
 */
export interface ModelConfig {
  /** 发给服务方的模型名。 */
  id: string;
  /** 显示名。空 = 直接显示 id。 */
  name?: string;
  /** 能收图片。 */
  vision?: boolean;
  /**
   * 上下文窗口有多大（token）。省略 = 没填，压缩阈值走设置里那个全局数。
   *
   * 填的是**窗口**而不是压缩阈值：窗口是模型文档上查得到的客观数字，阈值
   * 要先知道内部给回复和总结各留了多少才推得出来。宿主拿窗口减去这两笔
   * 预留算出阈值。
   */
  contextWindow?: number;
  /** 这个模型的采样参数。空字段继承 provider 的。 */
  sampling?: Sampling;
}

/** 联网能力。抓取和搜索分开开关。 */
export interface WebConfig {
  /** 允许 WebFetch 抓网页。 */
  fetchEnabled: boolean;
  /** 允许 WebSearch 搜索。空地址走内置实例。 */
  searchEnabled: boolean;
  /** 覆盖内置 SearXNG 的地址。空 = 用内置。 */
  searxngUrl: string;
  /** 蒸馏网页正文的辅助模型，格式 `providerId/model`。空 = 不蒸馏。 */
  distillModel: string;
}

/**
 * 一个 MCP 服务器（stdio 传输）。
 *
 * `id` 进工具名（`mcp__<id>__…`）和权限规则 —— 改了它等于换了一批工具名，
 * 用户点过的"总是允许"全部失配。
 */
export interface McpServerConfig {
  /** 稳定标识，只能用字母数字、- 和 _。 */
  id: string;
  /** 显示名。空 = 显示 id。 */
  name?: string;
  /** 启动命令，如 `npx`、`uvx` 或可执行文件路径。 */
  command: string;
  args?: string[];
  /** 附加环境变量（API key 之类）。 */
  env?: Record<string, string>;
  /** 关掉 = 进程停掉、工具消失，但配置留着。 */
  enabled?: boolean;
}

/**
 * 一条收藏的提示词。设置里维护，会话设置里挑一条填进「系统提示词」。
 *
 * 会话存的是正文副本而不是这条的 id：改一条预设不该悄悄改掉已有会话的行为。
 */
export interface PromptPreset {
  /** 稳定标识，只用于界面定位。 */
  id: string;
  /** 显示名。宿主在空的时候不发这个字段 —— 界面拿正文首行顶上。 */
  title?: string;
  /** 正文。选中时原样填进会话的系统提示词。 */
  body: string;
}

/** 应用配置，整个结构持久化到 config.json。 */
export interface AppConfig {
  providers: ProviderConfig[];
  activeProvider: string;
  activeModel: string;
  /** 最近打开过的项目目录，最近的在前。 */
  projects: string[];
  /** 新会话的默认权限模式。 */
  defaultMode?: PermissionMode | null;
  /** 权限弹窗等多久算超时（秒）。超时按拒绝处理，宿主侧夹在 5–3600。 */
  askTimeoutSecs: number;
  /** 单轮最多自主往返多少步。到顶停下等用户。宿主侧夹在 1–1000。 */
  maxTurns: number;
  web: WebConfig;
  /** MCP 服务器。连接是应用级的（会话共享），工具每轮快照。 */
  mcpServers: McpServerConfig[];
  /** 收藏的提示词。老配置可能没有这个字段。 */
  prompts?: PromptPreset[];
  /** 历史估算超过这个 token 数时自动摘要压缩。宿主侧夹在 8k–1M。 */
  compactThresholdTokens: number;
  /**
   * 视觉兼容模型，格式 `providerId/model`。
   *
   * 主模型收不了图片时，用它把图片转成文字再交给主模型。空 = 不转，
   * 截图工具会直接说去配一下。
   */
  visionModel: string;
  /**
   * 子 agent 的便宜模型，格式 `providerId/model`。空 = 跟主模型。
   *
   * 只有只读侦察（`explore`）会走它 —— 那类任务只汇报不改东西，但吃掉的
   * token 往往比主对话还多。会改代码的 `general-purpose` 始终跟主模型。
   */
  subagentModel: string;
  /**
   * 命令的 OS 级隔离。
   *
   * 不只是安全设置：权限决策链里"沙箱内自动放行"那一档要它开着才成立，
   * 关掉之后每个非只读命令又回到"要么弹窗、要么全部放行"的二选一。
   * 目前只有 macOS 能真正生效，其他平台自动降级成不隔离。
   */
  sandbox: SandboxMode;
  /**
   * 沙箱内额外可读的路径（Windows 的 allowRead，手填，每行一条的语义）。
   * per-user 工具（nvm、conda、`pip install --user`……）沙箱内默认打不开，
   * 需要谁填谁；macOS 读本就全开，用不上。老配置可能没有这个字段。
   */
  sandboxAllowRead?: string[];
}

export type SandboxMode = "workspaceWrite" | "workspaceWriteNoNet" | "off";

/** 侧边栏里的一个会话。会话从创建起绑定 root，永不改变。 */
export interface SessionInfo {
  id: string;
  root: string;
  title: string | null;
  seq: number;
  /** 会话级采样覆盖。空字段 = 继承模型/服务方那两层。 */
  sampling: Sampling;
  /** 宿主侧的当前权限模式。UI 显示必须以它为准，不能拿全局默认值顶替。 */
  mode: PermissionMode;
  /** 会话级思考策略。 */
  thinking: ThinkingPolicy;
  /** 会话级 Python 虚拟环境（venv 根目录）。null = 宿主默认环境。 */
  pythonVenv: string | null;
  /** 会话级追加的系统提示词。null = 只用内置提示词。 */
  systemPrompt: string | null;
  /** 此刻有没有轮子在跑。侧栏给后台忙碌的会话画指示点用。 */
  busy: boolean;
}

export interface ConfigStatus {
  config: AppConfig;
  /** provider id → "env" | "saved"。没配 key 的 provider 不出现。 */
  keyStatus: Record<string, string>;
  configPath: string;
  /** 本次启动时配置读不懂，原文件被挪到了这里。正常启动没有这个字段。 */
  configBackup?: string | null;
}

/** 当前激活的 provider 有没有可用的 key。 */
export function hasActiveKey(s: ConfigStatus): boolean {
  return Boolean(s.keyStatus[s.config.activeProvider]);
}

/** 事件流句柄。取消订阅只是停止分发，不会中断内核 —— 中断要显式调 interrupt。 */
export interface Subscription {
  unsubscribe(): void;
  /**
   * 宿主已经挂上这个 channel。之后的事件不会再进旧出口。
   *
   * 拉历史必须等它：并行的话，结束事件可能还打在已经没人听的旧
   * channel 上，而历史快照里 busy 仍是 true —— 界面就停在转圈。
   */
  ready: Promise<void>;
}

/**
 * 订阅序号。每次订阅取一个更大的值，宿主靠它分辨新旧。
 *
 * `[约束]` 必须单调递增，且**全局**唯一（不是每个会话一份）—— 同一个
 * 会话先后两次订阅要能比出先后，这是它唯一的用途。
 */
let subscribeEpoch = 0;

/**
 * 订阅一个会话的事件流。
 *
 * 用 `Channel` 而不是全局 `emit`：后者底层是拼 JS 字符串然后 eval，
 * 官方文档明说不适合高吞吐。Channel 保证有序，且能跨 command 长期持有。
 *
 * `[约束]` 必须带 `epoch`。两次订阅在宿主侧是两个并发任务，落地顺序
 * 没有保证；不带序号的话，先发的那次可能后落地，宿主于是把事件发给
 * 一个前端已经弃用的 channel —— 表现为发完消息永远转圈。见
 * `AppState::attach_sink`。
 */
export function subscribeSession(
  sessionId: string,
  onEvent: (event: AgentEvent) => void,
  onError?: (message: string) => void,
): Subscription {
  let active = true;
  const epoch = ++subscribeEpoch;
  const channel = new Channel<AgentEvent>();
  channel.onmessage = (event) => {
    if (active) onEvent(event);
  };

  // [约束] 这里必须处理失败。之前写的是 `void invoke(...)`，于是订阅被
  // ACL 拒绝时什么都不发生 —— 界面正常显示、发消息也不报错，只是永远
  // 收不到任何事件。那个 bug 藏了整整一个开发回合。
  const ready = invoke("subscribe_session", { sessionId, epoch, onEvent: channel }).then(
    () => undefined,
    (e: unknown) => {
      if (active) onError?.(String(e));
    },
  );

  return {
    unsubscribe() {
      active = false;
    },
    ready,
  };
}

/**
 * 发一轮用户输入。
 *
 * 立刻返回，不等这一轮跑完 —— 整轮可能要几分钟，等待期间用户按不了停止键。
 * 结果全部走事件流。
 *
 * 返回排队条目 id：上一轮还在跑时消息进插话队列（排队面板靠这个 id
 * 跟踪它，内核注入后回流的消息也用同一个 id）；`null` = 直接开轮了。
 */
export function sendTurn(
  sessionId: string,
  text: string,
  images: ImageInput[] = [],
  /** 输入框里选中的文件引用（那些块），项目内相对路径。 */
  refs: string[] = [],
): Promise<string | null> {
  return invoke<string | null>("send_turn", { sessionId, text, images, refs });
}

/** 丢掉这条助手回复及其后的一切，从它前面那条用户消息再跑一轮。 */
export function regenerateTurn(sessionId: string, messageId: string): Promise<void> {
  return invoke("regenerate_turn", { sessionId, messageId });
}

/**
 * 上下文编辑：把一条历史消息的文本段替换成新文本。
 *
 * 只动文本 —— 思考、工具调用/结果、附件原位保留。之后的轮次模型看到的
 * 就是改过的历史。空闲时才能做，忙时宿主会拒绝。
 */
export function editMessage(sessionId: string, messageId: string, text: string): Promise<void> {
  return invoke("edit_message", { sessionId, messageId, text });
}

/**
 * 上下文删除：按"轮"成对删 —— 这条消息所属的提问，连同它引出的全部
 * 回应（工具调用、结果、回复）一起从历史移除。提问没有得到任何回应
 * （被停止/出错）时就只删提问自己。空闲时才能做，忙时宿主会拒绝。
 */
export function deleteMessage(sessionId: string, messageId: string): Promise<void> {
  return invoke("delete_message", { sessionId, messageId });
}

/** 一条斜杠命令。模板正文留在宿主，展开走 slashExpand。 */
export interface SlashCommand {
  name: string;
  description: string;
  argumentHint?: string;
  /** `builtin` / `project` / `global` / `skill`。 */
  source: string;
  /**
   * 敲 `/名字` 时是否就地展开成提示词。
   *
   * 规则是「模型加载不了的才展开」：命令展开；普通技能不展开（把名字发给
   * 模型，由它用 Skill 工具按需加载正文，几 KB 正文不该进用户可见的消息）；
   * 写了 `disable-model-invocation` 的技能展开 —— 模型的清单里没有它。
   */
  expandInline: boolean;
  /**
   * 技能自己的层级（`builtin` / `global` / `project`）。只有 `source === "skill"` 才有。
   *
   * 有它才能在命令页写出「内置技能」而不是光写「技能」—— 后者会让同一个东西
   * 在 Skills 页和命令页显示成两个不同的标签。
   */
  skillSource?: string;
}

/** 可用的斜杠命令（内置 + 项目 + 全局）。root 为 null 时只列内置和全局。 */
export function slashCommands(root: string | null): Promise<SlashCommand[]> {
  return invoke<SlashCommand[]>("slash_commands", { root });
}

/** `@` 补全菜单的文件搜索。返回项目内相对路径，最多十来条。 */
export function searchFiles(sessionId: string, query: string): Promise<string[]> {
  return invoke<string[]>("search_files", { sessionId, query });
}

/** 配置里的一条 hook。error 非空时这条是"配置文件有问题"的提示。 */
export interface HookInfo {
  event: string;
  matcher: string;
  command: string;
  timeoutSecs: number;
  /** `global` / `project`。 */
  source: string;
  error?: string;
}

/** hooks.json 里配了什么（含解析失败的文件）。 */
export function hooksList(root: string | null): Promise<HookInfo[]> {
  return invoke<HookInfo[]>("hooks_list", { root });
}

/**
 * 展开一条自定义命令：`/name args` → 发给模型的 prompt。
 * null = 没这条命令，或它是内置命令（按 name 特判执行）。
 */
export function slashExpand(
  sessionId: string,
  name: string,
  args: string,
): Promise<string | null> {
  return invoke<string | null>("slash_expand", { sessionId, name, args });
}

/** 手动压缩会话历史（`/compact`）。完成时走事件流的 compacted。 */
export function compactSession(sessionId: string): Promise<void> {
  return invoke("session_compact", { sessionId });
}

/** 排队面板的一条插话摘要。images 是图片张数（全量 base64 回传太重）。 */
/** 排队面板的一条插话摘要。生成类型的别名，同 Protocol 的理由。 */
export type QueuedSummary = GeneratedQueuedSummary;

/** 当前排着的插话。切回会话时重建排队面板用。 */
export function queueList(sessionId: string): Promise<QueuedSummary[]> {
  return invoke<QueuedSummary[]>("queue_list", { sessionId });
}

/** 删一条排队插话。false = 条目已经不在（被注入或早被删了）。 */
export function queueRemove(sessionId: string, entryId: string): Promise<boolean> {
  return invoke<boolean>("queue_remove", { sessionId, entryId });
}

/** 撤回一条排队插话，拿回原始输入（放回输入框编辑）。 */
export function queueTake(
  sessionId: string,
  entryId: string,
): Promise<{ text: string; images: ImageInput[]; refs: string[] } | null> {
  return invoke<{ text: string; images: ImageInput[]; refs: string[] } | null>("queue_take", {
    sessionId,
    entryId,
  });
}

/** 随消息附上的一张图。data 是 base64，不含 `data:` 前缀。
 *  生成类型的别名，同 Protocol 的理由。 */
export type ImageInput = GeneratedImageInput;

/** 读一个图片文件（拖进来的、或从对话框选的）。太大或类型不认时 reject。 */
export function readImage(path: string): Promise<ImageInput & { name: string }> {
  return invoke("read_image", { path });
}

/**
 * 读一个文件的原始字节，给应用内预览用。
 *
 * 宿主走 Tauri 的二进制通道返回（不经 JSON / base64），这里拿到的就是
 * ArrayBuffer。超过宿主的预览上限（128 MB）或读不到时 reject，调用方
 * 把错误文案摆出来并给"用系统应用打开"的退路。
 */
export function readFileBytes(path: string): Promise<ArrayBuffer> {
  return invoke<ArrayBuffer>("read_file_bytes", { path });
}

/**
 * 弹系统的文件选择框，返回绝对路径。
 *
 * 只回路径不回内容:选中的非图片文件在输入框里变成 `@` 引用块，内容由
 * 宿主在发送时按上限读取（见 src-tauri 的 mentions）。这里读内容的话，
 * 一个几 MB 的文件会先在前端过一遍内存、再走一次 IPC，而它多半还要被
 * 截断。
 */
export async function pickFiles(imagesOnly = false): Promise<string[]> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  // 分开写而不是塞一个 undefined：tsconfig 开了 exactOptionalPropertyTypes，
  // 显式的 undefined 和"不传"是两件事。
  const picked = imagesOnly
    ? await open({
      multiple: true,
      filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg", "gif", "webp"] }],
    })
    : await open({ multiple: true });
  if (!picked) return [];
  return Array.isArray(picked) ? picked : [picked];
}

/**
 * 中断当前轮。内核会补齐所有悬空的 tool_result，见 invariants::check_tool_pairing。
 *
 * 返回是否真的取消了一轮。`false` = 宿主已经闲着，界面上的忙碌是残留，
 * 调用方该把停止键收掉。
 */
export function interrupt(sessionId: string): Promise<boolean> {
  return invoke<boolean>("interrupt", { sessionId });
}

/**
 * 本次会话改了哪些文件、哪些行（输入框上方的改动条）。
 *
 * 只含经 Edit / Write 落下的改动，基线是会话自己记的 —— 所以 commit
 * 之后它还在，回答的是"这个会话动了什么"，不是"工作区还有什么没提交"。
 */
export function sessionChanges(sessionId: string): Promise<FileChange[]> {
  return invoke<FileChange[]>("session_changes", { sessionId });
}

/**
 * 工作区相对所选基线的差异（侧边抽屉的 Git 面板）。
 *
 * `base` 空 = 当前分支 / HEAD。只换对比对象，不 checkout。
 * 跟着 git 走：包含用户自己的手改、bash 写盘、重命名检测。
 * 会话视角的净改动看上面的 `sessionChanges`。
 */
export function sessionGitChanges(sessionId: string, base?: string): Promise<GitChanges> {
  return invoke<GitChanges>(
    "session_git_changes",
    base ? { sessionId, base } : { sessionId },
  );
}

/** 回应一个权限询问。askId 来自 PermissionRequest 事件。 */
export function respondPermission(
  sessionId: string,
  askId: string,
  response: PermissionResponse,
): Promise<void> {
  return invoke("respond_permission", { sessionId, askId, response });
}

/** 会话级采样覆盖。空字段继承下面几层，`null` 表示这一项干脆不发；下一轮生效。 */
export function setSessionSampling(sessionId: string, sampling: Sampling): Promise<void> {
  return invoke("set_session_sampling", { sessionId, sampling });
}

/**
 * 探测会话根目录下的常见虚拟环境（.venv / venv）。
 * 系统选择框默认藏起点开头的目录，探测结果用来做一键填入。
 */
export function detectVenvs(sessionId: string): Promise<string[]> {
  return invoke<string[]>("detect_venvs", { sessionId });
}

/**
 * 会话的 Python 虚拟环境。空字符串清除；下一轮生效。
 * 宿主会验证目录里有没有 `bin/python`，不像 venv 时 reject。
 */
export function setSessionPythonVenv(sessionId: string, path: string): Promise<void> {
  return invoke("set_session_python_venv", { sessionId, path });
}

/** 会话级追加的系统提示词（附在内置提示词之后）。空字符串清除；下一轮生效。 */
export function setSessionSystemPrompt(sessionId: string, prompt: string): Promise<void> {
  return invoke("set_session_system_prompt", { sessionId, prompt });
}

/** 会话级思考策略。下一轮生效。 */
export function setSessionThinking(sessionId: string, thinking: ThinkingPolicy): Promise<void> {
  return invoke("set_session_thinking", { sessionId, thinking });
}

export function setPermissionMode(
  sessionId: string,
  mode: PermissionMode,
): Promise<void> {
  return invoke("set_permission_mode", { sessionId, mode });
}

/** 当前安装的版本，和 `tauri.conf.json` 同一份。 */
export function appVersion(): Promise<string> {
  return invoke<string>("app_version");
}

/** 对照 GitHub 最新正式 Release。 */
export interface UpdateInfo {
  current: string;
  latest: string | null;
  notes: string | null;
  /** 当前平台的安装包，没有就给 Release 页。 */
  url: string;
  newer: boolean;
}

export function checkUpdate(): Promise<UpdateInfo> {
  return invoke<UpdateInfo>("check_update");
}

export function getConfig(): Promise<ConfigStatus> {
  return invoke<ConfigStatus>("get_config");
}

export function setConfig(config: AppConfig): Promise<ConfigStatus> {
  return invoke<ConfigStatus>("set_config", { config });
}

/** 保存某个 provider 的 API key（宿主写进 0600 的 auth.json）。空字符串删除。 */
export function setApiKey(providerId: string, key: string): Promise<ConfigStatus> {
  return invoke<ConfigStatus>("set_api_key", { providerId, key });
}

/** 拉取某个 provider 的可用模型列表（GET /v1/models）。 */
export function listModels(providerId: string): Promise<string[]> {
  return invoke<string[]>("list_models", { providerId });
}

/* ── MCP 与 Skills ─────────────────────────── */

/** 一个 MCP 服务器此刻的连接状态。 */
/**
 * MCP 连接状态快照，给设置页看。
 *
 * 从生成类型派生而不是另写一份：字段增删跟着 `pnpm gen` 走。只把
 * `state` 收窄成三个字面量 —— Rust 那边是个 enum，但 JSON Schema
 * 落到 TS 只剩 `string`，界面要靠它分支画状态点。
 */
export type McpServerStatus = Omit<GeneratedMcpServerStatus, "state"> & {
  state: "connecting" | "connected" | "failed";
};

/** MCP 服务器的连接状态。设置页轮询它显示状态点和工具数。 */
export function mcpStatus(): Promise<McpServerStatus[]> {
  return invoke<McpServerStatus[]>("mcp_status");
}

/** 手动重连一个 MCP 服务器。 */
export function mcpRestart(serverId: string): Promise<void> {
  return invoke("mcp_restart", { serverId });
}

/** 当前 MCP 服务器的标准 JSON（`{"mcpServers": {...}}`，各家 README 的通用格式）。 */
export function mcpExportJson(): Promise<string> {
  return invoke<string>("mcp_export_json");
}

/**
 * 用标准 JSON 整体替换 MCP 服务器配置。宿主负责解析与校验；
 * 支持 Claude Desktop / Cursor / Cline / VS Code 的形状。
 */
export function mcpImportJson(raw: string): Promise<ConfigStatus> {
  return invoke<ConfigStatus>("mcp_import_json", { raw });
}

/** 一个技能（或一个解析失败的 SKILL.md，带原因）。 */
export interface SkillInfo {
  name: string;
  description: string;
  /** SKILL.md 的完整路径。内置技能编在二进制里，没有路径，给空串。 */
  path: string;
  /** `builtin` 随应用分发；`pack` 来自能力包；`global` / `project` 是用户写的。 */
  source: "builtin" | "pack" | "global" | "project";
  /** 解析失败的原因。没有 = 可用。 */
  error?: string | null;
}

/** 当前可用的技能清单。`root` 传当前会话的项目根；null 只列全局。 */
export function skillsList(root: string | null): Promise<SkillInfo[]> {
  return invoke<SkillInfo[]>("skills_list", { root });
}

/** 平台支持 OS 级隔离、但当前用不了的原因。 */
export type SandboxBlocker =
  /** 差一次需要管理员的一次性安装。前端据此显示安装入口。 */
  | { kind: "needsElevatedInstall" }
  /** 装过但探测不通（找不到 srt-win、装了一半）。原因原样端出来，否则没法查。 */
  | { kind: "broken"; error: string };

/**
 * OS 级隔离在这台机器上**现在**能不能用。
 *
 * 和配置里的 `sandbox` 字段是两回事：那个是意图，这个是现实。Windows 上
 * 没装时两者会分叉 —— 意图开着、现实没有，命令照常裸跑（安全，但用户会
 * 莫名其妙多点很多确认框）。
 */
export interface SandboxStatus {
  /** 这个平台实现了没有。false = 代码里就没这一半，和用户装没装无关。 */
  implemented: boolean;
  /** 现在这一刻能不能真隔离。 */
  ready: boolean;
  /** 「隔离并断网」那档能不能真断网。false 时选它会整档退回不隔离。 */
  networkIsolation: boolean;
  /** `implemented && !ready` 时差的是什么。 */
  blocker: SandboxBlocker | null;
}

/** 探一次沙箱的真实可用性。只查不改，随时可调。 */
export function sandboxStatus(): Promise<SandboxStatus> {
  return invoke<SandboxStatus>("sandbox_status");
}

/**
 * 跑那次一次性的提权安装。**会弹两次系统权限确认（UAC）。**
 *
 * 两步是分开提权的：第一步建专用账户，第二步摘掉出网栅栏（不摘的话沙箱内
 * 彻底断网）。调用前要让用户知道会弹两次，否则第二个弹窗看起来像出了问题。
 *
 * 慢，而且慢在等用户点对话框 —— 没有超时，取消会以一条给人看的话 reject。
 */
export function sandboxInstall(): Promise<void> {
  return invoke("sandbox_install");
}

/**
 * 卸载命令隔离：删掉沙箱专用账户与凭证。Windows 上会弹一次 UAC。
 *
 * 和安装一样慢在等用户点对话框；取消会以一条给人看的话 reject。
 */
export function sandboxUninstall(): Promise<void> {
  return invoke("sandbox_uninstall");
}

/** 一个可下载的能力包。 */
export interface PackStatus {
  id: string;
  name: string;
  description: string;
  /** 已装版本。null = 没装。 */
  installedVersion: string | null;
  /** 远端可装版本。null = 清单没拉到，或这个平台没有包。 */
  availableVersion: string | null;
  /** 下载体积，字节。 */
  downloadSize: number;
  /** 解压后体积，字节。 */
  installedSize: number;
  supported: boolean;
  /** 清单拉取失败的原因。有值时显示"离线"，而不是"没有可用更新"。 */
  manifestError: string | null;
}

/** 安装进度。各阶段耗时差着数量级，所以分开报而不是合成一个百分比。 */
export type PackProgress =
  | { kind: "downloading"; received: number; total: number }
  | { kind: "verifying" }
  | { kind: "extracting" }
  | { kind: "selfCheck" }
  | { kind: "done"; version: string }
  | { kind: "failed"; error: string };

/** 能力包清单：装了什么、有什么可装。 */
export function packsStatus(): Promise<PackStatus[]> {
  return invoke<PackStatus[]>("packs_status");
}

/** 下载并安装一个能力包。装完即可用，不需要重启。 */
export function packsInstall(
  id: string,
  onProgress: (p: PackProgress) => void,
): Promise<void> {
  const channel = new Channel<PackProgress>();
  channel.onmessage = onProgress;
  return invoke("packs_install", { id, onProgress: channel });
}

/** 卸载一个能力包，连带摘掉它注册的 MCP 服务器。 */
export function packsUninstall(id: string): Promise<void> {
  return invoke("packs_uninstall", { id });
}

/** 登记一个项目目录（验证并规范化），返回 canonical 根。不创建会话。 */
export function addProject(path: string): Promise<string> {
  return invoke<string>("add_project", { path });
}

/** 在某个项目下开新会话。返回的会话从创建起绑定这个目录。 */
export function createSession(root: string): Promise<SessionInfo> {
  return invoke<SessionInfo>("create_session", { root });
}

/** 哪些路径现在不是目录。只看、不改配置。侧栏用来标失效项目。 */
export function probeDirs(paths: string[]): Promise<string[]> {
  return invoke<string[]>("probe_dirs", { paths });
}

/** 所有活着的会话。启动或刷新后用它对齐侧边栏。 */
export function listSessions(): Promise<SessionInfo[]> {
  return invoke<SessionInfo[]>("list_sessions");
}

/**
 * 一个会话的完整历史 + 此刻是否有轮子在跑。切回会话时重建对话流。
 *
 * 忙碌状态跟着历史一起回：分两次问会在中间留一个窗口，那一瞬间界面
 * 显示空闲（没有停止键），而模型正在干活。
 */
export interface HistorySnapshot {
  messages: Message[];
  /** 压缩边界之前的消息。模型看不见，界面画在分割线上面。 */
  archived: Message[];
  busy: boolean;
  compacting: boolean;
  /** 还在等用户回答的权限询问。切回会话时靠它重建弹窗。 */
  pendingAsks: PendingAsk[];
  /** 正在流式生成的正文。流式增量不进历史，切回来靠它接着显示。 */
  liveText: string;
  /** 正在流式生成的思考。缺了它思考块的字数会清零重数。 */
  liveThinking: string;
}

export function getHistory(sessionId: string): Promise<HistorySnapshot> {
  return invoke<HistorySnapshot>("get_history", { sessionId });
}

/** 删除会话（正在跑的轮子会被中断）。幂等。 */
export function deleteSession(sessionId: string): Promise<void> {
  return invoke("delete_session", { sessionId });
}

/** 重命名会话。空标题清除手动名，回退到第一条消息。 */
export function renameSession(sessionId: string, title: string): Promise<void> {
  return invoke("rename_session", { sessionId, title });
}

/**
 * 把项目从列表移除，连带关闭它下面的会话。**不删磁盘上的目录。**
 * 返回被关闭的会话 id。
 */
export function removeProject(root: string): Promise<string[]> {
  return invoke<string[]>("remove_project", { root });
}

/* ── 内置浏览器面板 ─────────────────────────── */

/** 一帧画面。`data` 是 JPEG 的原始字节;宽高是 CSS 像素（坐标换算用它）。 */
export interface BrowserFrame {
  /** 标注 ArrayBuffer 后端:默认的 ArrayBufferLike 进不了 Blob（TS 5.7+）。 */
  data: Uint8Array<ArrayBuffer>;
  width: number;
  height: number;
}

/**
 * 面板发给页面的输入。
 *
 * 坐标是**页面坐标**（相对视口左上角的 CSS 像素）。面板负责把自己的 DOM
 * 坐标换算过来 —— 它知道当前缩放比例，宿主不知道。
 */
export type BrowserInput =
  | { kind: "click"; x: number; y: number; button: string }
  /** 鼠标按下/抬起。面板转发这两条（而不是合成的 click），页面里才能
   *  拖拽选字、拖滑块、双击选词。clickCount 让双击/三击成立。 */
  | { kind: "down"; x: number; y: number; button: string; clickCount: number }
  | { kind: "up"; x: number; y: number; button: string; clickCount: number }
  | { kind: "move"; x: number; y: number }
  /** 两个轴都要发。页面通常比面板宽，只发 deltaY 的话右边那截永远看不到。 */
  | { kind: "scroll"; x: number; y: number; deltaX: number; deltaY: number }
  | { kind: "text"; text: string }
  /** 输入法正在组字，text 是还没上屏的临时内容；空串表示取消。 */
  | { kind: "compose"; text: string }
  | { kind: "key"; key: string };

/** 标签栏上的一页。 */
export interface TabInfo {
  id: number;
  /** 页面地址。空 = 停在空白页，也就是"新标签页"。 */
  url: string;
  /** 页面标题。加载完之前是空的。 */
  title: string;
  canBack: boolean;
  canForward: boolean;
}

/**
 * 面板要显示的全部状态。
 *
 * 标签栏和工具栏一起回:它们描述的是同一个时刻。分两条查询的话，切标签的
 * 瞬间会出现"标签栏已经高亮了新页、地址栏还是旧页"这种自相矛盾的中间态。
 */
export interface PanelState {
  tabs: TabInfo[];
  /** 当前显示的那一页。没有标签页时是 0。 */
  active: number;
}

/**
 * 打开面板，开始接收画面。
 *
 * 返回的 unsubscribe 只停止本地分发；要让宿主停止编码必须调
 * {@link closeBrowser} —— 没人看的时候继续推是白烧 CPU。
 */
export function openBrowser(
  sessionId: string,
  onFrame: (f: BrowserFrame) => void,
  /**
   * 浏览器就绪时的标签栏状态。
   *
   * 有它才不用等下一次定时同步 —— 浏览器起来要一秒，再叠一个轮询间隔的话，
   * 用户看到的是"开了面板、空等两秒、才冒出一个标签页"。
   */
  onReady?: (s: PanelState) => void,
): Subscription {
  let active = true;
  // 帧是二进制:8 字节小端头（宽、高，各 u32）+ JPEG 字节，和宿主的
  // browser_open 对齐。不走 JSON —— 每帧几百 KB 的 base64 要在主线程上
  // JSON.parse，正是滚动时和输入事件抢时间的那一刀。
  const channel = new Channel<ArrayBuffer>();
  channel.onmessage = (buf) => {
    if (!active) return;
    const head = new DataView(buf);
    onFrame({
      width: head.getUint32(0, true),
      height: head.getUint32(4, true),
      data: new Uint8Array(buf, 8),
    });
  };
  const ready = invoke<PanelState>("browser_open", { sessionId, onFrame: channel }).then(
    (s) => {
      if (active) onReady?.(s);
    },
    (e: unknown) => {
      if (active) console.error("打开浏览器面板失败", e);
    },
  );
  return {
    unsubscribe() {
      active = false;
    },
    ready,
  };
}

export function closeBrowser(sessionId: string): Promise<void> {
  return invoke("browser_close", { sessionId });
}

export function browserNavigate(sessionId: string, url: string): Promise<void> {
  return invoke("browser_navigate", { sessionId, url });
}

/** 在历史里走一步：-1 后退，+1 前进。返回当前页走完之后的状态。 */
export function browserHistory(sessionId: string, delta: number): Promise<TabInfo> {
  return invoke<TabInfo>("browser_history", { sessionId, delta });
}

export function browserReload(sessionId: string): Promise<void> {
  return invoke("browser_reload", { sessionId });
}

/** 标签栏 + 工具栏的状态。页面自己跳转时只能靠问，没有通知。 */
export function browserState(sessionId: string): Promise<PanelState> {
  return invoke<PanelState>("browser_state", { sessionId });
}

/** 新开一个标签页并切过去。 */
export function browserNewTab(sessionId: string): Promise<PanelState> {
  return invoke<PanelState>("browser_new_tab", { sessionId });
}

/** 关一个标签页。关掉最后一个就是空清单，不补新页 —— 界面据此收掉
 *  浏览器标签。 */
export function browserCloseTab(sessionId: string, tab: number): Promise<PanelState> {
  return invoke<PanelState>("browser_close_tab", { sessionId, tab });
}

/** 切到某个标签页。画面、工具栏和模型的浏览器工具都跟着它。 */
export function browserSelectTab(sessionId: string, tab: number): Promise<PanelState> {
  return invoke<PanelState>("browser_select_tab", { sessionId, tab });
}

/**
 * 订阅标签清单的变更（开 / 关 / 切页）。通知不带内容 —— 收到就该立刻
 * 重查一次 {@link browserState}，新标签才能和画面同时出现，不用等下
 * 一拍轮询。返回退订函数（只停本地分发；宿主的通道在下次订阅时替换，
 * 取舍见宿主 browser_watch_tabs 的说明）。
 */
export function watchBrowserTabs(sessionId: string, onChange: () => void): () => void {
  let live = true;
  const channel = new Channel<boolean>();
  channel.onmessage = () => {
    if (live) onChange();
  };
  invoke("browser_watch_tabs", { sessionId, onChange: channel }).catch((e: unknown) => {
    // 会话没了、浏览器起不来 —— 标签栏退化成纯轮询，不值得打扰用户。
    console.warn("订阅浏览器标签变更失败", e);
  });
  return () => {
    live = false;
  };
}

/**
 * 告诉宿主画面区现在多大（CSS 像素）以及屏幕的像素密度，页面视口会跟着变。
 *
 * 尺寸不同步的话，帧的比例和面板的比例对不上，画面周围会留出黑边 —— 而且
 * 页面是被整体缩小塞进来的，字会小到看不清。
 *
 * 密度不同步的话，尺寸和比例都是对的，只是糊:帧按一倍出，面板要按两倍铺，
 * 中间那次放大让所有文字的边缘发虚。
 */
export function browserResize(
  sessionId: string,
  width: number,
  height: number,
  scale: number,
  /** 面板画面区 CSS 像素。推流按它封顶，页面视口仍是 width×height。 */
  viewWidth: number,
  viewHeight: number,
): Promise<void> {
  return invoke("browser_resize", {
    sessionId,
    width,
    height,
    scale,
    viewWidth,
    viewHeight,
  });
}

export function browserInput(sessionId: string, input: BrowserInput): Promise<void> {
  return invoke("browser_input", { sessionId, input });
}

/** 面板取件的结果：用户在面板里点中的那个元素。 */
export interface PickResult {
  /** 能直接喂给模型的 CSS 选择器。 */
  selector: string;
  /** 给用户看的一句话（标签、id、类、截断的文字）。 */
  description: string;
}

/**
 * 取件结果借用 Composer 现成的 `insertText` 通道送进输入框（省得再从
 * App 往下多穿一层 prop）。用一个不可能出现在正常文本里的前缀把结构化的
 * PickResult 编进字符串，Composer 认出前缀就渲染成"元素色块"，认不出就
 * 照旧当普通文本插入。前缀用 NUL —— 用户不可能打出来。
 */
const PICK_SENTINEL = "\u0000riot-pick\u0000";

export function encodePickForComposer(p: PickResult): string {
  return PICK_SENTINEL + JSON.stringify(p);
}

/** 认不出前缀返回 null（那就是普通 insertText，走原来的围栏逻辑）。 */
export function decodePickFromComposer(s: string): PickResult | null {
  if (!s.startsWith(PICK_SENTINEL)) return null;
  try {
    return JSON.parse(s.slice(PICK_SENTINEL.length)) as PickResult;
  } catch {
    return null;
  }
}

/**
 * 面板取件：把面板里的一个坐标命中测试成元素的选择器 + 描述。
 * 坐标是视口 CSS 坐标（和 browserInput 同一套）。点不到元素返回 null。
 */
export function browserPick(
  sessionId: string,
  x: number,
  y: number,
): Promise<PickResult | null> {
  return invoke<PickResult | null>("browser_pick", { sessionId, x, y });
}

/** 取件模式下鼠标移动：把高亮框移到光标下的元素上（前端节流后再调）。 */
export function browserPickHover(sessionId: string, x: number, y: number): Promise<void> {
  return invoke("browser_pick_hover", { sessionId, x, y });
}

/** 撤掉取件高亮框（退出取件模式 / 鼠标离开画面 / 点选完成）。 */
export function browserPickClear(sessionId: string): Promise<void> {
  return invoke("browser_pick_clear", { sessionId });
}

/** 本会话已授权的渗透 scope（host 列表）。给 scope 管理面板看。 */
export function browserScopeList(sessionId: string): Promise<string[]> {
  return invoke<string[]>("browser_scope_list", { sessionId });
}

/** 撤销一个渗透 scope 授权。之后对该目标的侵入性动作会重新要求授权。 */
export function browserScopeRevoke(sessionId: string, host: string): Promise<void> {
  return invoke("browser_scope_revoke", { sessionId, host });
}

/* ── 底部终端面板 ───────────────────────────── */

/**
 * 宿主推来的终端事件。
 *
 * `data` 是 base64 的原始字节 —— 输出的 chunk 边界随时可能切在一个 UTF-8
 * 序列中间，按字符串传会把半个字变成替换符。解码成 Uint8Array 交给 xterm，
 * 它自带跨 chunk 的解码器。
 */
export type TermEvent = { kind: "data"; data: string } | { kind: "exit" };

/**
 * 开一个终端：在 `root` 目录起用户的默认 shell。返回终端 id。
 *
 * `root` 传 null 或目录不存在时退回家目录 —— 终端还是要开，只是位置
 * 不理想。开不出来（PTY 分配失败）才 reject。
 */
export function termOpen(
  root: string | null,
  cols: number,
  rows: number,
  onEvent: (ev: TermEvent) => void,
): Promise<number> {
  const channel = new Channel<TermEvent>();
  channel.onmessage = onEvent;
  return invoke<number>("term_open", { root, cols, rows, onEvent: channel });
}

/** 一个已经存在的终端。 */
export interface TermSummary {
  id: number;
  title: string;
  /** 起它的命令。模型起的服务才有；用户自己开的 shell 是 null。 */
  command: string | null;
  running: boolean;
  /** 用户把这个终端交给模型看了。 */
  shared: boolean;
  /** 起它的会话（模型起的服务才有）。用户自己开的 shell 是 null。 */
  owner: string | null;
}

/**
 * 把一个终端交给模型看 / 收回来。
 *
 * 只有用户能调这条路 —— 模型侧没有对应接口，它不能给自己开权限。
 * 共享只给读：它读得到输出，但停不掉这个终端。
 */
export function termShare(id: number, shared: boolean): Promise<void> {
  return invoke("term_share", { id, shared });
}

/** 现有的终端。面板重建标签栏、发现模型起了新服务，都靠它。 */
export function termList(): Promise<TermSummary[]> {
  return invoke<TermSummary[]>("term_list");
}

/**
 * 挂到一个已经在跑的终端上，并回放它已有的输出。
 *
 * 模型起的服务在面板打开之前就在跑了 —— 没有这条，那些输出永远到不了
 * 用户眼前。
 */
export function termAttach(id: number, onEvent: (ev: TermEvent) => void): Promise<void> {
  const channel = new Channel<TermEvent>();
  channel.onmessage = onEvent;
  return invoke("term_attach", { id, onEvent: channel });
}

/** 键盘输入原样打进 shell。`data` 是 xterm 的 onData 给的串（含控制序列）。 */
export function termWrite(id: number, data: string): Promise<void> {
  return invoke("term_write", { id, data });
}

/** 终端区尺寸变了，PTY 跟着变 —— 不同步的话 shell 按旧宽度折行。 */
export function termResize(id: number, cols: number, rows: number): Promise<void> {
  return invoke("term_resize", { id, cols, rows });
}

/** 关一个终端（杀掉 shell）。幂等。 */
export function termClose(id: number): Promise<void> {
  return invoke("term_close", { id });
}

/** 这个终端的前台有没有正在跑的进程。关标签前的确认用。 */
export function termBusy(id: number): Promise<boolean> {
  return invoke<boolean>("term_busy", { id });
}

/** 在系统文件管理器（访达/资源管理器）里显示这个目录。 */
export async function revealInFinder(path: string): Promise<void> {
  const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
  await revealItemInDir(path);
}

/**
 * 用系统默认应用打开这个文件 —— 对源码文件通常就是用户的编辑器。
 *
 * 打不开（路径不存在、没有关联应用）时静默失败：这是点一下代码引用
 * 的顺手操作，弹一个错误对话框比没反应更烦人。
 */
export async function openInDefaultApp(path: string): Promise<void> {
  try {
    await openPath(path);
  } catch (e) {
    console.warn("打不开这个文件", path, e);
  }
}

/** 用系统默认应用打开路径。失败会抛，调用方自己决定怎么告诉用户。 */
export async function openPath(path: string): Promise<void> {
  // opener 插件是分离式启动，目标不存在它也报成功。先自己查一遍，
  // 让"文件不存在"成为看得见的失败，而不是点了没反应。
  if (!(await invoke<boolean>("path_exists", { path }))) {
    throw new Error(`文件不存在：${path}`);
  }
  const { openPath: open } = await import("@tauri-apps/plugin-opener");
  await open(path);
}

/** 用系统浏览器打开网址。失败会抛。 */
export async function openInBrowser(url: string): Promise<void> {
  const { openUrl } = await import("@tauri-apps/plugin-opener");
  await openUrl(url);
}

/**
 * 发一条系统通知。权限只在第一次要 —— 被拒绝就永远沉默，不反复骚扰。
 *
 * 失败静默：通知是锦上添花，平台不支持、用户拒绝、插件没装都不值得
 * 打断调用方。文案由调用方给，这层只负责把能力接出来。
 */
export async function notify(title: string, body: string): Promise<void> {
  try {
    const { isPermissionGranted, requestPermission, sendNotification } = await import(
      "@tauri-apps/plugin-notification"
    );
    let ok = await isPermissionGranted();
    if (!ok) ok = (await requestPermission()) === "granted";
    if (ok) sendNotification({ title, body });
  } catch {
    // 平台不支持或用户拒绝 —— 无声跳过
  }
}

/** 弹系统的目录选择框。`defaultPath` 指定起始目录（如会话根）。 */
export async function pickDirectory(defaultPath?: string): Promise<string | null> {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({
    directory: true,
    multiple: false,
    ...(defaultPath ? { defaultPath } : {}),
  });
  return typeof picked === "string" ? picked : null;
}

/**
 * 发一个最小请求，验证 base URL / key / 模型名。不传参数测当前激活的；
 * 设置页传"正在编辑的那个"。成功返回一句人话；失败时错误信息里已带原因。
 */
export function testConnection(providerId?: string, model?: string): Promise<string> {
  return invoke<string>("test_connection", {
    providerId: providerId ?? null,
    model: model ?? null,
  });
}

/**
 * 测搜索后端通不通。传正在编辑的地址；空 = 测内置，不用先保存。
 *
 * 会真发一次查询而不是只打首页 —— 首页 200 说明不了 JSON 输出开没开，
 * 而那正是最容易配错的一处。
 */
export function testSearchBackend(baseUrl: string): Promise<string> {
  return invoke<string>("test_search_backend", { baseUrl });
}

/**
 * 剪贴板上的文件（绝对路径）。没有文件、或不是 macOS 时是空数组。
 *
 * 为什么不用 `ClipboardEvent.clipboardData`:那里的 `File` 对象没有磁盘
 * 路径，webview 出于沙箱安全从不给。而非图片文件要变成 `@引用`,引用认
 * 的就是路径。
 */
export function clipboardPaths(): Promise<string[]> {
  return invoke<string[]>("clipboard_paths");
}

/** 拖到窗口上的那一批文件。`paths` 空 = 拖来的东西在磁盘上没有文件。 */
export interface DragDrop {
  kind: "enter" | "over" | "leave" | "drop";
  paths: string[];
}

/**
 * 窗口级的文件拖放（整个窗口都是落点，不只是输入框那一条）。
 *
 * 走 Tauri 的原生拖放事件而不是 HTML5 的 `ondrop`:后者给的 `File` 没有
 * 磁盘路径，非图片文件就没法变成引用。代价是 webview 里的 HTML5 拖放事件
 * 全被原生层吃掉 —— 从浏览器直接拖一张图（磁盘上没有那个文件）这条路
 * 断了，那种图改用复制粘贴，见 `clipboardPaths`。
 */
export function subscribeDragDrop(cb: (e: DragDrop) => void): () => void {
  let stopped = false;
  let unlisten: (() => void) | undefined;
  void (async () => {
    try {
      const { getCurrentWebview } = await import("@tauri-apps/api/webview");
      unlisten = await getCurrentWebview().onDragDropEvent((e) => {
        if (stopped) return;
        const p = e.payload;
        cb({ kind: p.type, paths: p.type === "enter" || p.type === "drop" ? p.paths : [] });
      });
      if (stopped) unlisten();
    } catch {
      // 不在 Tauri 里（纯浏览器、组件测试）就没有拖放。
    }
  })();
  return () => {
    stopped = true;
    unlisten?.();
  };
}

/** 窗口标题跟随当前项目 —— 多开窗口时用户靠标题分辨哪个是哪个。 */
export async function setWindowTitle(title: string): Promise<void> {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().setTitle(title);
}

/**
 * 窗口是否处于系统全屏。
 *
 * macOS Overlay 标题栏的红绿灯只在窗口态占左上角；进全屏它们消失，
 * 侧栏收起时那 84px 让位就成了空白。尺寸变化时重问一次 —— Tauri 没有
 * 单独的 fullscreen 事件，resize 覆盖进/出全屏。
 */
export function subscribeFullscreen(cb: (full: boolean) => void): () => void {
  let stopped = false;
  let unlisten: (() => void) | undefined;
  void (async () => {
    try {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      const win = getCurrentWindow();
      const emit = async () => {
        if (stopped) return;
        cb(await win.isFullscreen());
      };
      await emit();
      unlisten = await win.onResized(() => void emit());
      if (stopped) unlisten();
    } catch {
      // 不在 Tauri 里（纯浏览器、组件测试）就当没全屏。
    }
  })();
  return () => {
    stopped = true;
    unlisten?.();
  };
}

/**
 * 原生窗口焦点变化。比 window 的 "focus" 事件多覆盖一类场景：
 * 睡眠唤醒时 WebView 可能不发 DOM focus（窗口一直"可见"），
 * 只有 Tauri 层这个事件可靠。
 */
export function subscribeWindowFocus(cb: (focused: boolean) => void): () => void {
  let stopped = false;
  let unlisten: (() => void) | undefined;
  void (async () => {
    try {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      unlisten = await getCurrentWindow().onFocusChanged(({ payload }) => {
        if (!stopped) cb(payload);
      });
      if (stopped) unlisten();
    } catch {
      // 不在 Tauri 里（纯浏览器、组件测试）。DOM 的 focus 监听兜底。
    }
  })();
  return () => {
    stopped = true;
    unlisten?.();
  };
}
