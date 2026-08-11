#!/usr/bin/env python3
"""生成占位 app 图标。无第三方依赖，直接写 PNG。

只是为了让构建能跑起来 —— 有真图标了就删掉这个脚本。
用法: python3 scripts/gen_icon.py && pnpm tauri icon src-tauri/icons/icon.png
"""

import math
import struct
import zlib
from pathlib import Path

SIZE = 1024
CORNER = 224  # macOS squircle 观感


def dist_to_segment(px, py, ax, ay, bx, by):
    dx, dy = bx - ax, by - ay
    if dx == 0 and dy == 0:
        return math.hypot(px - ax, py - ay)
    t = max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)))
    return math.hypot(px - (ax + t * dx), py - (ay + t * dy))


def rounded_rect_alpha(x, y):
    """圆角矩形的覆盖度，边缘做 1px 抗锯齿。"""
    cx = min(max(x, CORNER), SIZE - CORNER)
    cy = min(max(y, CORNER), SIZE - CORNER)
    d = math.hypot(x - cx, y - cy)
    return max(0.0, min(1.0, CORNER - d + 0.5))


STROKES = [
    # 提示符 ">"
    (300, 330, 540, 512, 54),
    (540, 512, 300, 694, 54),
    # 光标 "_"
    (600, 668, 760, 668, 54),
]


def main():
    rows = bytearray()
    for y in range(SIZE):
        rows.append(0)  # filter: none
        # 背景竖向渐变
        t = y / SIZE
        br = int(32 + 12 * (1 - t))
        bg = int(34 + 14 * (1 - t))
        bb = int(48 + 18 * (1 - t))
        for x in range(SIZE):
            a = rounded_rect_alpha(x, y)
            if a <= 0:
                rows.extend((0, 0, 0, 0))
                continue

            cov = 0.0
            for ax, ay, bx, by, w in STROKES:
                d = dist_to_segment(x, y, ax, ay, bx, by)
                cov = max(cov, max(0.0, min(1.0, w / 2 - d + 0.5)))

            if cov > 0:
                r = int(br + (232 - br) * cov)
                g = int(bg + (238 - bg) * cov)
                b = int(bb + (250 - bb) * cov)
            else:
                r, g, b = br, bg, bb
            rows.extend((r, g, b, int(a * 255)))

    def chunk(tag, data):
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(bytes(rows), 9))
        + chunk(b"IEND", b"")
    )

    out = Path(__file__).resolve().parent.parent / "src-tauri" / "icons" / "icon.png"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_bytes(png)
    print(f"wrote {out} ({len(png) / 1024:.0f} KB)")


if __name__ == "__main__":
    main()
