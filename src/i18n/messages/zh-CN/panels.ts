/** 工作台面板：终端、浏览器、文件预览、文件树、Git 改动。 */
export default {
  /* ── 终端面板 ── */
  /** 没有目录时新标签的默认标题。 */
  "panels.terminal.title": "终端",
  "panels.terminal.newTab": "新终端",
  "panels.terminal.closeTab": "关闭终端",
  "panels.terminal.hide": "收起终端面板",
  /** 标签上的小角标：模型起的服务 / 已退出。 */
  "panels.terminal.badge.agent": "模型",
  "panels.terminal.badge.exited": "已退出",
  "panels.terminal.agentTabTitle": "{title}（模型起的服务）",
  "panels.terminal.share.on": "正在共享给 agent：它能读这个终端的输出（点击收回）",
  "panels.terminal.share.off": "共享给 agent：让它能读这个终端的输出，但不能停它",
  "panels.terminal.share.mark": "已共享",
  "panels.terminal.share.failed": "共享失败",
  "panels.terminal.sendSelection": "把选中的内容发给模型",
  "panels.terminal.sendSelection.hint": "先在终端里选中一段文本，再从这里发给模型",
  "panels.terminal.closeConfirm.title": "关闭「{title}」？",
  "panels.terminal.closeConfirm.agentBody": "这是模型起的服务，关闭会立即终止它 —— 模型可能正依赖这个服务。",
  "panels.terminal.closeConfirm.body": "这个终端里有正在运行的进程，关闭会立即终止它。",
  "panels.terminal.closeConfirm.confirm": "关闭并终止",
  /** 下面几条写进 xterm 画面里（不是 DOM）。 */
  "panels.terminal.reattachFailed": "重连后接不上这个终端，它可能已经退出了。",
  "panels.terminal.processExited": "[进程已退出。日志留在这里，点标签上的 × 关闭。]",
  "panels.terminal.attachFailed": "接不上这个终端。它对应的服务可能已经退出了，可以关掉这个标签。",
  "panels.terminal.startFailed": "终端没能启动，可以关掉这个标签再开一个试试。",

  /* ── 浏览器面板 ── */
  "panels.browser.back": "后退",
  "panels.browser.forward": "前进",
  "panels.browser.starting": "浏览器启动中…",
  "panels.browser.address": "地址栏",
  "panels.browser.address.placeholder": "输入 URL",
  "panels.browser.viewMode": "视口模式",
  "panels.browser.viewMode.fit": "自适应：页面按面板宽度渲染",
  "panels.browser.viewMode.web": "Web：按 {width}px 桌面宽度渲染，整体缩放进面板",
  "panels.browser.pick": "取件",
  "panels.browser.pick.title": "取件：点面板里的元素，拿到它的选择器交给模型",
  "panels.browser.pick.miss": "没点中任何元素，再点一次页面里的东西。",
  "panels.browser.pick.failed": "取件失败：{error}",
  "panels.browser.navFailed": "打不开：{error}",
  "panels.browser.keyboard": "页面键盘输入",
  "panels.browser.empty.title": "开始浏览",
  "panels.browser.empty.hint": "输入网址，与模型同看。",

  /* ── 文件预览 ── */
  "panels.filePreview.openFailed": "打不开",
  "panels.filePreview.showTree": "显示文件树",
  "panels.filePreview.hideTree": "隐藏文件树",
  "panels.filePreview.pickOne": "从右侧选择一个文件",
  "panels.filePreview.loadingViewer": "正在加载预览器…",
  "panels.filePreview.reading": "正在读取文件…",
  "panels.filePreview.binary": "这是二进制文件，应用内看不了。",

  /* ── 文件树 ── */
  "panels.fileTree.label": "项目文件",
  "panels.fileTree.loading": "正在读取…",
  "panels.fileTree.truncated": "还有 {count} 项未显示",
  "panels.fileTree.filter": "筛选文件",
  "panels.fileTree.filter.placeholder": "筛选文件…",
  "panels.fileTree.clear": "清除",
  "panels.fileTree.openFromDisk": "从磁盘打开",
  "panels.fileTree.openFromDisk.title": "从磁盘打开…（⌘O）",
  "panels.fileTree.noMatch": "没有匹配的文件",
  "panels.fileTree.symlink": "符号链接",

  /* ── Git 改动 ── */
  "panels.git.base.title": "对比基线。只换看哪条分支，不会 checkout。",
  "panels.git.currentBranch": "当前分支",
  "panels.git.recompare": "重新比对",
  "panels.git.comparing": "正在比对…",
  "panels.git.failed": "比对失败：{error}",
  "panels.git.stale": "下方显示的是上次的结果。",
  "panels.git.notRepo": "这个目录不是 git 仓库。",
  "panels.git.notRepo.hint": "初始化仓库（git init）之后，这里会显示未提交的改动。",
  "panels.git.noDiffAgainst": "相对 {base} 没有差异。",
  "panels.git.clean": "工作区干净，没有未提交的改动。",
  "panels.git.scope.title":
    "工作区（含未提交）相对所选分支的差异。换分支只换对比基线，不会 checkout。只看本次会话动了什么，用输入框上方的改动条。",
  "panels.git.baseLabel": "对比基线：{base}",
  "panels.git.allUncommitted": "显示的是 git 未提交的全部改动",
};
