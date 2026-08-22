// 文档能力包冒烟测试。
//
// 关键点是 PATH 里只放能力包的 bin 和系统基础目录 —— 不含 homebrew、nvm、pyenv。
// 在开发机上跑也能真实反映"一台没有开发环境的机器"上的表现,否则很容易出现
// 包里少了东西但被开发机上的全局 Python / Node 兜住、发布后才炸的情况。
//
// 用法: node scripts/doc-pack/verify-pack.mjs <pack 目录>

import { execFileSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import readline from 'node:readline';

const PACK = path.resolve(process.argv[2] ?? '');
if (!PACK || !fs.existsSync(path.join(PACK, 'pack.json'))) {
  console.error('用法: node scripts/doc-pack/verify-pack.mjs <pack 目录>');
  console.error('（该目录下需要有 pack.json）');
  process.exit(2);
}

const pack = JSON.parse(fs.readFileSync(path.join(PACK, 'pack.json'), 'utf8'));
const WORK = fs.mkdtempSync(path.join(os.tmpdir(), 'riot-pack-verify-'));
const isWin = process.platform === 'win32';
const sep = isWin ? ';' : ':';

const BASE_PATH = isWin
  ? ['C:\\Windows\\system32', 'C:\\Windows']
  : ['/usr/bin', '/bin', '/usr/sbin', '/sbin'];

// 严格照 Riot 的注入方式来：PATH 只拿 pack.json 的 pathPrepend（不含
// python3 / node），解释器通过 RUNTIME_* 显式定位。放宽任何一条，验证就会
// 被开发机上的全局 Python 兜住，测不出真实用户会遇到的问题。
const ENV = {
  ...Object.fromEntries(Object.entries(process.env).filter(([k]) =>
    !['PYTHONPATH', 'PYTHONHOME', 'VIRTUAL_ENV', 'NODE_PATH', 'NODE_OPTIONS'].includes(k))),
  PATH: [...(pack.pathPrepend ?? []).map((p) => path.join(PACK, p)), ...BASE_PATH].join(sep),
  ...Object.fromEntries(Object.entries(pack.env ?? {}).map(([k, v]) => [k, path.join(PACK, v)])),
};

let pass = 0;
let fail = 0;
const failures = [];
function chk(ok, label, detail) {
  if (ok) { pass++; console.log(`  PASS  ${label}`); }
  else { fail++; failures.push(label); console.log(`  FAIL  ${label}${detail ? `\n        ${String(detail).split('\n').join('\n        ')}` : ''}`); }
}
function section(t) { console.log(`\n${'='.repeat(60)}\n ${t}\n${'='.repeat(60)}`); }
function run(cmd, argv, opts = {}) {
  return execFileSync(cmd, argv, { env: ENV, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], ...opts });
}
function tryRun(cmd, argv, opts) {
  try { return { ok: true, out: run(cmd, argv, opts) }; }
  catch (e) { return { ok: false, out: `${e.message}\n${e.stderr ?? ''}` }; }
}

const BIN_DIR = ENV.RUNTIME_BIN_DIR ?? path.join(PACK, 'bin');
/// bin 目录里的 shim。两个平台上都是 bash 脚本 —— Riot 在 Windows 上执行模型
/// 命令用的是 Git bash。
const shim = (n) => path.join(BIN_DIR, n);

/// 能直接交给 CreateProcess / execve 的真二进制。
///
/// Windows 上不能拿 shim 当程序名:CreateProcess 不认无扩展名的脚本。包里
/// 真正的 exe 路径写在 pack.json 的 selfCheck 里,直接从那儿取。
function realBin(n) {
  if (!isWin) return shim(n);
  if (n === 'node') return ENV.RUNTIME_NODE;
  // 去掉版本后缀再比：Windows 包里的解释器叫 python.exe，而这里问的是 python3。
  const stem = n.replace(/\d+$/, '');
  const hit = (pack.selfCheck ?? []).find((c) => path.basename(c.command).toLowerCase().startsWith(stem));
  if (!hit) throw new Error(`pack.json 的 selfCheck 里找不到 ${n},没法在 Windows 上定位真二进制`);
  return path.join(PACK, hit.command);
}

/// 在只有 pathPrepend 的 PATH 上按名字找 —— 模型在 skill 里就是这么调的。
function onPath(name) {
  for (const dir of ENV.PATH.split(sep)) {
    for (const cand of isWin ? [name, `${name}.exe`, `${name}.cmd`] : [name]) {
      const p = path.join(dir, cand);
      if (fs.existsSync(p)) return p;
    }
  }
  return null;
}

console.log(`验证 ${pack.name} ${pack.version} (${pack.platform})`);
console.log(`  包目录: ${PACK}`);
console.log(`  临时区: ${WORK}`);
console.log(`  PATH:   ${ENV.PATH}`);

