// 把各平台构建机产出的 packs.json 合并成一份发布用的清单。
//
// 为什么需要这一步:包必须在对应平台的机器上造(原生二进制没法交叉编译),而每台
// 机器只会写出自己那个平台的清单 —— Mac 那份只有 `darwin-arm64` 一个键,Windows
// 那份只有 `win-x64`。直接把任意一份发上去,另一个平台的用户看到的是"这个包没有
// 适配当前系统的版本"。
//
// 用法:
//   node scripts/doc-pack/merge-manifest.mjs out.json mac/packs.json win/packs.json ...
//
// 合并只做并集,不做取舍 —— 同一个包在两份清单里说法不一致时**直接报错**而不是
// 挑一个。那种不一致意味着两台机器构建的不是同一批东西,挑哪个都会让一部分用户
// 装到配不上的组合,而症状要等到运行时才出现。

import fs from 'node:fs';
import path from 'node:path';

const [out, ...inputs] = process.argv.slice(2);
if (!out || inputs.length === 0) {
  console.error('用法: node merge-manifest.mjs <输出.json> <输入1.json> [输入2.json ...]');
  process.exit(2);
}

const merged = { schemaVersion: 1, packs: {} };
const origin = new Map(); // "<pack>/<platform>" -> 来自哪个文件，报错时指得出人

for (const file of inputs) {
  if (!fs.existsSync(file)) fail(`找不到 ${file}`);
  const m = JSON.parse(fs.readFileSync(file, 'utf8'));
  if (m.schemaVersion !== 1) fail(`${file} 的 schemaVersion 是 ${m.schemaVersion}，只认 1`);

  for (const [id, pack] of Object.entries(m.packs ?? {})) {
    const existing = merged.packs[id];
    if (!existing) {
      merged.packs[id] = structuredClone(pack);
      for (const p of Object.keys(pack.platforms ?? {})) origin.set(`${id}/${p}`, file);
      continue;
    }

    // 版本必须一致。对不上意味着两台机器构建的不是同一批,而清单只有一个
    // version 字段 —— 挑哪个都会让另一个平台的用户装到版本号名不副实的包。
    if (existing.version !== pack.version) {
      fail(
        `「${id}」的版本在两份清单里不一致：\n` +
          `  ${origin.get(`${id}/${Object.keys(existing.platforms)[0]}`)}: ${existing.version}\n` +
          `  ${file}: ${pack.version}\n` +
          `两台构建机跑的不是同一批。重新在各平台上跑一遍构建脚本。`,
      );
    }

    for (const [plat, asset] of Object.entries(pack.platforms ?? {})) {
      const prev = origin.get(`${id}/${plat}`);
      if (prev) fail(`「${id}」的 ${plat} 在 ${prev} 和 ${file} 里都有，重复了。`);
      existing.platforms[plat] = asset;
      origin.set(`${id}/${plat}`, file);
    }
  }
}

fs.mkdirSync(path.dirname(path.resolve(out)), { recursive: true });
fs.writeFileSync(out, `${JSON.stringify(merged, null, 2)}\n`);

console.log(`合并 ${inputs.length} 份 → ${out}`);
for (const [id, pack] of Object.entries(merged.packs)) {
  const plats = Object.keys(pack.platforms ?? {}).sort();
  console.log(`  ${id} ${pack.version}  ${plats.join(', ')}`);
}

function fail(m) {
  console.error(`\n错误: ${m}\n`);
  process.exit(1);
}
