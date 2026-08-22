// 制作 Riot 文档能力包(macOS)。
//
// 目标是让一台没装过 Python / Node / LibreOffice 的机器,只靠下载这一个包就能
// 创建和编辑 docx / xlsx / pptx / pdf。所有二进制从本机已安装的 Codex 运行时
// 提取,整棵树用相对路径互相引用,可以整体搬到任意位置。
//
// 用法:
//   node scripts/build-doc-pack.mjs              产出到能力包仓库的本平台目录
//   node scripts/build-doc-pack.mjs --out <目录>  换一个能力包仓库
//   node scripts/build-doc-pack.mjs --stage-only 只铺出目录,不打包(调试用)
//   node scripts/build-doc-pack.mjs --keep-stage 打包后保留铺出的目录
//
// Windows 包必须在 Windows 机器上用 build-doc-pack.ps1 单独制作:
// skia.node 之类的原生绑定是按平台编译的,没法交叉产出。两台机器各写各的
// 平台目录,最后用 scripts/doc-pack/merge-manifest.mjs 把清单并成一份。

import { execFileSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { adaptSkill, SKILL_NAMES } from './doc-pack/adapt-skills.mjs';

const PACK_NAME = 'doc-runtime';
const PACK_VERSION = '0.1.0';

// 能力包仓库。包体走它的 Releases(仓库里放不下:GitHub 单文件上限 100MB,
// 而一个包两百多 MB),清单直接从仓库文件读。
const PKG_REPO_SLUG = 'caiwuu/riot-pkg';
const RELEASE_TAG = `${PACK_NAME}-v${PACK_VERSION}`;

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const DIST = path.join(ROOT, 'dist', 'doc-pack');
const CACHE = path.join(DIST, '.cache');

const argv = process.argv.slice(2);
const args = new Set(argv);
const stageOnly = args.has('--stage-only');
const keepStage = args.has('--keep-stage') || stageOnly;
function argOf(name, fallback) {
  const i = argv.indexOf(name);
  return i >= 0 && argv[i + 1] ? argv[i + 1] : fallback;
}

// —— 平台 ——————————————————————————————————————————————————

if (process.platform !== 'darwin') {
  fail(`这个脚本只产出 macOS 包,当前平台是 ${process.platform}。Windows 请用 scripts/build-doc-pack.ps1。`);
}
const PLATFORM = `darwin-${process.arch}`;

// 成品落到能力包仓库的 `<包名>/<平台>/`。包名在上层是因为那个仓库以后不止装
// 文档这一个能力 —— 按平台分在最外面的话,一个包的东西会散在各平台目录里,想
// 知道"仓库里有哪些能力"得把每个平台目录都翻一遍。
//
// 铺出目录和下载缓存留在 dist/,那些是构建中间物,不该进包仓库。
const PKG_REPO = path.resolve(argOf('--out', process.env.RIOT_PKG_REPO ?? path.join(ROOT, '..', 'riot-pkg')));
const OUT = path.join(PKG_REPO, PACK_NAME, PLATFORM);

// —— 源:本机的 Codex 运行时 ——————————————————————————————

const DEPS = path.join(os.homedir(), '.cache/codex-runtimes/codex-primary-runtime/dependencies');
const PLUGINS = path.join(os.homedir(), '.codex/plugins/cache/openai-primary-runtime');

function requireSource(p, hint) {
  if (!fs.existsSync(p)) fail(`找不到 ${p}\n${hint}`);
  return p;
}
requireSource(DEPS, '需要本机装过 Codex 并让它把主运行时下载完(装完后随便用一次文档能力即可触发)。');
requireSource(PLUGINS, '需要本机装过 Codex 的文档插件。');

// 插件目录下每个 skill 各有一层版本号目录,取实际存在的那个。
function pluginSkillDir(name) {
  const base = path.join(PLUGINS, name);
  const versions = fs.existsSync(base) ? fs.readdirSync(base).filter((d) => /^\d/.test(d)) : [];
  if (versions.length === 0) fail(`Codex 插件缓存里没有 ${name}`);
  versions.sort();
  const version = versions[versions.length - 1];
  return { dir: path.join(base, version, 'skills', name), version };
}

// —— 铺出 ——————————————————————————————————————————————————

const STAGE = path.join(DIST, `${PACK_NAME}-${PACK_VERSION}-${PLATFORM}`);
fs.rmSync(STAGE, { recursive: true, force: true });
fs.mkdirSync(STAGE, { recursive: true });
fs.mkdirSync(CACHE, { recursive: true });

log(`制作 ${PACK_NAME} ${PACK_VERSION} (${PLATFORM})`);
log(`  源运行时: ${DEPS}`);
log(`  铺出:     ${STAGE}`);
log(`  成品:     ${OUT}`);

// 1. Python ——————————————————————————————————————————————
// 这份 Python 是可重定位的(sys.prefix 按二进制实际位置解析),整个目录搬走也能跑。
step('Python');
copyDir(path.join(DEPS, 'python'), path.join(STAGE, 'python'));
const SITE = path.join(STAGE, 'python/lib/python3.12/site-packages');
// artifact_tool_v2 是 artifact-tool 的 Python 侧实现,Riot 走 MCP(JS 侧),用不上。
// pandas 没有任何 skill 脚本引用。两个加起来 200MB。
for (const glob of ['artifact_tool_v2', 'pandas']) {
  for (const e of fs.readdirSync(SITE)) {
    if (e === glob || e.startsWith(`${glob}-`)) {
      fs.rmSync(path.join(SITE, e), { recursive: true, force: true });
    }
  }
}
pruneCaches(path.join(STAGE, 'python'));
report('python');

// 2. Node + artifact-tool ————————————————————————————————
step('Node 与 artifact-tool');
fs.mkdirSync(path.join(STAGE, 'node/node_modules/@oai'), { recursive: true });
fs.copyFileSync(path.join(DEPS, 'node/bin/node'), path.join(STAGE, 'node/node'));
fs.chmodSync(path.join(STAGE, 'node/node'), 0o755);
copyDir(
  path.join(DEPS, 'node/node_modules/@oai/artifact-tool'),
  path.join(STAGE, 'node/node_modules/@oai/artifact-tool'),
);
report('node');

// 3. LibreOffice ————————————————————————————————————————
step('LibreOffice');
copyDir(path.join(DEPS, 'native/libreoffice-headless/libreoffice'), path.join(STAGE, 'libreoffice'));
report('libreoffice');

// 4. Poppler ——————————————————————————————————————————————
// render_docx.py 通过 pdf2image 调 pdftoppm/pdfinfo,所以 poppler 是渲染门禁的必要件。
// 但 Codex 那份是个完整 conda 环境(bzip2、certutil 之类全在里面),只留 pdf 工具链。
step('Poppler');
copyDir(path.join(DEPS, 'native/poppler/poppler'), path.join(STAGE, 'poppler'));
const POP = path.join(STAGE, 'poppler');
for (const rel of ['include', 'conda-meta', 'ssl', 'sbin',
  'share/gir-1.0', 'share/locale', 'share/terminfo', 'share/man', 'share/doc', 'share/info']) {
  fs.rmSync(path.join(POP, rel), { recursive: true, force: true });
}
for (const e of fs.readdirSync(path.join(POP, 'lib'))) {
  if (e.endsWith('.a') || e === 'pkgconfig' || e === 'cmake') {
    fs.rmSync(path.join(POP, 'lib', e), { recursive: true, force: true });
  }
}
const KEEP_BINS = ['pdftoppm', 'pdfinfo', 'pdftocairo', 'pdfimages'];
const popBin = path.join(POP, 'bin');
for (const e of fs.readdirSync(popBin)) {
  if (!KEEP_BINS.includes(e)) fs.rmSync(path.join(popBin, e), { recursive: true, force: true });
}
report('poppler');

// 5. CJK 字体 ————————————————————————————————————————————
// 打包的 LibreOffice 自带 128 个字体但一个 CJK 都没有,而且构建成看不见系统字体。
// 不补这一步,中文文档会整片渲染成空白 —— 比不渲染更糟,因为模型看到空白会
// 以为是自己排版写错了,然后开始瞎改。
step('CJK 字体');
const fontDir = path.join(STAGE, 'libreoffice/LibreOfficeDev.app/Contents/Resources/fonts/truetype');
if (!fs.existsSync(fontDir)) fail(`LibreOffice 字体目录不存在: ${fontDir}`);
for (const f of await fetchCjkFonts()) {
  fs.copyFileSync(f, path.join(fontDir, path.basename(f)));
  log(`  装入 ${path.basename(f)}`);
}

// 6. shim ——————————————————————————————————————————————————
// 全部用相对于自身的路径转发,这样整个包搬到任何位置都不用改写。
//
// 分两个目录是有意的:
//   bin/  —— 全套。作为 RUNTIME_BIN_DIR 暴露,要用哪个就显式写全路径。
//   path/ —— 只有 soffice 和 poppler 那几个。这个目录才会被拼进 PATH。
//
// python3 和 node **不进 PATH**。进了的话,用户给会话设了 venv 时,
// 一句 `python3 manage.py` 会拿到包里这份、找不到项目依赖 —— 为了文档功能
// 把用户原本的 Python 工作流弄坏,是不划算的。反过来 soffice / pdftoppm
// 别处没有,放进 PATH 不会遮住任何东西,而 render_docx.py 内部就是按名字
// 调 soffice 的(它自带的运行时探测只认 Codex 的目录布局,在我们这儿探不到)。
step('shim');
const SHIMS = {
  soffice: 'libreoffice/LibreOfficeDev.app/Contents/MacOS/soffice',
  pdftoppm: 'poppler/bin/pdftoppm',
  pdfinfo: 'poppler/bin/pdfinfo',
  pdftocairo: 'poppler/bin/pdftocairo',
  pdfimages: 'poppler/bin/pdfimages',
  python3: 'python/bin/python3.12',
  python: 'python/bin/python3.12',
  node: 'node/node',
};
// 别处不提供、放进 PATH 不会遮住用户任何东西的那些。
const ON_PATH = ['soffice', 'pdftoppm', 'pdfinfo', 'pdftocairo', 'pdfimages'];

function writeShim(dir, name, targetFromRoot) {
  fs.mkdirSync(dir, { recursive: true });
  const rel = path.relative(dir, path.join(STAGE, targetFromRoot));
  fs.writeFileSync(path.join(dir, name), `#!/usr/bin/env bash
# 相对路径转发,整个能力包可以整体搬移。
set -euo pipefail
DIR="$(cd "$(dirname "\${BASH_SOURCE[0]}")" && pwd)"
exec "\${DIR}/${rel}" "$@"
`);
  fs.chmodSync(path.join(dir, name), 0o755);
}

for (const [name, target] of Object.entries(SHIMS)) {
  if (!fs.existsSync(path.join(STAGE, target))) fail(`shim ${name} 的目标不存在: ${target}`);
  writeShim(path.join(STAGE, 'bin'), name, target);
  if (ON_PATH.includes(name)) writeShim(path.join(STAGE, 'path'), name, target);
}
log(`  bin/ ${Object.keys(SHIMS).length} 个，path/ ${ON_PATH.length} 个`);

// 7. Skills ————————————————————————————————————————————————
step('Skills');
const skillsDir = path.join(STAGE, 'skills');
fs.mkdirSync(skillsDir, { recursive: true });
let runtimeVersion = null;
for (const name of SKILL_NAMES) {
  const { dir, version } = pluginSkillDir(name);
  runtimeVersion ??= version;
  copyDir(dir, path.join(skillsDir, name));
  adaptSkill(path.join(skillsDir, name), name, log);
}

// 8. pack.json ————————————————————————————————————————————
// Riot 的 Rust 侧读这个文件来接线。路径一律相对包根,由 Rust 解析成绝对路径。
step('pack.json');
const packJson = {
  name: PACK_NAME,
  version: PACK_VERSION,
  platform: PLATFORM,
  builtAt: new Date().toISOString(),
  sourceRuntime: runtimeVersion,
  env: {
    RUNTIME_NODE: 'bin/node',
    RUNTIME_NODE_MODULES: 'node/node_modules',
    RUNTIME_BIN_DIR: 'bin',
  },
  // 只有 path/ 进 PATH，理由见上面 shim 那一段。
  pathPrepend: ['path'],
  // 装完立刻实跑一遍。soffice 和 python3.12 只有 ad-hoc 签名,万一在用户机器上
  // 被系统拦下,要在他刚点完"安装"的时候就报出来,而不是几天后让模型撞上。
  selfCheck: [
    { command: 'bin/python3', args: ['-c', 'import docx, pptx, openpyxl, pdfplumber, reportlab'] },
    { command: 'bin/node', args: ['-v'] },
    { command: 'bin/soffice', args: ['--version'] },
    { command: 'bin/pdftoppm', args: ['-v'] },
  ],
  mcpServers: [
    {
      id: 'doc-artifact-tool',
      command: 'bin/node',
      args: ['node/node_modules/@oai/artifact-tool/dist/artifact-session-mcp/server.mjs'],
    },
  ],
  skills: SKILL_NAMES,
};
fs.writeFileSync(path.join(STAGE, 'pack.json'), `${JSON.stringify(packJson, null, 2)}\n`);

const installedSize = dirSize(STAGE);
log(`\n铺出完成: ${mb(installedSize)}`);

if (stageOnly) {
  log('--stage-only,跳过打包。');
  log(`\n本地验证: node scripts/doc-pack/verify-pack.mjs "${STAGE}"`);
  process.exit(0);
}

// 9. 打包 ——————————————————————————————————————————————————
// bsdtar 不一定带 --zstd,用管道更稳。
//
// COPYFILE_DISABLE=1 不能省:macOS 的 tar 默认会给带扩展属性的文件多写一个
// `._xxx` 的 AppleDouble 条目,而 `tar -tf` 列出来时又会把它藏起来 —— 所以
// 肉眼看归档是干净的,别的 tar 实现解出来却凭空多出一堆 `._` 文件。安装时
// "剥掉最外层目录"的判断因此会失效(顶层变成两个条目)。
step('打包 tar.zst');
fs.mkdirSync(OUT, { recursive: true });
const tarball = path.join(OUT, `${PACK_NAME}-${PACK_VERSION}-${PLATFORM}.tar.zst`);
fs.rmSync(tarball, { force: true });
execFileSync('bash', ['-c',
  `set -o pipefail; COPYFILE_DISABLE=1 tar -cf - -C ${sh(DIST)} ${sh(path.basename(STAGE))} | zstd -19 -T0 -q -o ${sh(tarball)}`,
], { stdio: 'inherit' });

const sha256 = sha256File(tarball);
const size = fs.statSync(tarball).size;
log(`  ${path.basename(tarball)}  ${mb(size)}  sha256 ${sha256.slice(0, 16)}…`);

// 10. 本平台清单 ————————————————————————————————————————————
// 这份只描述本机造出来的东西。跨平台的合并交给 merge-manifest.mjs —— 另一个
// 平台的包在另一台机器上,这里既算不出它的 sha256 也不该猜。
step('packs.json');
const manifestPath = path.join(OUT, 'packs.json');
const manifest = fs.existsSync(manifestPath)
  ? JSON.parse(fs.readFileSync(manifestPath, 'utf8'))
  : { schemaVersion: 1, packs: {} };
const entry = (manifest.packs[PACK_NAME] ??= { version: PACK_VERSION, platforms: {} });
entry.version = PACK_VERSION;
entry.platforms[PLATFORM] = {
  url: `https://github.com/${PKG_REPO_SLUG}/releases/download/${RELEASE_TAG}/${path.basename(tarball)}`,
  sha256,
  size,
  installedSize,
};
fs.writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
log(`  ${manifestPath}`);

if (!keepStage) fs.rmSync(STAGE, { recursive: true, force: true });

log(`\n完成。压缩 ${mb(size)},安装后 ${mb(installedSize)}。`);
log(`  ${tarball}`);
log(`\n验证: node scripts/doc-pack/verify-pack.mjs <解压后的目录>`);
log(`发布: node scripts/doc-pack/publish.mjs`);

// —— 辅助 ——————————————————————————————————————————————————

// 只取 Regular 和 Bold:全字重 90MB 里绝大部分用不上,这两个覆盖正文和标题。
async function fetchCjkFonts() {
  const url = 'https://github.com/notofonts/noto-cjk/releases/download/Sans2.004/08_NotoSansCJKsc.zip';
  const zip = path.join(CACHE, 'NotoSansCJKsc.zip');
  if (!fs.existsSync(zip) || fs.statSync(zip).size < 1024 * 1024) {
    log('  下载 Noto Sans CJK SC（约 90MB,已缓存则跳过）…');
    const res = await fetch(url);
    if (!res.ok) fail(`字体下载失败: ${res.status} ${url}`);
    fs.writeFileSync(zip, Buffer.from(await res.arrayBuffer()));
  }
  const out = path.join(CACHE, 'noto');
  fs.rmSync(out, { recursive: true, force: true });
  execFileSync('unzip', ['-o', '-q', zip, '-d', out]);
  const want = ['NotoSansCJKsc-Regular.otf', 'NotoSansCJKsc-Bold.otf'];
  const found = [...walk(out)].filter((f) => want.includes(path.basename(f)));
  if (found.length !== want.length) {
    fail(`字体包里没找到 ${want.join(' / ')},实际有: ${[...walk(out)].map((f) => path.basename(f)).join(', ')}`);
  }
  return found;
}

// -c 走 APFS clone,同卷内近乎瞬时且不额外占盘。
function copyDir(src, dest) {
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  execFileSync('cp', ['-Rpc', src, dest]);
}

function pruneCaches(dir) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) {
      if (e.name === '__pycache__') fs.rmSync(p, { recursive: true, force: true });
      else pruneCaches(p);
    } else if (e.name.endsWith('.pyc')) {
      fs.rmSync(p, { force: true });
    }
  }
}

function* walk(dir) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) yield* walk(p);
    else if (e.isFile()) yield p;
  }
}

function dirSize(dir) {
  return Number(execFileSync('du', ['-sk', dir]).toString().split('\t')[0]) * 1024;
}

function sha256File(file) {
  const h = crypto.createHash('sha256');
  h.update(fs.readFileSync(file));
  return h.digest('hex');
}

function mb(bytes) { return `${(bytes / 1024 / 1024).toFixed(0)}MB`; }
function sh(s) { return `'${s.replace(/'/g, `'\\''`)}'`; }
function log(m) { console.log(m); }
function step(m) { console.log(`\n[${m}]`); }
function report(name) { log(`  ${name}: ${mb(dirSize(path.join(STAGE, name)))}`); }
function fail(m) { console.error(`\n错误: ${m}\n`); process.exit(1); }