// —— 1. 自足性 ————————————————————————————————————————————
section('1. 依赖自足性（模拟无开发环境的机器）');

for (const name of ['python3', 'node', 'soffice', 'pdftoppm']) {
  chk(fs.existsSync(shim(name)), `RUNTIME_BIN_DIR/${name} 存在`);
}
if (isWin) {
  // Windows 上 Python 侧不走 shim（CreateProcess 不认无扩展名的脚本），
  // 靠这份清单找真 exe。缺了它 render_docx.py 和 render_slides.py 全瘸。
  const f = path.join(BIN_DIR, 'native-executables.json');
  let names = [];
  try { names = Object.keys(JSON.parse(fs.readFileSync(f, 'utf8'))); } catch { /* 下面报 */ }
  const need = ['soffice', 'pdftoppm', 'pdfinfo'];
  chk(need.every((n) => names.includes(n)), '原生 exe 清单齐全（Windows 侧 Python 靠它定位）',
    `${f} -> ${names.join(', ') || '读取失败'}`);
}
{
  const r = tryRun(realBin('python3'), ['-V']);
  chk(r.ok, `python3 可执行 (${r.out.trim()})`, r.ok ? null : r.out);
}
{
  const r = tryRun(realBin('node'), ['-v']);
  chk(r.ok, `node 可执行 (${r.out.trim()})`, r.ok ? null : r.out);
}
// pdf skill 的正文里直接写着 `pdftoppm -png ...`，所以这两个必须按名字可达。
// soffice 不要求在 PATH 上：Windows 上它的目录里带着自己的 python.exe，
// 那个目录不能进 PATH，改由 render_docx.py 从 RUNTIME_BIN_DIR 解析（见下一节）。
for (const name of ['pdftoppm', 'pdfinfo']) {
  chk(onPath(name) !== null, `${name} 在 PATH 上`);
}
// 反过来，包里的 python3 / node 不该出现在 PATH 上 —— 出现了就会盖掉用户
// 项目的虚拟环境。系统自带的 /usr/bin/python3 排在后面是正常的。
for (const name of ['python3', 'node']) {
  const found = onPath(name);
  chk(found === null || !found.startsWith(PACK),
    `包里的 ${name} 不在 PATH 上（避免盖掉用户的解释器）`, found);
}
{
  const mods = 'docx,pptx,openpyxl,pdfplumber,pypdf,PIL,lxml,numpy,pdf2image,reportlab';
  const r = tryRun(realBin('python3'), ['-c', `import ${mods.split(',').join(',')}`]);
  chk(r.ok, `文档库可导入 (${mods})`, r.ok ? null : r.out);
}
{
  // 这三个是 skill 自带脚本直接读的,缺一个就整条链断。
  const missing = ['RUNTIME_NODE', 'RUNTIME_NODE_MODULES', 'RUNTIME_BIN_DIR']
    .filter((k) => !ENV[k] || !fs.existsSync(ENV[k]));
  chk(missing.length === 0, 'RUNTIME_* 环境变量指向真实路径', missing.join(', '));
}

// —— 2. docx 渲染闭环 + CJK ————————————————————————————
section('2. docx 渲染闭环 + CJK');

