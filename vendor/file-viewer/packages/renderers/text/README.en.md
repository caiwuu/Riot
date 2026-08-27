# @file-viewer/renderer-text

Standalone code, text, Markdown, patch, and git bundle renderer package for Flyfish File Viewer. It handles source highlighting, Markdown reading surfaces, embedded Mermaid diagrams in Markdown, side-by-side patch diffs, git bundle inspection, and unified zoom for `.txt`, `.json`, `.ts`, `.vue`, `.log`, `.md`, `.markdown`, `.patch`, `.bundle`, and other code delivery formats.

## Usage

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

You can also compose it with other renderers:

```ts
import { textRenderer } from '@file-viewer/renderer-text'
import { pdfRenderer } from '@file-viewer/renderer-pdf'

const options = {
  builtinRenderers: 'none',
  renderers: [pdfRenderer, textRenderer],
}
```

## Capabilities

- Code and text preview uses `highlight.js` core with per-language dynamic imports instead of registering every language up front.
- Code, text, and virtualized Markdown source views show their file type, indexing status, and line-count metadata bar by default. Set `options.text.toolbar: false` to hide this renderer-local bar without hiding the viewer-level download, search, or zoom toolbar.
- Regular code and text previews can show a line-number gutter with `options.text.lineNumbers: true`. The gutter is excluded from copied source, search matches, and assistive reading. Virtual large-text views keep their existing gutter unless it is explicitly set to `false`.
- `patch` uses `diff2html` on demand for side-by-side review.
- `bundle` / `bdl` parses Git bundle headers, refs, commit history, file trees, and readable blobs; regular OFS_DELTA / REF_DELTA objects are resolved in the browser, while very large packs or bundles that depend on external prerequisites surface a clear boundary notice.
- HTML, XML, Vue, and similar files are escaped and shown as source, never executed.
- Markdown uses `marked` for a read-only reading surface with dark/light theme support, table scrolling, and a unified zoom provider.
- Markdown no longer falls back to source because of the general large-text threshold. Set `options.text.markdownVirtualizeAboveBytes` only when an application must bound exceptionally large Markdown files.
- Does not depend on any online service or public CDN, making it suitable for intranet logs, configs, snippets, README files, and knowledge-base attachments.

## Migration Note

`@file-viewer/core` no longer bundles code / markdown renderers and no longer depends directly on `highlight.js`, `marked`, `diff2html`, or git bundle parsing dependencies. Install this package explicitly for code, text, Markdown, patch, or git bundle preview, or use `@file-viewer/preset-all`, which aggregates this renderer automatically.
