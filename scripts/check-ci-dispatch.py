#!/usr/bin/env python3
"""校验 CI 手动触发的 job 选择器自洽。

`workflow_dispatch` 的 `only` 是个 choice 列表，而每个重活 job 用
`inputs.only == '<自己的名字>'` 来认领它。三处名字（job key、choice 选项、
if 里的字符串）必须两两对上 —— 对不上的表现是**静默的**：选了某个 job，
一个都没触发，workflow 照样绿。那比红更糟，因为它看起来像"验过了"。

用法：python3 scripts/check-ci-dispatch.py
"""

import pathlib
import re
import sys

CI = pathlib.Path(__file__).resolve().parent.parent / ".github" / "workflows" / "ci.yml"


def main() -> int:
    text = CI.read_text(encoding="utf-8")

    jobs = set(re.findall(r"^  ([a-z0-9][a-z0-9-]*):$", text, re.M))
    if not jobs:
        print("解析不出任何 job —— 这个脚本的假设和 ci.yml 的结构漂移了", file=sys.stderr)
        return 1

    # choice 列表：`options:` 之后连续的 `- xxx` 行。
    block = re.search(r"^        options:\n((?:^          - \S+\n)+)", text, re.M)
    if not block:
        print("找不到 workflow_dispatch 的 options 列表", file=sys.stderr)
        return 1
    options = set(re.findall(r"- (\S+)", block.group(1))) - {"all"}

    claimed = set(re.findall(r"inputs\.only == '([^']+)'", text)) - {"all"}

    problems = []
    for name in sorted(options - jobs):
        problems.append(f"选项 {name!r} 不是任何 job 的名字 —— 选它会一个 job 都不跑")
    for name in sorted(claimed - jobs):
        problems.append(f"if 里认领的 {name!r} 不是任何 job 的名字 —— 这个 job 永远不会被手动选中")
    for name in sorted(claimed - options):
        problems.append(f"job {name!r} 认领了自己，但没进 options 列表 —— 界面上选不到它")
    for name in sorted(options - claimed):
        problems.append(f"选项 {name!r} 没有任何 job 认领 —— 选它会一个 job 都不跑")

    # 认领的名字必须是它自己。写成别人的名字同样静默：选 A 跑起 B。
    for job, cond in re.findall(r"^  ([a-z0-9][a-z0-9-]*):\n    if: ([^\n]+)$", text, re.M):
        for name in re.findall(r"inputs\.only == '([^']+)'", cond):
            if name != "all" and name != job:
                problems.append(f"job {job!r} 认领的是 {name!r} —— 选 {name!r} 会跑起 {job!r}")

    if problems:
        print("CI 的 job 选择器对不上：", file=sys.stderr)
        for p in problems:
            print(f"  - {p}", file=sys.stderr)
        return 1

    print(f"CI 选择器自洽：{len(options)} 个可单独触发的 job")
    return 0


if __name__ == "__main__":
    sys.exit(main())
