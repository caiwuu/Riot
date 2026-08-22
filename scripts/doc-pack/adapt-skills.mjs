// 把 Codex 的文档类 skill 改写成 Riot 能直接用的版本。
//
// 为什么要脚本化而不是手工改一遍:Codex 每次升级都会重写这些 SKILL.md,
// 手工改的结果没法跟着升级走。更要紧的是 —— 每条改写都声明成"必须命中",
// 上游改了措辞就直接构建失败,而不是把 Riot 里根本不存在的宿主工具
// (list_artifact_templates 之类) 悄悄塞进模型的上下文里。
//
// 三类差异需要抹平:
//   1. 宿主工具:Codex 独有的模板选择器、操作打点、文件引用语法
//   2. 运行时发现:load_workspace_dependencies 换成 Riot 预置的 RUNTIME_* 环境变量
//   3. 云端集成:Google Drive/Docs/Sheets/Slides 走的是 Codex 的 connector

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

// —— 文本工具 ——————————————————————————————————————————————

// 删掉一整个 markdown 章节(从标题到下一个同级或更高级标题)。
function dropSection(text, heading, level = 2) {
  const hashes = '#'.repeat(level);
  const lines = text.split('\n');
  const start = lines.findIndex((l) => l.trim() === `${hashes} ${heading}`);
  if (start === -1) return null;
  let end = lines.length;
  for (let i = start + 1; i < lines.length; i++) {
    const m = /^(#{1,6})\s/.exec(lines[i]);
    if (m && m[1].length <= level) { end = i; break; }
  }
  lines.splice(start, end - start);
  return lines.join('\n');
}

// 整段替换一个章节的正文,保留标题。
function replaceSectionBody(text, heading, level, body) {
  const hashes = '#'.repeat(level);
  const lines = text.split('\n');
  const start = lines.findIndex((l) => l.trim() === `${hashes} ${heading}`);
  if (start === -1) return null;
  let end = lines.length;
  for (let i = start + 1; i < lines.length; i++) {
    const m = /^(#{1,6})\s/.exec(lines[i]);
    if (m && m[1].length <= level) { end = i; break; }
  }
  lines.splice(start + 1, end - start - 1, '', ...body.trim().split('\n'), '');
  return lines.join('\n');
}

function replaceAll(text, needle, replacement) {
  if (!text.includes(needle)) return null;
  return text.split(needle).join(replacement);
}

// 把命令行里裸的 python / node 换成能力包里的那一份。
//
// 包里的 python3 和 node 刻意不进 PATH(见 build-doc-pack.mjs 的 shim 那段),
// 所以文档里照抄 Codex 的 `python scripts/foo.py` 会拿到用户自己的解释器 ——
// 那里面没有 python-docx,脚本以 ImportError 挂掉,而模型看到的错误跟真正的
// 原因(解释器选错了)完全对不上,大概率会开始瞎试 pip install。
//
// 只改行首的调用。行内提到的 `python-docx`、`python3 -m pip` 之类不动:
// 前者是包名,后者本来就该失败(能力包里不该装东西)。
function rewriteInterpreterCalls(text) {
  return text
    .replace(/^(\s*)python3?(?= +[\w$./"'-])/gm, '$1"$RUNTIME_BIN_DIR/python3"')
    .replace(/^(\s*)node(?= +[\w$./"'-])/gm, '$1"$RUNTIME_NODE"');
}

function replaceRe(text, re, replacement) {
  if (!re.test(text)) return null;
  return text.replace(re, replacement);
}

// 换掉 frontmatter 里的 description。Riot 对 description 有 250 字符硬上限,
// Codex 那边没有,所以长的必须重写。
function setDescription(text, description) {
  if (description.length > 250) {
    throw new Error(`description 超过 250 字符 (${description.length}): ${description}`);
  }
  const re = /^(---\n(?:.*\n)*?description:\s*)(?:"(?:[^"\\]|\\.)*"|.*)(\n)/;
  if (!re.test(text)) return null;
  return text.replace(re, (_m, head, tail) => `${head}${JSON.stringify(description)}${tail}`);
}

// —— 共用片段 ——————————————————————————————————————————————

const RUNTIME_ENV = `Riot 在启动工具进程时已经把文档运行时接好了,不需要任何发现或安装步骤。三个环境变量总是可用:

- \`RUNTIME_BIN_DIR\` — 包内所有可执行文件所在目录
- \`RUNTIME_NODE\` — Node 可执行文件
- \`RUNTIME_NODE_MODULES\` — 含 \`@oai/artifact-tool\` 的包目录

**跑 Python 必须写成 \`"$RUNTIME_BIN_DIR/python3"\`,跑 Node 必须写成 \`"$RUNTIME_NODE"\`。** 直接敲 \`python3\` 或 \`node\` 拿到的是用户自己的解释器 —— 那里面没有 python-docx、python-pptx、openpyxl,脚本会以 ImportError 失败。这两个刻意不放进 \`PATH\`,免得盖掉用户项目的虚拟环境。

\`pdftoppm\`、\`pdfinfo\` 在 \`PATH\` 上,按名字直接调即可。LibreOffice **不在** \`PATH\` 上 —— 它的目录里带着自己的 \`python.exe\`,放进 \`PATH\` 会盖掉用户的 Python。要用它就调 skill 自带的 \`render_docx.py\`,那个脚本自己会从 \`RUNTIME_BIN_DIR\` 找。

不要用 \`brew\`、\`apt\`、\`pip install\`、\`npm install\` 装任何东西 —— 目标用户机器上没有开发环境,装不上,也不需要装。如果某个 \`RUNTIME_*\` 变量缺失,说明文档能力包没装好,直接报阻塞,不要自己找替代路径。`;

const DELIVERY = `交付时用普通 Markdown 链接或绝对路径指向最终文件,正文里说明改了什么。渲染出的 PNG 和中间 PDF 只用于你自己的质检,除非用户明确要,否则不要交付。`;

const MCP_CONTRACT = `- 电子表格的创建和编辑一律走 \`artifact_session_run\` 这个 MCP 工具,它就是 \`@oai/artifact-tool\` 的服务端封装,API 与 \`artifact_tool_docs/\` 里写的完全一致。
- **每次调用都必须显式传 \`target\` 绝对路径。** Codex 会从会话 id 推断输出位置,Riot 不提供这个,漏传就会写到你预期之外的地方。创建新文件时同时传 \`create: true\`。
- \`code\` 参数里的脚本运行在 workbook 上下文中,可以直接用 \`workbook\`,通过 \`return\` 把要回读的值带出来。
- 不要用 \`openpyxl\`、\`xlsxwriter\`、\`pandas.ExcelWriter\` 来写工作簿 —— 它们不做公式求值,存出来的公式没有缓存值,Excel 之外的工具全读成空。反向校验时用 \`openpyxl\` 读是可以的。
- 需要在工作簿之外做数据处理时,用能力包里的 Python 存 JSON/CSV 中间结果,再由 \`artifact_session_run\` 写进工作簿。可审计的计算要以公式形式留在表里。`;

// —— 各 skill 的改写规则 ————————————————————————————————————

// required 为 true 的规则没命中就抛错,防止上游改版后静默漏改。
const SKILLS = {
  documents: {
    description:
      '创建、编辑、审阅和批注 .docx / Word 文档。带强制的渲染校验流程:用 render_docx.py 生成页面 PNG,逐页看过确认排版无缺陷后再交付。',
    rules: [
      ['drop-template-picker', (t) => dropSection(t, 'Artifact Template Selection')],
      ['drop-google-docs', (t) => dropSection(t, 'Google Docs-targeted output')],
      ['drop-citations', (t) => dropSection(t, 'Final response citations')],
      ['rewrite-tools-contract', (t) => replaceSectionBody(t, 'Tools + Contract', 2, `${RUNTIME_ENV}

- 生成文档和做确定性的 OOXML 编辑时,用本 skill 包里自带的 Python 助手脚本。
- 从可写的工作目录或临时目录运行构建脚本,不要在能力包目录里就地跑。
- ${DELIVERY}`)],
      // 正文多处把 Google Docs 当成一等交付目标,但那条链路依赖 Codex 的 Drive connector
      ['soften-google-docs-mentions', (t) => replaceAll(t,
        'Docs-targeted document artifacts **in this container environment** and verify\nthem visually.',
        'artifacts 并做可视化校验。\n\n本 skill 只产出本地 `.docx`。Codex 版本里的 Google Docs 导入链路依赖它的 Drive connector,Riot 没有,已移除。')],
      ['fix-template-following', (t) => replaceAll(t,
        'The render gate and Google Docs import contract\nstill apply. For a Google Docs-targeted result, record any change made by the\nrequired title sanitizer as an intentional fidelity deviation.',
        'The render gate still applies.')],
    ],
  },

  spreadsheets: {
    description:
      '创建、编辑、分析和校验 .xlsx / .xls / .csv / .tsv 电子表格,通过 artifact_session_run 工具完成,支持真实公式求值。不用于控制正在运行的 Excel 应用。',
    rules: [
      ['drop-template-picker', (t) => dropSection(t, 'Artifact Template Selection')],
      ['drop-citations', (t) => dropSection(t, 'Final response citations')],
      ['rewrite-tools-contract', (t) => replaceSectionBody(t, 'Tools + Contract Requirements', 2, `${MCP_CONTRACT}
- 复杂表格任务用 \`TodoWrite\` 记待办。
- ${DELIVERY}`)],
      ['drop-google-sheets-routing', (t) => replaceSectionBody(t, 'Decision Boundary', 2,
        '默认用 `artifact_session_run` 创建和编辑电子表格。Codex 版本里的 Google Sheets 链路依赖它的 Drive connector,Riot 没有,已移除。')],
      ['runtime-env', (t) => replaceAll(t, '## Other documents', `## 运行时

${RUNTIME_ENV}

## Other documents`)],
    ],
  },

  presentations: {
    description:
      '创建和编辑 .pptx 演示文稿,通过 artifact_session_run 工具完成。带渲染校验:把幻灯片渲成 PNG 逐张检查溢出和排版问题后再交付。',
    rules: [
      ['drop-google-slides', (t) => dropSection(t, 'Google Slides Routing')],
      ['drop-template-picker', (t) => dropSection(t, 'Artifact Template Selection')],
      ['rewrite-environment', (t) => replaceSectionBody(t, 'Environment', 3, `在可写的工作目录或临时目录里干活,遵守用户给的输出路径。

设置:

- \`SKILL_DIR=<本 skill 的绝对路径>\`
- \`TMP_DIR=<工作目录下的临时构建目录绝对路径>\`
- \`FINAL_PPTX=<最终 .pptx 的绝对路径>\`

${RUNTIME_ENV}

自己写的 \`.mjs\` 构建脚本如果用了裸的 \`@oai/artifact-tool\` import,在可写构建目录里建一个指向 \`RUNTIME_NODE_MODULES\` 的 \`node_modules\` 符号链接(Windows 用 junction)。不要改能力包目录本身。skill 自带的脚本直接读 \`RUNTIME_*\`,不需要这个链接。

也可以用 \`artifact_session_run\` MCP 工具替代自己写构建脚本 —— **调用时必须显式传 \`target\` 绝对路径**,Riot 不提供 Codex 那套会话 id 推断。

中间产物放 \`$TMP_DIR\`,只有最终交付物放输出位置,全部用绝对路径。\`$TMP_DIR\` 里生成的说明性文本用 \`.txt\`,\`.md\` 留给 skill 自带资源。`)],
    ],
  },

  pdf: {
    description:
      '创建、填写和校验 PDF。用 reportlab 生成,pdfplumber / pypdf 读取和填 AcroForm 表单,pdftoppm 渲成 PNG 后逐页目检再交付。',
    rules: [
      ['drop-citations', (t) => dropSection(t, 'Final response citations')],
      ['rewrite-dependencies', (t) => replaceSectionBody(t, 'Dependencies', 2, RUNTIME_ENV)],
      ['rewrite-env', (t) => replaceSectionBody(t, 'Environment', 2,
        '无需额外环境变量,上面那些由 Riot 预置。')],
    ],
  },
};

// —— 助手脚本的改写 ————————————————————————————————————————

// 插在 render_docx.py 的 import 之后。原版把 soffice 和 poppler 都按名字交给
// PATH 去找;presentations 那边的 runtime_helpers.py 早就改成从 RUNTIME_BIN_DIR
// 解析绝对路径了,这里补上同样的能力。
//
// 非做不可的原因在 Windows:LibreOffice 的 program 目录里躺着一个 python.exe,
// 想让 soffice 上 PATH 就得把那个目录整个塞进去,于是用户敲 python 会拿到
// LibreOffice 内部那份。解析成绝对路径就没这个两难 —— DLL 按 exe 所在目录搜,
// 不需要 PATH 配合。
const RENDER_DOCX_RESOLVER = `

def _runtime_binary(name: str) -> str:
    """从 Riot 能力包里定位一个原生可执行文件,回落到 PATH。

    Windows 上包里的 bin 目录只放一份 native-executables.json 清单,真正的
    exe 在 native/ 下(动态库都在它旁边),所以要按清单跳过去。
    """

    bin_dir = os.environ.get("RUNTIME_BIN_DIR")
    if bin_dir:
        if os.name == "nt":
            import json

            try:
                with open(os.path.join(bin_dir, "native-executables.json"), encoding="utf-8") as f:
                    rel = json.load(f).get(name)
            except (OSError, ValueError):
                rel = None
            if rel:
                resolved = os.path.realpath(os.path.join(bin_dir, rel))
                if os.path.isfile(resolved):
                    return resolved
        candidate = os.path.join(bin_dir, name)
        if os.path.isfile(candidate):
            return candidate
    # 没装能力包也别直接崩:本机自己装了 LibreOffice 的话仍然能渲染。
    return shutil.which(name) or name


def _poppler_dir() -> str | None:
    """pdf2image 要的是目录;给 None 让它自己去 PATH 找。"""

    pdftoppm = _runtime_binary("pdftoppm")
    return os.path.dirname(pdftoppm) if os.path.isabs(pdftoppm) else None

`;

// soffice 在三处命令列表里各出现一次(DOCX→PDF、DOCX→ODT、ODT→PDF 三条链路)。
// 数量对不上说明上游改了渲染流程,宁可构建失败也不要漏改一处。
const SOFFICE_CALL_SITES = 3;

function patchRenderDocx(text) {
  const anchor = 'from pdf2image import convert_from_path, pdfinfo_from_path\n';
  if (!text.includes(anchor)) return null;
  let out = text.replace(anchor, anchor + RENDER_DOCX_RESOLVER);

  const sites = out.match(/^(\s*)"soffice",$/gm) ?? [];
  if (sites.length !== SOFFICE_CALL_SITES) {
    throw new Error(`render_docx.py 里的 soffice 调用点有 ${sites.length} 处,预期 ${SOFFICE_CALL_SITES} 处`);
  }
  out = out.replace(/^(\s*)"soffice",$/gm, '$1_runtime_binary("soffice"),');

  const before = out;
  out = out
    .replace('pdfinfo_from_path(pdf_path)', 'pdfinfo_from_path(pdf_path, poppler_path=_poppler_dir())')
    .replace(/(convert_from_path\(\n\s*pdf_path,\n)(\s*)/, `$1$2poppler_path=_poppler_dir(),\n$2`);
  if (out === before) return null;

  return out;
}

// 各 skill 里需要就地改写的助手脚本。
const SCRIPT_PATCHES = {
  documents: [['render-docx-resolve-binaries', 'render_docx.py', patchRenderDocx]],
};

// 四个 skill 都要做的收尾改写。
const COMMON_RULES = [
  // 打点脚本回调的是 Codex 宿主,Riot 里跑不通;整段(说明 + 代码块)一起删。
  ['drop-operation-marker', (t) => {
    if (!t.includes('mark_artifact_operation_started')) return t;
    const cleaned = t.replace(
      /\n[^\n]*mark_artifact_operation_started[^\n]*\n(?:\n?```bash\n[\s\S]*?\n```\n)?/g,
      '\n',
    );
    return cleaned.includes('mark_artifact_operation_started') ? null : cleaned;
  }],
  // Codex 的行内文件引用语法,Riot 用普通 Markdown 链接。
  ['drop-file-citations', (t) => {
    if (!t.includes('codex-file-citation')) return t;
    const cleaned = t
      .replace(/^.*codex-file-citation.*$\n?/gm, '')
      .replace(/\n{3,}/g, '\n\n');
    return cleaned.includes('codex-file-citation') ? null : cleaned;
  }],
  ['pin-interpreters', rewriteInterpreterCalls],
  ['map-plan-tool', (t) => t.split('`update_plan`').join('`TodoWrite`')],
  ['map-ask-tool', (t) => t.split('`request_user_input`').join('`AskUserQuestion`')],
  ['drop-dependency-loader', (t) => t.split('`load_workspace_dependencies`').join('预置的 `RUNTIME_*` 环境变量')],
  // 视觉检查在 Riot 里就是用 Read 工具读 PNG,模型自己看。
  ['name-image-tool', (t) => t.split('Open the PNGs').join('用 `Read` 工具打开 PNG')],
  ['rename-host', (t) => t.replace(/\bCodex\b/g, 'Riot')],
];

// 改完之后不该再出现的东西。留一个在正文里,模型就会去找不存在的工具然后卡住。
const FORBIDDEN = [
  'mark_artifact_operation_started',
  'list_artifact_templates',
  'choose_artifact_template',
  'codex-file-citation',
  'load_workspace_dependencies',
  'mcp__codex_apps',
  'update_plan',
  'request_user_input',
];

const MAX_DESCRIPTION = 250;
const MAX_BODY = 64 * 1024;

/**
 * 就地改写一个 skill 目录。返回本次应用的规则名,便于构建日志核对。
 */
export function adaptSkill(skillDir, name, log = () => {}) {
  const spec = SKILLS[name];
  if (!spec) throw new Error(`没有为 skill "${name}" 定义改写规则`);

  const file = path.join(skillDir, 'SKILL.md');
  let text = fs.readFileSync(file, 'utf8');
  const applied = [];

  for (const [id, fn] of [...spec.rules, ...COMMON_RULES]) {
    const next = fn(text);
    if (next === null || next === undefined) {
      throw new Error(
        `[${name}] 改写规则 "${id}" 没有命中。` +
          `多半是上游 Codex skill 改了措辞,需要同步更新 scripts/doc-pack/adapt-skills.mjs。`,
      );
    }
    if (next !== text) applied.push(id);
    text = next;
  }

  const desc = setDescription(text, spec.description);
  if (desc === null) throw new Error(`[${name}] frontmatter 里找不到 description`);
  text = desc.replace(/\n{3,}/g, '\n\n');

  for (const bad of FORBIDDEN) {
    if (text.includes(bad)) throw new Error(`[${name}] 改写后仍残留 Codex 宿主调用: ${bad}`);
  }
  const body = text.replace(/^---\n[\s\S]*?\n---\n/, '');
  if (Buffer.byteLength(body, 'utf8') > MAX_BODY) {
    throw new Error(`[${name}] 正文 ${Buffer.byteLength(body, 'utf8')} 字节,超过 Riot 的 64KB 上限`);
  }
  if (spec.description.length > MAX_DESCRIPTION) {
    throw new Error(`[${name}] description 超过 ${MAX_DESCRIPTION} 字符`);
  }

  fs.writeFileSync(file, text);

  // 依赖 Codex 宿主的附属文件一并删掉,免得模型顺着 SKILL.md 之外的线索找过去。
  const removed = [];
  for (const rel of ['container_tools/mark_artifact_operation_started.mjs', 'routing']) {
    const p = path.join(skillDir, rel);
    if (fs.existsSync(p)) { fs.rmSync(p, { recursive: true, force: true }); removed.push(rel); }
  }
  const ct = path.join(skillDir, 'container_tools');
  if (fs.existsSync(ct) && fs.readdirSync(ct).length === 0) fs.rmSync(ct, { recursive: true });

  for (const [id, rel, fn] of SCRIPT_PATCHES[name] ?? []) {
    const target = path.join(skillDir, rel);
    if (!fs.existsSync(target)) throw new Error(`[${name}] 改写规则 "${id}" 的目标不存在: ${rel}`);
    const next = fn(fs.readFileSync(target, 'utf8'));
    if (next === null || next === undefined) {
      throw new Error(
        `[${name}] 助手脚本改写 "${id}" 没有命中。` +
          `多半是上游 Codex 改了 ${rel},需要同步更新 scripts/doc-pack/adapt-skills.mjs。`,
      );
    }
    fs.writeFileSync(target, next);
    applied.push(id);
  }

  const swept = sweepSupportingDocs(skillDir);
  regenerateManifest(skillDir);

  log(`  ${name}: ${applied.length} 条改写, 删除 ${removed.length} 项, 附属文档 ${swept} 处, description ${spec.description.length} 字符, 正文 ${(Buffer.byteLength(body, 'utf8') / 1024).toFixed(1)}KB`);
  return { applied, removed, swept };
}

// 渐进披露会让模型读到 SKILL.md 之外的任务文档,宿主称呼也要一并换掉。
// 但 codex-grid-layout-library 是模板的标识符(目录名、manifest 里的 id 都引用它),
// 改了会把模板引用打断,所以整个模板资源目录跳过。
function sweepSupportingDocs(skillDir) {
  let count = 0;
  for (const file of walk(skillDir)) {
    if (!file.endsWith('.md')) continue;
    if (file.includes('builtin_templates') || file.includes('codex-grid-layout-library')) continue;
    if (path.basename(file) === 'SKILL.md') continue;
    const before = fs.readFileSync(file, 'utf8');
    const after = rewriteInterpreterCalls(before).replace(/\bCodex\b/g, 'Riot');
    if (after !== before) { fs.writeFileSync(file, after); count++; }
  }
  return count;
}

// manifest.txt 是给 Codex 的按需下载工具用的文件清单。Riot 里整包都在本地,
// 但 SKILL.md 仍然提到它,所以重建成与实际内容一致,免得指向已删掉的文件。
function regenerateManifest(skillDir) {
  const manifest = path.join(skillDir, 'manifest.txt');
  if (!fs.existsSync(manifest)) return;
  const entries = [...walk(skillDir)]
    .map((f) => path.relative(skillDir, f))
    .filter((r) => r !== 'manifest.txt' && r !== 'SKILL.md')
    .sort();
  fs.writeFileSync(manifest, `${entries.join('\n')}\n`);
}

function* walk(dir) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) yield* walk(p);
    else if (e.isFile()) yield p;
  }
}

export const SKILL_NAMES = Object.keys(SKILLS);

// 直接跑时当 CLI 用:`node adapt-skills.mjs <skills 根目录>`,把四个 skill 全改一遍。
// build-doc-pack.ps1 走这条路 —— PowerShell 没法 import ESM 模块。
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = process.argv[2];
  if (!root) {
    console.error('用法: node adapt-skills.mjs <skills 根目录>');
    process.exit(2);
  }
  for (const name of SKILL_NAMES) {
    adaptSkill(path.join(root, name), name, (m) => console.log(m));
  }
}
