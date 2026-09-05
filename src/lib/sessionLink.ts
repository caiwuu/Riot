/**
 * "跳到某个历史会话"的模块级入口（和 subagentLink 同一个形状）。
 *
 * 来源只有一处：回答里的 `riot://session/<id>` 链接 —— 模型翻过历史会话
 * 摘录之后按系统提示词里的约定引用它。接收方是 App：把侧栏切到那个会话。
 *
 * 会话可能已经被删了（摘录目录里的引用比会话活得久是正常的：模型引用的
 * 时候它还在，用户后来删了）。所以入口要回答"切过去了没有"，链接那边
 * 据此提示"该会话已删除"，而不是点了没反应。
 */

let listener: ((sessionId: string) => boolean) | null = null;

/** 请求切到会话。返回 false = 没有这个会话（已删除或不属于当前列表）。 */
export function openSession(sessionId: string): boolean {
  return listener?.(sessionId) ?? false;
}

/** App 订阅。返回退订函数。 */
export function subscribeSessionOpen(cb: (sessionId: string) => boolean): () => void {
  listener = cb;
  return () => {
    if (listener === cb) listener = null;
  };
}

/** `riot://session/ses_xxx` 链接的前缀。系统提示词 `past_sessions` 一节教模型这么写。 */
export const SESSION_LINK_PREFIX = "riot://session/";

/** 从链接地址里取会话 id；不是会话链接返回 null。 */
export function sessionIdFromHref(href: string): string | null {
  const raw = href.trim();
  if (!raw.toLowerCase().startsWith(SESSION_LINK_PREFIX)) return null;
  const id = raw.slice(SESSION_LINK_PREFIX.length).replace(/[/?#].*$/, "").trim();
  return id || null;
}
