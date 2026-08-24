/* 由 schemas/protocol.json 生成，勿手改。改 crates/riot-protocol 里的 Rust 类型后跑 pnpm gen */

export type AgentEvent =
  | {
      after?: Transition | null;
      model: string;
      turn: number;
      type: "request_start";
    }
  | StreamDelta
  | Message
  | {
      payload: ProgressPayload;
      tool_use_id: string;
      type: "progress";
    }
  | {
      detail: PermissionAsk;
      request_id: string;
      type: "permission_request";
    }
  | {
      reason: DecisionReason;
      request_id: string;
      type: "permission_resolved";
    }
  | {
      type: "compacting";
    }
  | {
      after_tokens: number;
      before_tokens: number;
      strategy: CompactStrategy;
      type: "compacted";
    }
  | {
      mode: PermissionMode;
      type: "mode_changed";
    }
  | {
      message_id: string;
      /**
       * 撤回之后这个会话一条消息都不剩了。
       *
       * 宿主据此把自动标题一并撤掉：标题正是从这条消息取的，留着就是
       * 一个空会话顶着一句从没发出去的话，而且之后真正的第一句话
       * 再也改不动它了。
       */
      session_empty: boolean;
      type: "prompt_withdrawn";
    }
  | {
      reason: TerminalReason;
      type: "done";
    };
/**
 * 一轮结束后为什么继续。
 *
 * `[约束]` 每次主循环 `continue` 之前必须设置它，并带进下一个
 * [`AgentEvent::RequestStart`]。这是把「恢复路径」变成可观测行为的唯一手段。
 *
 * 放在 protocol 而不是 core，是因为前端也要用 —— 用户需要知道
 * 「转了 30 秒是因为在压缩上下文」，而不是以为卡住了。
 */
export type Transition =
  "next_turn" | "reactive_compact_retry" | "output_limit_recovery" | "stop_hook_blocking" | "token_budget_nudge";
/**
 * 流式增量。高频（每秒可能上百条），仅用于打字机效果。
 *
 * **不进 transcript，黄金回放测试也会忽略它** —— 断言 Delta
 * 会让用例极其脆弱，改一点流式切分逻辑就全红。
 */
export type StreamDelta = {
  type: "delta";
} & (
  | {
      kind: "text";
      message_id: string;
      text: string;
    }
  | {
      kind: "thinking";
      message_id: string;
      text: string;
    }
  | {
      kind: "tool_start";
      name: string;
      tool_use_id: string;
    }
  | {
      kind: "tool_input";
      partial_json: string;
      tool_use_id: string;
    }
);
/**
 * 一条完整消息。可持久化、可回放、可送回模型。
 */
export type Message = {
  type: "message";
} & (
  | {
      content: UserContent[];
      id: string;
      meta?: MessageMeta;
      role: "user";
    }
  | {
      content: AssistantContent[];
      id: string;
      meta?: MessageMeta;
      role: "assistant";
      usage?: Usage | null;
    }
  | {
      id: string;
      level: SystemLevel;
      role: "system";
      text: string;
    }
);
export type UserContent =
  | {
      text: string;
      type: "text";
    }
  | {
      content: ToolResultContent;
      is_error: boolean;
      tool_use_id: string;
      type: "tool_result";
    }
  | Attachment;
export type ToolResultContent =
  | {
      text: string;
      type: "text";
    }
  | {
      path: string;
      preview: string;
      total_bytes: number;
      type: "spilled";
    }
  | {
      type: "cleared";
    }
  | {
      data: string;
      /**
       * `data` 的类型（压缩产物通常是 image/jpeg），不一定等于原图类型。
       */
      media_type: string;
      /**
       * 原图的位置：截图落盘的文件，或被读的图片本身。界面优先按它
       * 显示原图（清晰、可另存）；`None` 表示没落成盘，界面显示
       * `data` 里的压缩图兜底。模型不用这个字段。
       */
      path?: string | null;
      type: "image";
    }
  | {
      /**
       * 压缩图。界面在 `path` 缺失时用它兜底显示。
       */
      data: string;
      /**
       * `data` 的类型（压缩产物通常是 image/jpeg），不一定等于原图类型。
       */
      media_type: string;
      /**
       * 原图位置，语义同 [`Image::path`](ToolResultContent::Image)。
       */
      path?: string | null;
      /**
       * 给模型的转述，自带"当作亲眼所见"的使用指示。
       */
      text: string;
      type: "described_image";
    }
  | {
      data: string;
      media_type: string;
      path?: string | null;
      /**
       * 配套文字。Set-of-Marks 场景是编号清单（编号同
       * [`crate::browser::MarkedView`]），MCP 场景是结果的文本内容块。
       */
      text: string;
      type: "marked_image";
    };
