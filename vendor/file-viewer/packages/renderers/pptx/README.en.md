# @file-viewer/pptx

Native PPTX rendering engine extracted from Flyfish File Viewer. It is maintained as a standalone package so presentation rendering can evolve independently from framework adapters.

## Highlights

- Pure TypeScript/DOM API with no Vue, React, or iframe dependency.
- Web Worker based PPTX parsing with progressive slide output.
- Preserves the latest native Flyfish rendering path for themes, images, shapes, tables, text, and basic charts.
- Supports `fitMode`, `zoomPercent`, custom Worker URLs, and Worker factories for private deployment and strict CSP environments.

## Usage

```ts
import { PptxViewer } from '@file-viewer/pptx'

const buffer = await file.arrayBuffer()
const viewer = await PptxViewer.open(buffer, document.querySelector('#app')!, {
  fitMode: 'contain',
  zoomPercent: 100,
  onSlideRendered(index) {
    console.log('slide rendered', index)
  },
})

await viewer.setZoom(125)
viewer.destroy()
```

Most users should consume PPTX through `@file-viewer/core` or a standard framework package. The core package lazy-loads this renderer only when a presentation is opened.
