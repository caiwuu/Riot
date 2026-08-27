# @file-viewer/renderer-word

Standalone Word and compatible document renderer package for Flyfish File Viewer. It owns the DOCX/DOCM/DOTX/DOTM, legacy DOC/DOT, RTF, ODT, and ODP preview path so `@file-viewer/core` no longer installs Word parsing engines directly.

## Usage

```ts
import FileViewer from '@file-viewer/vue3'
import { wordRenderer } from '@file-viewer/renderer-word'

const options = {
  rendererMode: 'replace',
  renderers: wordRenderer,
}
```

You can compose it with other renderers:

```ts
import { wordRenderer } from '@file-viewer/renderer-word'
import { pdfRenderer } from '@file-viewer/renderer-pdf'
import { presentationRenderer } from '@file-viewer/renderer-presentation'

const options = {
  rendererMode: 'replace',
  renderers: [wordRenderer, pdfRenderer, presentationRenderer],
}
```

Use `@file-viewer/preset-all` when you want the same complete matrix as the official demo.

## Capabilities

- DOCX / DOCM / DOTX / DOTM use the self-maintained `@file-viewer/docx` engine with Worker parsing, continuous reading layout, cached TOC fields, async batched rendering, and a dark document surface that follows the viewer theme.
- DOC / DOT use `@file-viewer/doc` with a Word-like paper surface, zoom, print, and HTML export adapters.
- RTF uses `rtf.js`; ODT / ODP read `content.xml` from OpenDocument packages for safe structure previews.
- The renderer reuses core search, zoom, print, export, lifecycle, and operation APIs.

## Offline Assets

DOCX Worker defaults to viewer assets:

- `vendor/docx/docx.worker.js`
- `vendor/docx/jszip.min.js`

Override them for private deployments:

```ts
const options = {
  docx: {
    workerUrl: '/file-viewer/vendor/docx/docx.worker.js',
    workerJsZipUrl: '/file-viewer/vendor/docx/jszip.min.js',
  },
}
```

DOCX dark rendering follows `options.theme` by default: `dark` enables it, `light` disables it, and `system` follows the browser color scheme. Pass `options.docx.darkMode: true / false` when the host app needs a fixed result.

## Migration

`@file-viewer/core` no longer depends on `@file-viewer/docx`, `@file-viewer/doc`, `rtf.js`, `linkedom`, or `@xmldom/xmldom` directly. Install and pass this renderer for full Word preview, or use `@file-viewer/preset-all`.
