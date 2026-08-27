#!/usr/bin/env node
/**
 * 把 file-viewer fork 的**构建产物**同步进 vendor/file-viewer。
 *
 * 用法：node scripts/vendor-file-viewer.mjs [fork 路径]     默认 ../file-viewer
 *
 * ── 为什么 vendor 产物而不是源码 ────────────────────────────────
 *
 * 这些包的 main/types 指向 dist/，而它们的构建不是一句 tsc：core 要跑 ESM
 * 扩展名修复脚本，pptx 和 spreadsheet 还要用 esbuild 打 worker，而且 10 个
 * 包之间有严格顺序。搬源码就等于把这一整条链接进 Riot 的前端构建 —— 每个
 * 只想改一行 CSS 的人都得先编一遍文档查看器。
 *
 * 搬产物则构建链一行不动：pnpm 把它们当普通 workspace 成员链进去。代价是
 * 仓库里多一批生成文件、升级 diff 不可读 —— 但 vendor 的第三方产物本来也
 * 不逐行看，`files: ["dist", …]` 说明这正是 npm 会发布的内容。
 *
 * ── 为什么必须是 workspace 成员而不是 link: 覆盖 ──────────────
 *
 * `[约束]` 之前用的是 `overrides: link:../file-viewer/...`，指向机器上的
 * fork。CI 上那个目录不存在，pnpm 照样把软链建出来、`--frozen-lockfile`
 * 退 0，直到 tsc 才报 "Cannot find module" —— 装不上却装得"成功"，症状推迟
 * 到最后一刻。而且 `link:` **不会安装被链包自己的依赖**（jszip、pdfjs-dist
 * 那些），之前是靠 fork 自己的 node_modules 兜住的。改成 workspace 成员，
 * pnpm 才会把它们的依赖也解进 Riot 的 lockfile。
 *
 * 所以这里只搬 package.json + dist + 说明文件，并且**剥掉 scripts 和
 * devDependencies**：构建脚本指向没搬过来的 scripts/ 目录，留着只会在
 * `pnpm install` 的生命周期钩子上炸；devDeps（typescript、esbuild）在不
 * 构建的前提下纯属拖慢安装。
 */

import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const DEST = join(ROOT, "vendor", "file-viewer");
const FORK = resolve(process.argv[2] ?? join(ROOT, "..", "file-viewer"));

/** Riot 直接用的 7 个，加上它们的 workspace 依赖闭包。 */
const PACKAGES = [
  "packages/core",
  "packages/components/react",
  "packages/presets/vite-plugin",
  "packages/renderers/doc",
  "packages/renderers/pdf",
  "packages/renderers/pptx",
  "packages/renderers/presentation",
  "packages/renderers/spreadsheet",
  "packages/renderers/text",
  "packages/renderers/word",
];

/** 不构建就没意义、留着还会坏事的字段。 */
const DROP_FIELDS = ["scripts", "devDependencies"];

function git(...args) {
  return execFileSync("git", args, { cwd: FORK, encoding: "utf8" }).trim();
}

function main() {
  if (!existsSync(join(FORK, "package.json"))) {
    console.error(`找不到 fork：${FORK}\n用法：node scripts/vendor-file-viewer.mjs [fork 路径]`);
    process.exit(1);
  }
  if (git("status", "--porcelain")) {
    console.error(`fork 工作区不干净（${FORK}）—— 先提交或清理，否则记进 NOTICE 的 commit 对不上实际产物`);
    process.exit(1);
  }

  const head = git("rev-parse", "HEAD");
  const base = git("merge-base", "HEAD", "origin/main");
  const local = git("log", "--oneline", `${base}..HEAD`);

  rmSync(join(DEST, "packages"), { recursive: true, force: true });

  const names = [];
  for (const rel of PACKAGES) {
    const src = join(FORK, rel);
    const dst = join(DEST, rel);
    const pkg = JSON.parse(readFileSync(join(src, "package.json"), "utf8"));
    if (!existsSync(join(src, "dist"))) {
      console.error(`${pkg.name} 没有 dist/ —— 先在 fork 里跑 pnpm build`);
      process.exit(1);
    }
    mkdirSync(dst, { recursive: true });
    cpSync(join(src, "dist"), join(dst, "dist"), { recursive: true });
    for (const f of ["README.md", "README.en.md"]) {
      if (existsSync(join(src, f))) cpSync(join(src, f), join(dst, f));
    }
    for (const k of DROP_FIELDS) delete pkg[k];
    writeFileSync(join(dst, "package.json"), `${JSON.stringify(pkg, null, 2)}\n`);
    names.push(`${pkg.name}@${pkg.version}`);
    console.log(`  ${pkg.name}`);
  }

  cpSync(join(FORK, "LICENSE"), join(DEST, "LICENSE"));
  writeFileSync(
    join(DEST, "NOTICE.md"),
    `# vendor/file-viewer

文件预览用的渲染库，来自 [flyfish-dev/file-viewer](https://github.com/flyfish-dev/file-viewer)
（Apache-2.0，许可证原文见同目录 \`LICENSE\`）。

**这里放的是构建产物（\`dist/\`），不是源码。** 理由、以及为什么必须是 pnpm
workspace 成员而不是 \`link:\` 覆盖，见 \`scripts/vendor-file-viewer.mjs\` 的文件头。

## 版本溯源

| | |
|---|---|
| 上游基线 | \`${base}\` |
| 同步自 | \`${head}\` |

fork 相对上游的本地定制：

\`\`\`
${local || "（无）"}
\`\`\`

\`[约束]\` 这些定制**只存在于 fork 里**，仓库里只有它们编译后的样子。fork 丢了
就找不回来 —— 要么把 fork 推到远端，要么在这里改成 vendor 源码。

## 收录的包

${names.map((n) => `- \`${n}\``).join("\n")}

\`@file-viewer/docx\` 和 \`@file-viewer/ppt\` 不在此列：它们是正常发布到 npm 的包
（\`^0.3.27\` / \`0.3.3\`），由 lockfile 正常解析。

## 怎么升级

\`\`\`bash
cd ../file-viewer && git pull --rebase && pnpm install && pnpm build
cd -             && node scripts/vendor-file-viewer.mjs && pnpm install
\`\`\`

脚本会拒绝在 fork 工作区不干净时运行 —— 否则记在上面的 commit 和实际搬过来的
产物对不上，而那种错要等到下次排查时才会发现。
`,
  );

  console.log(`\n已同步 ${PACKAGES.length} 个包 → vendor/file-viewer（fork ${head.slice(0, 8)}）`);
  console.log("接下来：pnpm install");
}

main();