const docx = path.join(WORK, 'test.docx');
{
  const script = `
from docx import Document
d = Document()
d.add_heading('Riot 文档能力包验证', 0)
d.add_paragraph('中文正文：这份文档验证 python-docx 生成、LibreOffice 渲染、CJK 字体三条链路。')
d.add_paragraph('Mixed 中英 content 123 —— punctuation：，。！？')
t = d.add_table(rows=4, cols=3); t.style = 'Table Grid'
for i, row in enumerate([('季度','营收','同比'),('Q1','120','+8%'),('Q2','150','+25%'),('Q3','180','+20%')]):
    for j, v in enumerate(row):
        t.rows[i].cells[j].text = v
d.save(${JSON.stringify(docx)})
`;
  const r = tryRun(realBin('python3'), ['-c', script]);
  chk(r.ok, 'python-docx 生成 docx', r.ok ? null : r.out);
}
{
  const renderer = path.join(PACK, 'skills/documents/render_docx.py');
  const r = tryRun(realBin('python3'), [renderer, docx, '--output_dir', path.join(WORK, 'render'), '--emit_pdf']);
  chk(r.ok, 'render_docx.py 渲染（包自带的官方渲染脚本）', r.ok ? null : r.out);
}
{
  const dir = path.join(WORK, 'render');
  const pngs = fs.existsSync(dir) ? fs.readdirSync(dir).filter((f) => f.endsWith('.png')) : [];
  chk(pngs.length > 0, `产出页面 PNG（${pngs.length} 张）`);
}
{
  // 中文是否真的画进去了,只看有没有 PNG 是看不出来的 —— 缺字体时页面照样生成,
  // 只是中文位置一片空白。从 PDF 抽文本才能证伪。
  const script = `
import glob, sys, pdfplumber
pdfs = glob.glob(${JSON.stringify(path.join(WORK, 'render', '*.pdf'))})
if not pdfs:
    print('没有 PDF 产出'); sys.exit(1)
text = ''
with pdfplumber.open(pdfs[0]) as pdf:
    for p in pdf.pages:
        text += p.extract_text() or ''
text = text.replace(' ', '')
# 逐词校验:pdfplumber 的阅读顺序可能把标题拆行,不能按整句拼接断言
need = ['文档能力包验证', '中文正文', '季度', '营收', '同比', '，。！？']
missing = [n for n in need if n not in text]
if missing:
    print('缺失:', missing); print('实际抽到:', repr(text[:200])); sys.exit(1)
print('命中全部 CJK 片段')
`;
  const r = tryRun(realBin('python3'), ['-c', script]);
  chk(r.ok, 'CJK 正确渲染（标题/正文/表头/标点均落入 PDF）', r.ok ? null : r.out);
}
{
  // Windows 上跑的就是这个配置：PATH 里一个包内目录都没有，soffice 和 poppler
  // 全靠 RUNTIME_BIN_DIR 解析。在 macOS 上先把它测通，等于提前验掉 Windows 包
  // 最容易翻车的一环 —— 那边没法用 PATH 兜底，LibreOffice 目录进不了 PATH。
  const r = tryRun(realBin('python3'),
    [path.join(PACK, 'skills/documents/render_docx.py'), docx,
      '--output_dir', path.join(WORK, 'render-nopath')],
    { env: { ...ENV, PATH: BASE_PATH.join(sep) } });
  const dir = path.join(WORK, 'render-nopath');
  const pngs = fs.existsSync(dir) ? fs.readdirSync(dir).filter((f) => f.endsWith('.png')) : [];
  chk(r.ok && pngs.length > 0, 'PATH 上没有包内目录时仍能渲染（Windows 走的就是这条）',
    r.ok ? null : r.out);
}

// —— 3. xlsx 引擎 ————————————————————————————————————————
section('3. xlsx 引擎（artifact-tool MCP）');

const xlsx = path.join(WORK, 'book.xlsx');
const mcpResult = await probeArtifactTool();
chk(mcpResult.ok, 'artifact-tool MCP 建表并求值公式', mcpResult.ok ? null : mcpResult.detail);
chk(fs.existsSync(xlsx), 'xlsx 落盘');
if (fs.existsSync(xlsx)) {
  // 反向校验:公式必须带缓存值。openpyxl 自己写不出缓存值,正好用来证明
  // 这确实是 artifact-tool 求值的结果,而不是一个空壳公式。
  const script = `
import openpyxl, sys
p = ${JSON.stringify(xlsx)}
wf = openpyxl.load_workbook(p); wv = openpyxl.load_workbook(p, data_only=True)
f = wf['营收']['B5'].value; v = wv['营收']['B5'].value
print(f'B5 公式={f!r} 缓存值={v!r} 工作表={wf.sheetnames}')
sys.exit(0 if f == '=SUM(B2:B4)' and v == 450 else 1)
`;
  const r = tryRun(realBin('python3'), ['-c', script]);
  chk(r.ok, '公式 =SUM(B2:B4) 已求值为 450', r.out.trim());
}

// —— 4. skill 改写结果 ————————————————————————————————
section('4. skill 已剥离 Codex 宿主调用');

const FORBIDDEN = ['mark_artifact_operation_started', 'list_artifact_templates',
  'choose_artifact_template', 'codex-file-citation', 'load_workspace_dependencies',
  'mcp__codex_apps', '`update_plan`', '`request_user_input`'];
for (const name of pack.skills ?? []) {
  const file = path.join(PACK, 'skills', name, 'SKILL.md');
  if (!fs.existsSync(file)) { chk(false, `skills/${name}/SKILL.md 存在`); continue; }
  const text = fs.readFileSync(file, 'utf8');
  const hits = FORBIDDEN.filter((f) => text.includes(f));
  chk(hits.length === 0, `${name}: 无 Codex 宿主残留`, hits.join(', '));

  const m = /^---\n([\s\S]*?)\n---/.exec(text);
  const desc = /description:\s*(?:"((?:[^"\\]|\\.)*)"|(.*))/.exec(m?.[1] ?? '');
  const len = (desc?.[1] ?? desc?.[2] ?? '').length;
  chk(len > 0 && len <= 250, `${name}: description ${len} 字符（上限 250）`);
}