/**
 * 文件引用、图片、系统提醒。展开时机由上下文管理层决定。
 */
export type Attachment = {
  type: "attachment";
} & (
  | {
      content: string;
      kind: "memory";
      path: string;
    }
  | {
      content: string;
      kind: "restored_file";
      path: string;
    }
  | {
      content: string;
      kind: "user_file";
      path: string;
    }
  | {
      kind: "environment";
      text: string;
    }
  | {
      kind: "system_reminder";
      text: string;
    }
  | {
      data: string;
      kind: "image";
      media_type: string;
    }
  | {
      data: string;
      kind: "described_image";
      /**
       * `data` 的类型（客户端压缩产物通常是 image/jpeg）。
       */
      media_type: string;
      /**
       * 给模型的转述，自带"当作亲眼所见"的使用指示。
       */
      text: string;
    }
);
export type AssistantContent =
  | {
      text: string;
      type: "text";
    }
  | {
      /**
       * 签名与模型绑定。换模型前必须剥离，否则 API 400。
       * 由 INV-9 断言保证。
       */
      signature?: string | null;
      text: string;
      type: "thinking";
    }
  | {
      id: string;
      input: unknown;
      name: string;
      type: "tool_use";
    };
export type SystemLevel = "info" | "warning" | "error";
export type ProgressPayload =
  | {
      kind: "line";
      stream: OutputStream;
      text: string;
    }
  | {
      done: number;
      kind: "fraction";
      label: string;
      total: number;
    }
  | {
      kind: "status";
      text: string;
    }
  | {
      event: AgentEvent;
      kind: "nested";
    };
export type OutputStream = "stdout" | "stderr";
/**
 * 决策理由。UI 的解释、日志、遥测共用同一份数据。
 *
 * 没有理由的决策无法调试 —— 用户报"为什么它问我这个"时，
 * 你需要能立刻回答。
 */
export type DecisionReason =
  | {
      kind: "rule";
      pattern: string;
      source: RuleSource;
    }
  | {
      kind: "mode";
      mode: PermissionMode;
    }
  | {
      kind: "hook";
      name: string;
    }
  | {
      confidence: number;
      kind: "classifier";
    }
  | {
      kind: "safety_check";
      safety: SafetyKind;
    }
  | {
      kind: "sandbox";
    }
  | {
      kind: "preapproved";
      what: string;
    }
  | {
      kind: "consent";
      what: string;
    }
  | {
      kind: "unverifiable";
      what: string;
    }
  | {
      kind: "user_choice";
      remembered: boolean;
    }
  | {
      kind: "timeout";
    };
export type RuleSource = "policy" | "cli_arg" | "session" | "local" | "project" | "user";
export type PermissionMode =
  "default" | "acceptEdits" | "plan" | "auto" | "bypassPermissions" | "unattended" | "dontAsk";
export type SafetyKind =
  | "git_internals"
  | "ssh_config"
  | "shell_rc"
  | "agent_config"
  | "credentials"
  | "command_injection"
  | "unparseable_command"
  | "out_of_scope";
/**
 * 结构化的"永久同意"。
 */
export type PermissionUpdate =
  | {
      decision: RuleDecision;
      pattern?: string | null;
      scope: UpdateScope;
      tool: string;
      type: "add_rule";
    }
  | {
      mode: PermissionMode;
      scope: UpdateScope;
      type: "set_mode";
    }
  | {
      path: string;
      scope: UpdateScope;
      type: "add_working_directory";
    };
export type RuleDecision = "allow" | "ask" | "deny";
export type UpdateScope = "session" | "local" | "project" | "user";
export type CompactStrategy = "spill" | "aggregate_budget" | "micro_compact" | "full_summary";
/**
 * 终止原因。
 *
 * 在 TS 版本里这是 AsyncGenerator 的 return 值，控制流与数据流分离。
 * Rust 的 `async_stream::stream!` 要求块返回 `()`，所以降级成事件变体。
 * 好处是终止原因现在可序列化、可持久化、可被回放测试断言。
 * 详见 ARCHITECTURE.md §4.2
 */
