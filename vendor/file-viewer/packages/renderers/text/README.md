# @file-viewer/renderer-text

Flyfish File Viewer 的独立代码、文本、Markdown、patch 和 git bundle renderer 包。它负责 `.txt`、`.json`、`.ts`、`.vue`、`.log`、`.md`、`.markdown`、`.patch`、`.bundle` 等文本与代码交付文件的源码高亮、Markdown 阅读面、Markdown 内嵌 Mermaid 图、左右比对、bundle 结构查看和统一缩放。

## 用法

```ts
import FileViewer from '@file-viewer/vue3'
import { textRenderer } from '@file-viewer/renderer-text'

const options = {
  builtinRenderers: 'none',
  renderers: textRenderer,
  text: {
    lineNumbers: true,
  },
}
```

也可以与其他 renderer 组合：

```ts
import { textRenderer } from '@file-viewer/renderer-text'
import { pdfRenderer } from '@file-viewer/renderer-pdf'

const options = {
  builtinRenderers: 'none',
  renderers: [pdfRenderer, textRenderer],
}
```

## 能力边界

- 代码和文本使用 `highlight.js` core + 按语言动态加载，避免一次性注册全部语言。
- 代码、文本和超大 Markdown 源码视图默认显示文件类型、索引状态和行数元信息栏；传入 `options.text.toolbar: false` 可隐藏该 renderer 内部栏，不影响 Viewer 的下载、搜索、缩放等全局工具栏。
- 普通代码和文本可通过 `options.text.lineNumbers: true` 显示行号；行号不会进入复制内容、搜索结果或无障碍朗读。超大文本保留原有的虚拟行号栏，可显式传 `false` 隐藏。
- `patch` 使用 `diff2html` 按需渲染左右比对视图。
- `bundle` / `bdl` 会解析 Git bundle header、refs、commit 历史、文件树和可读 blob；常规 OFS_DELTA / REF_DELTA 会在浏览器端还原，超大包或依赖外部 prerequisite 的包会给出清晰边界提示。
- HTML / XML / Vue 等文件按源码方式转义展示，不执行脚本。
- Markdown 使用 `marked` 输出只读阅读面，并保留明暗主题、表格滚动和统一缩放 provider。
- Markdown 不再因为通用大文本阈值自动退化成源码；如业务必须限制超大 Markdown，可单独设置 `options.text.markdownVirtualizeAboveBytes`。
- 不绑定任何在线服务或公共 CDN，适合内网日志、配置、代码片段、README 和知识库附件预览。

## 迁移说明

`@file-viewer/core` 已不再内置 code / markdown renderer，也不再直接依赖 `highlight.js`、`marked`、`diff2html` 或 Git bundle 解析依赖。需要代码、文本、Markdown、patch 或 git bundle 预览时，请显式安装本包，或使用会自动聚合本包的 `@file-viewer/preset-all`。