// 文档里照抄过来的 `python scripts/foo.py` 会命中用户自己的解释器，
// 那里面没有 python-docx。附属任务文档也一起查 —— 渐进披露会让模型读到它们。
{
  const offenders = [];
  for (const file of walkMd(path.join(PACK, 'skills'))) {
    const lines = fs.readFileSync(file, 'utf8').split('\n');
    lines.forEach((line, i) => {
      if (/^\s*(python3?|node)\s+[\w$./"'-]/.test(line)) {
        offenders.push(`${path.relative(PACK, file)}:${i + 1}  ${line.trim().slice(0, 60)}`);
      }
    });
  }
  chk(offenders.length === 0, '技能文档里没有裸的 python / node 调用',
    offenders.slice(0, 8).join('\n') + (offenders.length > 8 ? `\n… 共 ${offenders.length} 处` : ''));
}

function* walkMd(dir) {
  if (!fs.existsSync(dir)) return;
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) yield* walkMd(p);
    else if (e.name.endsWith('.md')) yield p;
  }
}

// —— 结果 ————————————————————————————————————————————————
section(`结果: ${pass} 通过, ${fail} 失败`);
if (fail > 0) {
  console.log('失败项:');
  for (const f of failures) console.log(`  - ${f}`);
  console.log(`\n产物保留在 ${WORK} 供排查。`);
  process.exit(1);
}
fs.rmSync(WORK, { recursive: true, force: true });
console.log('能力包自足,可发布。');

// —— artifact-tool MCP 探测 ————————————————————————————
// 以一个普通 MCP stdio 客户端的身份驱动它。Riot 里这一层由 MCP hub 承担,
// 这里只验证服务端本身不依赖 Codex 宿主。
async function probeArtifactTool() {
  const spec = (pack.mcpServers ?? [])[0];
  if (!spec) return { ok: false, detail: 'pack.json 里没有 mcpServers' };

  const child = spawn(path.join(PACK, spec.command), spec.args.map((a) => path.join(PACK, a)), {
    stdio: ['pipe', 'pipe', 'pipe'],
    env: { ...ENV, PLUGIN_DATA: path.join(WORK, 'sessions') },
  });
  let stderr = '';
  child.stderr.on('data', (d) => { stderr += d; });

  const pending = new Map();
  let nextId = 1;
  readline.createInterface({ input: child.stdout, crlfDelay: Infinity }).on('line', (line) => {
    let msg;
    try { msg = JSON.parse(line); } catch { return; }
    pending.get(msg.id)?.(msg);
    pending.delete(msg.id);
  });
  const rpc = (method, params) => new Promise((resolve, reject) => {
    const id = nextId++;
    const timer = setTimeout(() => reject(new Error(`${method} 超时`)), 60_000);
    pending.set(id, (m) => { clearTimeout(timer); resolve(m); });
    child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', id, method, params })}\n`);
  });

  try {
    const init = await rpc('initialize', {
      protocolVersion: '2025-11-25', capabilities: {},
      clientInfo: { name: 'riot-pack-verify', version: '0' },
    });
    if (!init.result?.serverInfo) return { ok: false, detail: `initialize 失败: ${JSON.stringify(init)}\n${stderr}` };

    const res = await rpc('tools/call', {
      name: 'artifact_session_run',
      arguments: {
        artifactType: 'spreadsheet',
        // Riot 不提供 Codex 的会话 id,target 必须显式给绝对路径。
        target: xlsx,
        create: true,
        autoSave: true,
        summary: 'riot pack verification',
        code: `
          const sheet = workbook.worksheets.add("营收");
          sheet.getRange("A1:C1").values = [["季度", "营收", "同比"]];
          sheet.getRange("A2:C4").values = [["Q1",120,"+8%"],["Q2",150,"+25%"],["Q3",180,"+20%"]];
          sheet.getRange("A5").values = [["合计"]];
          sheet.getRange("B5").formulas = [["=SUM(B2:B4)"]];
          sheet.getRange("A1:C1").format.font.bold = true;
          return { computedB5: sheet.getRange("B5").values };
        `,
      },
    });
    const sc = res.result?.structuredContent;
    if (sc?.ok !== true) return { ok: false, detail: `${JSON.stringify(sc ?? res)}\n${stderr}` };
    if (sc.result?.computedB5?.[0]?.[0] !== 450) {
      return { ok: false, detail: `公式未求值: ${JSON.stringify(sc.result)}` };
    }
    return { ok: true };
  } catch (e) {
    return { ok: false, detail: `${e.message}\n${stderr}` };
  } finally {
    child.kill('SIGTERM');
  }
}