export type TerminalReason =
  | {
      reason: "completed";
    }
  | {
      limit: number;
      reason: "max_turns";
    }
  | {
      by: AbortSource;
      reason: "aborted";
    }
  | {
      cancelled: number;
      reason: "aborted_tools";
    }
  | {
      message: string;
      reason: "stop_hook_prevented";
    }
  | {
      error: AgentError;
      reason: "error";
    };
export type AbortSource = "user" | "user_interjection" | "sibling_failure" | "permission_denied" | "shutdown";
export type AgentError =
  | {
      kind: "context_exhausted";
      limit: number;
      used: number;
    }
  | {
      attempts: number;
      kind: "compact_circuit_open";
    }
  | {
      kind: "provider";
      message: string;
      retryable: boolean;
    }
  | {
      kind: "internal";
      message: string;
    };
export type Message1 =
  | {
      content: UserContent[];
      id: string;
      meta?: MessageMeta;
      role: "user";
    }
  | {
      content: AssistantContent[];
      id: string;
      meta?: MessageMeta;
      role: "assistant";
      usage?: Usage | null;
    }
  | {
      id: string;
      level: SystemLevel;
      role: "system";
      text: string;
    };
/**
 * 宿主对权限请求的应答。
 */
export type PermissionResponse =
  | {
      /**
       * 用户选中的选项 id（只有 [`AskPreview::Choice`] 会用到）。
       *
       * 空 = 这不是一次提问，或者用户没选任何一项。
       */
      choice?: string[];
      decision: "allow";
      remember?: PermissionUpdate[];
    }
  | {
      decision: "deny";
      /**
       * 用户可以说明理由，会作为 tool_result 喂回模型。
       */
      message?: string | null;
    };
/**
 * Provider 流里的一个事件。
 *
 * 可序列化是为了黄金回放：用例把模型响应存成 JSON，测试时原样喂回主循环。
 */
export type ProviderEvent = StreamDelta1 | Message2 | Usage1 | ProviderError;
/**
 * 流式增量的种类。
 *
 * `[约束]` tag 必须是 `kind`，不能是 `type`。
 *
 * `AgentEvent::Delta` 是 newtype variant，serde 的 internally-tagged 表示会把
 * 这里的字段**摊平**到 AgentEvent 那一层。两边都叫 `type` 的话，序列化产物是
 * `{"type":"delta","type":"text",...}` —— 重复 key，反序列化直接报
 * `duplicate field`，前端一个 token 都收不到。
 *
 * 由 `every_event_variant_roundtrips` 断言。
 */
export type StreamDelta1 = {
  event: "delta";
} & (
  | {
      kind: "text";
      message_id: string;
      text: string;
    }
  | {
      kind: "thinking";
      message_id: string;
      text: string;
    }
  | {
      kind: "tool_start";
      name: string;
      tool_use_id: string;
    }
  | {
      kind: "tool_input";
      partial_json: string;
      tool_use_id: string;
    }
);
/**
 * 一条完整的助手消息。
 */
export type Message2 = {
  event: "message";
} & (
  | {
      content: UserContent[];
      id: string;
      meta?: MessageMeta;
      role: "user";
    }
  | {
      content: AssistantContent[];
      id: string;
      meta?: MessageMeta;
      role: "assistant";
      usage?: Usage | null;
    }
  | {
      id: string;
      level: SystemLevel;
      role: "system";
      text: string;
    }
);
/**
 * 出错。**流在此结束**，不会再有后续事件。
 */
export type ProviderError = {
  event: "error";
} & (
  | {
      kind: "context_overflow";
      limit: number;
      used: number;
    }
  | {
      kind: "output_limit";
    }
  | {
      bytes: number;
      kind: "media_too_large";
    }
  | {
      kind: "retries_exhausted";
      message: string;
    }
  | {
      kind: "auth";
      message: string;
    }
  | {
      kind: "transport";
      message: string;
    }
  | {
      kind: "refused";
      message: string;
    }
);
/**
 * 内核 → 宿主，单向推送。
 */
