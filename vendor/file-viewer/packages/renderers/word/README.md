# @file-viewer/renderer-word

Flyfish File Viewer 的独立 Word / 兼容文档 renderer 包。它承接 DOCX/DOCM/DOTX/DOTM、旧版 DOC/DOT、RTF、ODT/ODP 等文档预览链路，让 `@file-viewer/core` 不再直接安装 Word 解析与渲染依赖。

## 用法

```ts
import FileViewer from '@file-viewer/vue3'
import { wordRenderer } from '@file-viewer/renderer-word'

const options = {
  rendererMode: 'replace',
  renderers: wordRenderer,
}
```

也可以和其他 renderer 一起组合：

```ts
import { wordRenderer } from '@file-viewer/renderer-word'
import { pdfRenderer } from '@file-viewer/renderer-pdf'
import { presentationRenderer } from '@file-viewer/renderer-presentation'

const options = {
  rendererMode: 'replace',
  renderers: [wordRenderer, pdfRenderer, presentationRenderer],
}
```

官方 Demo 和完整格式矩阵可以直接使用 `@file-viewer/preset-all`。

## 能力边界

- DOCX / DOCM / DOTX / DOTM 使用自研 `@file-viewer/docx`，默认 Worker 解析、连续流式阅读、目录字段缓存、异步分批渲染，并跟随 viewer 主题启用暗黑文档面。
- DOC / DOT 使用 `@file-viewer/doc`，并套用 Word 风格纸张阅读面、缩放、打印和 HTML 导出适配。
- RTF 使用 `rtf.js`，ODT / ODP 读取 OpenDocument 包内 `content.xml` 做安全结构预览。
- 继续复用 core 的统一搜索、缩放、打印、导出、生命周期和操作能力。

## 离线资产

DOCX Worker 默认读取 viewer assets 下的：

- `vendor/docx/docx.worker.js`
- `vendor/docx/jszip.min.js`

私有化部署时可以通过 `options.docx.workerUrl` 和 `options.docx.workerJsZipUrl` 覆盖：

```ts
const options = {
  docx: {
    workerUrl: '/file-viewer/vendor/docx/docx.worker.js',
    workerJsZipUrl: '/file-viewer/vendor/docx/jszip.min.js',
  },
}
```

DOCX 暗黑渲染默认由 `options.theme` 决定：`dark` 开启、`light` 关闭、`system` 跟随浏览器系统主题。需要业务固定效果时可传 `options.docx.darkMode: true / false`。

## 迁移说明

`@file-viewer/core` 已不再直接依赖 `@file-viewer/docx`、`@file-viewer/doc`、`rtf.js`、`linkedom` 或 `@xmldom/xmldom`。需要 Word 完整预览时，请安装本包并通过 `renderers` 传入，或使用 `@file-viewer/preset-all`。
