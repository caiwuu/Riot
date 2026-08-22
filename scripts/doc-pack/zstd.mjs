// tar → tar.zst。
//
// 单独拆出来是为了 Windows:那边没有 zstd 命令行工具,而 PowerShell 的管道是
// 对象流,拿它传二进制会被编码转换弄坏。Node 自带 zstd(22.15+),用它最省事。
//
// 用法: node scripts/doc-pack/zstd.mjs <输入> <输出> [压缩等级]

import fs from 'node:fs';
import { pipeline } from 'node:stream/promises';
import zlib from 'node:zlib';

const [input, output, level = '19'] = process.argv.slice(2);
if (!input || !output) {
  console.error('用法: node scripts/doc-pack/zstd.mjs <输入> <输出> [压缩等级]');
  process.exit(2);
}
if (typeof zlib.createZstdCompress !== 'function') {
  console.error(`当前 Node (${process.version}) 不支持 zstd,需要 22.15 以上。`);
  process.exit(1);
}

await pipeline(
  fs.createReadStream(input),
  zlib.createZstdCompress({
    params: { [zlib.constants.ZSTD_c_compressionLevel]: Number(level) },
  }),
  fs.createWriteStream(output),
);