export type RpcNotification =
  | {
      data: {
        event: AgentEvent;
        session_id: string;
      };
      event: "event.agent";
    }
  | {
      data: {
        fatal: boolean;
        message: string;
      };
      event: "event.kernel_error";
    };
/**
 * 宿主 → 内核。有返回值。
 */
export type RpcRequest =
  | {
      method: "session.create";
      params: {
        cwd: string;
        model: string;
      };
    }
  | {
      method: "session.resume";
      params: {
        cwd: string;
        session_id: string;
      };
    }
  | {
      method: "session.list";
    }
  | {
      method: "session.delete";
      params: {
        session_id: string;
      };
    }
  | {
      method: "turn.submit";
      params: {
        config: TurnConfig;
        input: TurnInput;
        session_id: string;
      };
    }
  | {
      method: "turn.regenerate";
      params: {
        config: TurnConfig1;
        /**
         * 要点重新生成的助手消息 id（不是界面条目 id）。
         */
        message_id: string;
        session_id: string;
      };
    }
  | {
      method: "turn.interrupt";
      params: {
        /**
         * 用户插话时为 true —— UI 不显示"已中断"文案。
         */
        interjection: boolean;
        session_id: string;
      };
    }
  | {
      method: "queue.list";
      params: {
        session_id: string;
      };
    }
  | {
      method: "queue.remove";
      params: {
        entry_id: string;
        session_id: string;
      };
    }
  | {
      method: "queue.take";
      params: {
        entry_id: string;
        session_id: string;
      };
    }
  | {
      method: "session.compact";
      params: {
        model: ModelEndpoint;
        session_id: string;
      };
    }
  | {
      method: "session.changes";
      params: {
        session_id: string;
      };
    }
  | {
      method: "session.git_changes";
      params: {
        /**
         * 对比基线。空 = 当前分支 / HEAD。只换对比对象,不 checkout。
         */
        base?: string | null;
        session_id: string;
      };
    }
  | {
      method: "session.set_title";
      params: {
        session_id: string;
        title?: string | null;
      };
    }
  | {
      method: "scope.list";
      params: {
        session_id: string;
      };
    }
  | {
      method: "scope.revoke";
      params: {
        host: string;
        session_id: string;
      };
    }
  | {
      method: "mcp.reconcile";
      params: {
        servers: McpServerSpec[];
      };
    }
  | {
      method: "mcp.status";
    }
  | {
      method: "mcp.restart";
      params: {
        id: string;
      };
    }
  | {
      method: "permission.respond";
      params: {
        request_id: string;
        response: PermissionResponse;
      };
    }
  | {
      method: "config.set_mode";
      params: {
        mode: PermissionMode;
        session_id: string;
      };
    }
  | {
      method: "tools.list";
      params: {
        session_id: string;
      };
    }
  | {
      method: "kernel.ping";
    }
  | {
      method: "kernel.shutdown";
    };
/**
 * 说话用的协议。决定请求格式与认证头。
 *
 * 和宿主 `config` 里的 `Protocol` 同构 —— 那个是配置侧(会序列化进
 * `config.json`),这个是传输侧(宿主↔内核 RPC)。分开是因为配置类型
 * 属于宿主、不该进 protocol 这个叶子 crate。
 */
export type ApiProtocol = "openai" | "anthropic";
/**
 * 思考力度档。取值刻意与 OpenAI 的 `reasoning_effort` 对齐 ——
 * low/medium/high 是各家（OpenAI / DeepSeek / GLM）都接受的交集，
 * DeepSeek 和 GLM 会把 medium 兼容映射到 high。
 */
export type ThinkingEffort = "low" | "medium" | "high";
/**
 * 内核 → 宿主，对 [`RpcRequest`] 的应答。
 */
