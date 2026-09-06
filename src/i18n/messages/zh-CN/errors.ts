/** bridge 层自己产生的错误，以及把宿主错误拼成一句话的模板。 */
export default {
  /** 宿主错误带技术细节时的拼法。`text` 是译文，`detail` 是原始原因。 */
  "errors.withDetail": "{text}（{detail}）",
  "errors.ipcTimeout": "宿主没有响应：{command} 超过 {seconds} 秒没有返回。",
  "errors.disconnected": "与宿主的连接已断开，请稍后重试。",
  "errors.unknown": "出了点问题。",

  /* ── 网页版（WebSocket 传输）的连接状态 ── */
  "errors.authRequired": "还没连上宿主：需要访问令牌",
  "errors.reconnectingWithToken": "正在用新令牌重新连接。",
  "errors.signedOut": "已退出",
  "errors.hostDenied": "宿主拒绝连接：{reason}",

  /* ── 网页版里没有"这台机器"可用的操作 ── */
  "errors.webNoServerFiles": "网页版不能选服务器上的文件，请在输入框里用 @ 引用它。",
  "errors.webNoLocalOpen": "网页版无法在这台设备上打开服务器上的文件。",
  "errors.webUseInAppDirPicker": "网页版请使用应用内的目录选择器。",
  "errors.popupBlocked": "浏览器拦住了新窗口，请允许弹出窗口后重试。",

  "errors.fileNotFound": "文件不存在：{path}",
  /** 系统文件选择框里"图片"那一类过滤器的名字。不是错误，但属于 bridge 层的文案。 */
  "errors.dialog.imagesFilter": "图片",
};
