// 把能力包仓库里各平台的产物发布出去。
//
// 仓库长这样:
//
//   riot-pkg/
//     packs.json                     ← 合并后的发布清单，Riot 运行时拉的就是它
//     doc-runtime/
//       darwin-arm64/
//         packs.json                 ← 这台 Mac 构建时写的，只描述 darwin-arm64
//         doc-runtime-0.1.0-darwin-arm64.tar.zst
//       win-x64/
//         packs.json
//         doc-runtime-0.1.0-win-x64.tar.zst
//
// 为什么包体不进 git:GitHub 拒收超过 100MB 的单个文件，而一个包两百多 MB。
// Git LFS 的免费额度是 1GB 存储 + 1GB/月流量，两个平台的包就占掉一半存储，
// 用户下几次流量就见底了。所以 tar.zst 走 Releases（单文件上限 2GB、不计入
// 仓库体积、也不拖慢 clone），仓库里只留几百字节的清单。
//
// 用法:
//   node scripts/doc-pack/publish.mjs             合并 + 校验 + 上传 + 推清单（需要 gh）
//   node scripts/doc-pack/publish.mjs --dry-run   只合并和校验，列出该传哪些文件
//   node scripts/doc-pack/publish.mjs --no-upload 包体已经手工传好了，只推清单
//   node scripts/doc-pack/publish.mjs --repo <目录>
//
// 手工上传的话分两步:先 --dry-run 看该传什么、传到哪个 tag，去 GitHub 网页上把
// 那几个 .tar.zst 拖进对应的 release，然后 --no-upload 推清单。顺序不能反 ——
// Riot 一读到新清单就会拿里面的 url 去下载。

import { spawnSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(HERE, '..', '..');

const argv = process.argv.slice(2);
const dryRun = argv.includes('--dry-run');
const noUpload = argv.includes('--no-upload');
const repo = path.resolve(
  argOf('--repo', process.env.RIOT_PKG_REPO ?? path.join(ROOT, '..', 'riot-pkg')),
);

if (!fs.existsSync(path.join(repo, '.git'))) {
  fail(`${repo} 不是一个 git 仓库。\n先 git clone https://github.com/caiwuu/riot-pkg.git 到那里，或用 --repo 指别处。`);
}

// 1. 收集各平台清单 ————————————————————————————————————————
// 布局是 <包名>/<平台>/packs.json。目录由构建脚本创建，这里只认已经存在的 ——
// 少了哪个平台是构建的事，发布环节报"缺 win-x64"没有意义，那台机器可能压根
// 还没轮到。
const platformManifests = subdirs(repo)
  .flatMap((pack) => subdirs(path.join(repo, pack)).map((plat) => path.join(repo, pack, plat, 'packs.json')))
  .filter((p) => fs.existsSync(p))
  .sort();

if (platformManifests.length === 0) {
  fail(`${repo} 下面没有任何平台清单（找的是 <包名>/<平台>/packs.json）。\n先跑 scripts/build-doc-pack.mjs（或 .ps1）。`);
}

step('合并平台清单');
const rootManifest = path.join(repo, 'packs.json');
// 子进程失败时它自己已经把原因打全了，这里再抛一遍只会盖上一屏无关的栈。
const mergeResult = spawnSync(
  process.execPath,
  [path.join(HERE, 'merge-manifest.mjs'), rootManifest, ...platformManifests],
  { stdio: 'inherit' },
);
if (mergeResult.status !== 0) process.exit(mergeResult.status ?? 1);

// 2. 校验 ————————————————————————————————————————————————————
// 分两种情况，因为发布必然是从**某一台**机器上做的，而它手里只有自己造的那个
// 平台的包 —— 别的平台的 .tar.zst 不进 git，clone 不过来。
//
//   本地有  → 比 sha256。清单里的哈希是构建时算的，此刻磁盘上的文件要是对不上，
//             说明包在构建之后被动过，而症状要到用户装包校验失败时才出现。
//   本地没有 → 那它必须**已经在 release 里**。查一下地址通不通、字节数对不对。
//             漏掉这步的话，一份清单可以理直气壮地指着一个根本不存在的资产。
step('校验');
const merged = JSON.parse(fs.readFileSync(rootManifest, 'utf8'));
const uploads = new Map(); // tag -> [文件绝对路径]，只装本地真有的
const tags = new Set(); // 清单涉及的全部 tag，本地有没有产物都算

for (const [id, pack] of Object.entries(merged.packs)) {
  for (const [platform, asset] of Object.entries(pack.platforms ?? {})) {
    const { tag, file } = parseAssetUrl(asset.url, `${id}/${platform}`);
    const local = path.join(repo, id, platform, file);
    tags.add(tag);

    if (!fs.existsSync(local)) {
      const head = await headAsset(asset.url);
      if (!head) {
        fail(
          `${id} 的 ${platform} 包本地没有，release 里也拉不到:\n  ${asset.url}\n` +
            `要么是那台机器的产物没同步过来，要么是还没传上去。`,
        );
      }
      if (head.size !== null && head.size !== asset.size) {
        fail(
          `${id} 的 ${platform} 在 release 里的大小和清单对不上:\n` +
            `  清单: ${asset.size} 字节\n  release: ${head.size} 字节\n` +
            `多半是传了个旧版本上去，重传一次。`,
        );
      }
      log(
        head.size === null
          ? `  ${id} ${platform}  已在 release 里（对方没给长度，大小没比成）`
          : `  ${id} ${platform}  ${mb(head.size)}  已在 release 里`,
      );
      continue;
    }

    const size = fs.statSync(local).size;
    const sha256 = sha256File(local);
    if (size !== asset.size || sha256 !== asset.sha256) {
      fail(
        `${path.relative(repo, local)} 和清单对不上:\n` +
          `  清单: ${asset.size} 字节, sha256 ${asset.sha256}\n` +
          `  实际: ${size} 字节, sha256 ${sha256}\n` +
          `重跑一遍那个平台的构建脚本。`,
      );
    }
    log(`  ${id} ${platform}  ${mb(size)}  本地校验通过`);
    if (!uploads.has(tag)) uploads.set(tag, []);
    uploads.get(tag).push(local);
  }
}

if (dryRun) {
  log('\n--dry-run，到此为止。');
  if (uploads.size === 0) {
    log('\n本地没有待上传的产物，清单里的资产都已经在 release 里了。');
    log('直接跑: node scripts/doc-pack/publish.mjs --no-upload');
  } else {
    log(`\n把这些文件传到 https://github.com/${slug()}/releases 的对应 release:`);
    for (const [tag, files] of uploads) {
      log(`  tag ${tag}`);
      for (const f of files) log(`    ${f}`);
    }
    log('\n传完之后跑: node scripts/doc-pack/publish.mjs --no-upload');
  }
  process.exit(0);
}

// 3. 上传 ——————————————————————————————————————————————————
if (noUpload) {
  step('跳过上传');
  log('  --no-upload，当作包体已经在 release 里了。');
} else {
  step('上传 release 资产');
  if (!hasGh()) {
    fail(
      'gh 没装或没登录，没法自动上传。\n' +
        '装一个:  brew install gh && gh auth login\n' +
        '或者手工传，然后只推清单:\n' +
        '  node scripts/doc-pack/publish.mjs --dry-run    # 看该传哪些文件\n' +
        '  node scripts/doc-pack/publish.mjs --no-upload  # 传完之后推清单',
    );
  }
  for (const [tag, files] of uploads) {
    const exists = gh(['release', 'view', tag], { allowFail: true }).status === 0;
    if (!exists) {
      log(`  新建 release ${tag}`);
      gh(['release', 'create', tag, '--repo', slug(), '--title', tag, '--notes', `Riot 能力包 ${tag}`]);
    }
    // --clobber:重发同一版是常事（比如某个平台的包重造了），没有它第二次会撞名失败。
    log(`  上传 ${files.length} 个资产 → ${tag}`);
    gh(['release', 'upload', tag, ...files, '--repo', slug(), '--clobber']);
  }
}

// 4. 推清单 ————————————————————————————————————————————————
// 清单最后推:Riot 一读到新清单就会拿里面的 url 去下载，资产得先在那儿。
step('提交 packs.json');
// 平台清单也要看:在 Windows 上发布时，根清单可能因为已经合并过而没变化，
// 变的是新增的那份 win-x64/packs.json。只看根清单会把它漏在工作区里。
const tracked = ['packs.json', ...platformManifests.map((p) => path.relative(repo, p))];
const status = git(['status', '--porcelain', '--', ...tracked]).stdout.trim();
if (!status) {
  log('  清单没变化，跳过。');
} else {
  git(['add', ...tracked]);
  git(['commit', '-m', `发布 ${[...tags].join(', ')}`]);
  git(['push']);
  log('  已推送。');
}

log('\n完成。Riot 下次启动就能看到新版本。');

// —— 辅助 ——————————————————————————————————————————————————

// 从下载地址反推 tag 和文件名，让构建脚本保持 tag 的唯一定义处。
function parseAssetUrl(url, who) {
  const m = /\/releases\/download\/([^/]+)\/([^/]+)$/.exec(url ?? '');
  if (!m) fail(`${who} 的 url 不像 release 资产地址: ${url}`);
  return { tag: m[1], file: m[2] };
}

// null = 拉不到；{ size } = 在（size 可能为 null，对方没给 content-length）。
async function headAsset(url) {
  try {
    const res = await fetch(url, { method: 'HEAD', redirect: 'follow' });
    if (!res.ok) return null;
    const len = res.headers.get('content-length');
    return { size: len === null ? null : Number(len) };
  } catch {
    return null;
  }
}

function subdirs(dir) {
  return fs
    .readdirSync(dir, { withFileTypes: true })
    .filter((e) => e.isDirectory() && !e.name.startsWith('.'))
    .map((e) => e.name);
}

function slug() {
  const url = git(['remote', 'get-url', 'origin']).stdout.trim();
  const m = /github\.com[/:]([^/]+\/[^/.]+)/.exec(url);
  if (!m) fail(`认不出 origin 指向哪个 GitHub 仓库: ${url}`);
  return m[1];
}

function hasGh() {
  return spawnSync('gh', ['auth', 'status'], { stdio: 'ignore' }).status === 0;
}

function gh(args, { allowFail = false } = {}) {
  const r = spawnSync('gh', args, { cwd: repo, stdio: allowFail ? 'ignore' : 'inherit' });
  if (!allowFail && r.status !== 0) fail(`gh ${args.slice(0, 2).join(' ')} 失败`);
  return r;
}

function git(args) {
  const r = spawnSync('git', args, { cwd: repo, encoding: 'utf8' });
  if (r.status !== 0) fail(`git ${args.join(' ')} 失败:\n${r.stderr}`);
  return r;
}

function argOf(name, fallback) {
  const i = argv.indexOf(name);
  return i >= 0 && argv[i + 1] ? argv[i + 1] : fallback;
}

function sha256File(file) {
  const h = crypto.createHash('sha256');
  const fd = fs.openSync(file, 'r');
  const buf = Buffer.alloc(1 << 20);
  let n;
  while ((n = fs.readSync(fd, buf, 0, buf.length, null)) > 0) h.update(buf.subarray(0, n));
  fs.closeSync(fd);
  return h.digest('hex');
}

function mb(bytes) { return `${(bytes / 1024 / 1024).toFixed(0)}MB`; }
function log(m) { console.log(m); }
function step(m) { console.log(`\n[${m}]`); }
function fail(m) { console.error(`\n错误: ${m}\n`); process.exit(1); }