export type RpcResponse =
  | {
      data: {
        session_id: string;
      };
      result: "session_created";
    }
  | {
      data: {
        /**
         * 压缩边界之前的消息。模型看不见,界面画在分割线上面。
         */
        archived: Message1[];
        /**
         * 有没有轮子在跑。决定界面显示停止键还是发送键。
         */
        busy: boolean;
        compacting: boolean;
        /**
         * 正在流式生成的正文。流式增量不进历史 —— 不带这段的话，
         * 切回来的界面只能从 0 重新攒，正文缺头直到消息完成。
         */
        live_text?: string;
        /**
         * 正在流式生成的思考。症状同 `live_text`：思考块的字数清零重数。
         */
        live_thinking?: string;
        messages: Message1[];
        /**
         * 还在等用户回答的权限询问。事件只发一次，弹窗跨"切走再切回"
         * 活下来靠这份快照。`default` 兼容旧 transcript 回放。
         */
        pending_asks?: PendingAsk[];
      };
      result: "session_resumed";
    }
  | {
      data: {
        sessions: SessionSummary[];
      };
      result: "session_list";
    }
  | {
      data: {
        turn_id: string;
      };
      result: "turn_started";
    }
  | {
      data: {
        queued_id?: string | null;
      };
      result: "turn_submitted";
    }
  | {
      data: {
        entries: QueuedSummary[];
      };
      result: "queue_list";
    }
  | {
      data: {
        input?: TurnInput1 | null;
      };
      result: "queue_taken";
    }
  | {
      data: {
        removed: boolean;
      };
      result: "removed";
    }
  | {
      data: {
        changes: FileChange[];
      };
      result: "changes";
    }
  | {
      data: {
        git: GitChanges;
      };
      result: "git_changes";
    }
  | {
      data: {
        hosts: string[];
      };
      result: "scope_hosts";
    }
  | {
      data: {
        servers: McpServerStatus[];
      };
      result: "mcp_statuses";
    }
  | {
      data: {
        tools: ToolInfo[];
      };
      result: "tools_list";
    }
  | {
      data: {
        version: string;
      };
      result: "pong";
    }
  | {
      result: "ok";
    }
  | {
      data: {
        error: RpcError;
      };
      result: "error";
    };
export type LineKind = "context" | "add" | "del";
export type ChangeStatus = ("created" | "modified" | "deleted") | "renamed";
export type RpcErrorCode = ("session_not_found" | "invalid_params" | "internal") | "turn_in_progress";

/**
 * 把所有顶层类型收进一个 root，让生成的 schema 共享同一份 `$defs`。
 * 这样下游的 TS 生成器能产出一个类型互相引用的完整文件。
 */
export interface ProtocolRoot {
  agent_event: AgentEvent;
  message: Message1;
  permission_ask: PermissionAsk;
  permission_response: PermissionResponse;
  provider_event: ProviderEvent;
  rpc_notification: RpcNotification;
  rpc_request: RpcRequest;
  rpc_response: RpcResponse;
}
export interface MessageMeta {
  /**
   * 该消息由哪个 agent 产生。None = 主 agent。
   */
  agent_id?: string | null;
  /**
   * 这条回答是被用户按停止**截断**的，不是模型自己说完的。
   *
   * 只给界面标注用。模型那边不需要额外说明 —— 它看到的就是一句
   * 半截话后面紧跟着用户的下一条消息，而 meta 从来不进 wire 格式。
   */
  interrupted?: boolean;
  /**
   * API 错误产生的消息。**这类消息上绝不能跑 stop hooks**，
   * 否则会形成 error → hook 注入 → 重试 → error 的死循环。
   * 由 INV-6 断言保证。
   */
  is_api_error?: boolean;
  /**
   * 产生这条消息的模型。thinking signature 与模型绑定，
   * 降级换模型时要靠这个字段找出需要剥离签名的消息。
   * 由 INV-9 断言保证。
   */
  model_origin?: string | null;
  /**
   * 是否为系统合成（而非模型产出或用户输入）。
   */
  synthetic?: boolean;
}
/**
 * Token 用量。
 *
 * 注意：流式 API 报的是**累计值不是增量**。`message_delta` 里的
 * input/cache 字段可能回 0，直接覆盖会抹掉 `message_start` 的真值。
 * 累加时用 [`Usage::merge`]，它对这些字段做了 `> 0` 守卫。
 */
export interface Usage {
  cache_creation_tokens: number;
  cache_read_tokens: number;
  input_tokens: number;
  output_tokens: number;
}
/**
 * 发给 UI 的权限请求详情。
 */
