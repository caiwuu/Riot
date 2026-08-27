# vendor/file-viewer

文件预览用的渲染库，来自 [flyfish-dev/file-viewer](https://github.com/flyfish-dev/file-viewer)
（Apache-2.0，许可证原文见同目录 `LICENSE`）。

**这里放的是构建产物（`dist/`），不是源码。** 理由、以及为什么必须是 pnpm
workspace 成员而不是 `link:` 覆盖，见 `scripts/vendor-file-viewer.mjs` 的文件头。

## 版本溯源

| | |
|---|---|
| 上游基线 | `9bfed4c30152a1d6179708e84d642e40757e0d1a` |
| 同步自 | `60dab668bc2cc87fd2c520017615e68d769bc0b7` |

fork 相对上游的本地定制：

```
60dab668 Riot 定制:点击/滚动不关闭 PDF 自动贴宽(仅真实缩放操作关闭)
cdb26d14 Riot 定制:表格禁用默认初始缩放(原始尺寸+横滚);docx 贴宽允许放大
0a4ef88c Riot 定制:docx 贴宽预留量归零(同 PDF,抽屉预览要求页面贴满)
168e5611 Riot 定制:PDF 贴宽预留量归零(抽屉预览要求页面贴满)
```

`[约束]` 这些定制**只存在于 fork 里**，仓库里只有它们编译后的样子。fork 丢了
就找不回来 —— 要么把 fork 推到远端，要么在这里改成 vendor 源码。

## 收录的包

- `@file-viewer/core@2.4.0`
- `@file-viewer/react@2.4.0`
- `@file-viewer/vite-plugin@2.4.0`
- `@file-viewer/doc@2.4.0`
- `@file-viewer/renderer-pdf@2.4.0`
- `@file-viewer/pptx@2.4.0`
- `@file-viewer/renderer-presentation@2.4.0`
- `@file-viewer/renderer-spreadsheet@2.4.0`
- `@file-viewer/renderer-text@2.4.0`
- `@file-viewer/renderer-word@2.4.0`

`@file-viewer/docx` 和 `@file-viewer/ppt` 不在此列：它们是正常发布到 npm 的包
（`^0.3.27` / `0.3.3`），由 lockfile 正常解析。

## 怎么升级

```bash
cd ../file-viewer && git pull --rebase && pnpm install && pnpm build
cd -             && node scripts/vendor-file-viewer.mjs && pnpm install
```

脚本会拒绝在 fork 工作区不干净时运行 —— 否则记在上面的 commit 和实际搬过来的
产物对不上，而那种错要等到下次排查时才会发现。