export interface PermissionAsk {
  /**
   * 结构化预览：diff、命令、URL。UI 据此渲染。
   */
  preview:
    | {
        command: string;
        cwd: string;
        kind: "command";
      }
    | {
        diff: string;
        kind: "file_edit";
        path: string;
      }
    | {
        bytes: number;
        kind: "file_write";
        lines: number;
        path: string;
        preview: string;
        truncated: boolean;
      }
    | {
        kind: "network_fetch";
        url: string;
      }
    | {
        kind: "plain";
        text: string;
      }
    | {
        /**
         * 允许选多项。
         */
        allow_multiple: boolean;
        kind: "choice";
        options: AskChoiceOption[];
        question: string;
      };
  reason: DecisionReason;
  suggestions: PermissionUpdate[];
  /**
   * 给用户看的一句话描述，如 "运行 npm test"。
   */
  summary: string;
  tool_name: string;
  tool_use_id: string;
}
/**
 * 结构化提问的一个候选项。
 */
export interface AskChoiceOption {
  /**
   * 回传给模型的稳定标识。用它而不是 label —— label 是给人读的文案，
   * 改一个字就会让模型收到不一样的答案。
   */
  id: string;
  /**
   * 给用户看的文案。
   */
  label: string;
}
/**
 * Token 用量。
 *
 * 注意：流式 API 报的是**累计值不是增量**。`message_delta` 里的
 * input/cache 字段可能回 0，直接覆盖会抹掉 `message_start` 的真值。
 * 累加时用 [`Usage::merge`]，它对这些字段做了 `> 0` 守卫。
 */
export interface Usage1 {
  cache_creation_tokens: number;
  cache_read_tokens: number;
  input_tokens: number;
  output_tokens: number;
}
/**
 * 本轮的完整配置:模型端点、联网/视觉、limits、mode、会话设置。
 * Box 是因为它比其它变体大得多,不装箱会把整个 enum 撑大。
 */
export interface TurnConfig {
  /**
   * 只读侦察子 agent 的便宜档;也用于 Auto 模式的判危分类器。
   * None = 跟主模型。
   */
  cheap_model?: ModelEndpoint | null;
  limits: TurnLimits;
  /**
   * 会话权限模式。
   */
  mode: "default" | "acceptEdits" | "plan" | "auto" | "bypassPermissions" | "unattended" | "dontAsk";
  model: ModelEndpoint1;
  /**
   * 会话级 Python 虚拟环境根目录。
   */
  python_venv?: string | null;
  /**
   * 会话内累积的权限规则("总是允许"等)。
   */
  rules?: PermissionRule[];
  /**
   * 会话级追加系统提示词。
   */
  system_prompt_extra?: string | null;
  /**
   * 会话级思考策略。
   */
  thinking?:
    | {
        mode: "default";
      }
    | {
        mode: "adaptive";
      }
    | {
        mode: "disabled";
      }
    | {
        level: ThinkingEffort;
        mode: "fixed";
      };
  vision: VisionSetup;
  web: WebSetup;
}
/**
 * 一个已解析的模型端点:宿主把 provider 配置和明文 key 都填好,内核直接
 * 拿它建 Provider。
 *
 * 这是 `config::ResolvedModel` 的"传输版" —— 区别在于 `api_key` 是**明文**
 * (宿主已从环境变量 / auth.json 解析出来),而不是一个待查的变量名。
 * 拆进程后内核拿不到 auth.json,key 必须在宿主这一侧解析完再传进来。
 */
export interface ModelEndpoint {
  /**
   * 明文密钥。见模块文档的约束。
   */
  api_key: string;
  /**
   * 接口路径,空 = 按主机猜(见 `riot_providers::endpoint`)。
   */
  api_path: string;
  base_url: string;
  fallback_model?: string | null;
  model: string;
  protocol: ApiProtocol;
  sampling?: EndpointSampling;
}
/**
 * 采样参数。`None` = 用端点默认。
 *
 * 独立于 `riot-providers` 的 `SamplingParams`(那个不含 `max_output_tokens`,
 * 因为输出上限在主循环单独走恢复路径)—— 这里是"宿主配置的完整快照",
 * 由内核在建 Provider 和设置输出上限时各取所需。
 */
export interface EndpointSampling {
  max_output_tokens?: number | null;
  temperature?: number | null;
  top_k?: number | null;
  top_p?: number | null;
}
/**
 * 一轮的数值上限与隔离强度。
 */
export interface TurnLimits {
  /**
   * 权限弹窗等多久算超时(秒)。超时按拒绝处理。
   */
  ask_timeout_secs: number;
  /**
   * 历史超过这个 token 数就在开工前做 LLM 总结压缩。
   */
  compact_threshold_tokens: number;
  /**
   * 单轮最多自主往返多少步。
   */
  max_turns: number;
  /**
   * 命令的 OS 级隔离强度。和宿主 `config::SandboxMode` 同构。
   */
  sandbox?: "workspace_write" | "workspace_write_no_net" | "off";
}
/**
 * 一个已解析的模型端点:宿主把 provider 配置和明文 key 都填好,内核直接
 * 拿它建 Provider。
 *
 * 这是 `config::ResolvedModel` 的"传输版" —— 区别在于 `api_key` 是**明文**
 * (宿主已从环境变量 / auth.json 解析出来),而不是一个待查的变量名。
 * 拆进程后内核拿不到 auth.json,key 必须在宿主这一侧解析完再传进来。
 */
export interface ModelEndpoint1 {
  /**
   * 明文密钥。见模块文档的约束。
   */
  api_key: string;
  /**
   * 接口路径,空 = 按主机猜(见 `riot_providers::endpoint`)。
   */
  api_path: string;
  base_url: string;
  fallback_model?: string | null;
  model: string;
  protocol: ApiProtocol;
  sampling?: EndpointSampling;
}
export interface PermissionRule {
  decision: RuleDecision;
  /**
   * None = 整工具规则；Some = 内容级规则，如 `npm run *`。
   */
  pattern?: string | null;
  source: RuleSource;
  tool: string;
}
/**
 * 视觉能力配置。
 */
export interface VisionSetup {
  /**
   * 主模型能否直接收图片。
   */
  accepts_images: boolean;
  /**
   * 视觉兼容模型端点(主模型收不了图时转述)。None = 无,截图工具报未配置。
   */
  describe?: ModelEndpoint | null;
}
/**
 * 联网能力配置(随 turn 传给内核)。
 *
 * 抓取(fetch)不需要第三方服务;搜索(search)默认走内置 SearXNG,用户可覆盖;
 * 蒸馏(distill)要一个辅助模型端点。三者独立开关,和宿主 `WebConfig` 一致。
 */
export interface WebSetup {
  /**
   * 网页正文蒸馏的辅助模型端点。None = 不蒸馏,抓取返回截断原文。
   */
  distill?: ModelEndpoint | null;
  fetch_enabled: boolean;
  search_enabled: boolean;
  /**
   * 用户覆盖的 SearXNG 地址。空 = 用内置实例。
   */
  searxng_url?: string;
}
/**
 * 用户原始输入(text/images/refs)。图片转述、`@` 展开、hook 都在内核做。
 */
export interface TurnInput {
  images?: ImageInput[];
  refs?: string[];
  text: string;
}
/**
 * 用户随消息附上的一张图。只走内容不走路径(剪贴板截图没有路径)。
 */
export interface ImageInput {
  data: string;
  mediaType: string;
}
/**
 * 提交一轮所需的完整配置(`turn.submit` 的 RPC 载荷,除用户输入之外的一切)。
 *
 * 宿主从 `AppConfig` + 会话设置解析出它,内核据此现装 provider、联网、视觉、
 * 子 agent、权限。**不含** MCP / Skill 工具:那些是 trait object,不能跨进程,
 * 由内核自己从 MCP hub 和技能目录装配(见 M-B4b)。
 */
export interface TurnConfig1 {
  /**
   * 只读侦察子 agent 的便宜档;也用于 Auto 模式的判危分类器。
   * None = 跟主模型。
   */
  cheap_model?: ModelEndpoint | null;
  limits: TurnLimits;
  /**
   * 会话权限模式。
   */
  mode: "default" | "acceptEdits" | "plan" | "auto" | "bypassPermissions" | "unattended" | "dontAsk";
  model: ModelEndpoint1;
  /**
   * 会话级 Python 虚拟环境根目录。
   */
  python_venv?: string | null;
  /**
   * 会话内累积的权限规则("总是允许"等)。
   */
  rules?: PermissionRule[];
  /**
   * 会话级追加系统提示词。
   */
  system_prompt_extra?: string | null;
  /**
   * 会话级思考策略。
   */
  thinking?:
    | {
        mode: "default";
      }
    | {
        mode: "adaptive";
      }
    | {
        mode: "disabled";
      }
    | {
        level: ThinkingEffort;
        mode: "fixed";
      };
  vision: VisionSetup;
  web: WebSetup;
}
/**
 * MCP 服务器的启动描述。宿主从设置里组好(过滤掉未启用/没填完的),
 * 内核只管照单连接 —— 内核不读配置文件。
 */
export interface McpServerSpec {
  args: string[];
  command: string;
  env: [unknown, unknown][];
  /**
   * 稳定标识,进工具名(`mcp__<id>__…`),也是权限规则的一部分。
   */
  id: string;
}
/**
 * 一条还在等用户回答的权限询问（会话快照用）。
 *
 * `permission_request` 事件只在询问产生那一刻发一次；界面切走的话它发进
 * 没人听的旧通道。快照不带的话，切回来弹窗再也不出现 —— 那次询问只能
 * 等到超时被拒，模型收到"授权请求没有得到回应"。
 */
export interface PendingAsk {
  detail: PermissionAsk;
  request_id: string;
}
export interface SessionSummary {
  cwd: string;
  message_count: number;
  session_id: string;
  title?: string | null;
  updated_at_ms: number;
}
/**
 * 排队面板的一条插话摘要。
 */
export interface QueuedSummary {
  id: string;
  /**
   * 附了几张图。面板只显示个数 —— 全量 base64 回传太重。
   */
  images: number;
  /**
   * 引用的文件路径。面板直接列出来(它们是路径,不重)。
   */
  refs: string[];
  text: string;
}
/**
 * 用户这一轮发来的原始输入。图片转述、`@` 展开、UserPromptSubmit hook 都在
 * 内核完成 —— 所以这里只传原始三样,内核据此构造最终消息(内核有 vision /
 * mentions / hooks,宿主没有,不能在宿主构造一半)。
 */
export interface TurnInput1 {
  images?: ImageInput[];
  refs?: string[];
  text: string;
}
export interface FileChange {
  added: number;
  /**
   * 二进制文件:没有可读的逐行差异,`hunks` 为空、行数为 0。
   */
  binary?: boolean;
  hunks: Hunk[];
  /**
   * 相对项目根的路径。绝对路径在界面上又长又没有信息量。
   */
  path: string;
  removed: number;
  /**
   * 重命名前的旧路径。仅 status = renamed 时有。
   */
  renamedFrom?: string | null;
  status: ChangeStatus;
  /**
   * 差异太大,`hunks` 只是前一截。
   */
  truncated: boolean;
}
export interface Hunk {
  /**
   * `@@ -1,4 +1,6 @@` 那一行。
   */
  header: string;
  lines: DiffLine[];
}
export interface DiffLine {
  kind: LineKind;
  text: string;
}
/**
 * `session.git_changes` 的应答:工作区相对所选基线的差异。
 *
 * 和会话改动(`session.changes`)回答的问题不同:那边是"这个会话经
 * 工具改了什么",commit 之后依然在;这边跟着 git 走。基线默认是
 * 当前分支(等于 HEAD);用户换分支只换对比对象,不 checkout。
 */
export interface GitChanges {
  /**
   * 实际用来 `git diff` 的基线(分支名或 HEAD)。
   */
  base?: string | null;
  /**
   * 当前检出的分支。detached HEAD 时为空。
   */
  branch?: string | null;
  changes: FileChange[];
  /**
   * 下拉里的候选:本地分支 + 远程跟踪分支。
   */
  refs?: string[];
  /**
   * false = 项目目录不是 git 仓库。面板显示引导文案,而不是把
   * "没有仓库"和"工作区干净"混成同一个空列表。
   */
  repo: boolean;
}
/**
 * MCP 连接状态快照,给设置页看。
 */
export interface McpServerStatus {
  /**
   * connected 时是服务器自报的名字和版本;failed 时是错误原因。
   */
  detail: string;
  id: string;
  /**
   * `connecting` / `connected` / `failed`
   */
  state: string;
  /**
   * 对外的完整工具名(`mcp__…`)。
   */
  tools: string[];
}
export interface ToolInfo {
  enabled: boolean;
  name: string;
  user_facing_name: string;
}
export interface RpcError {
  code: RpcErrorCode;
  message: string;
}
