---
name: mutate
description: 动过权限决策链、Bash 分析、路径围栏、安全检查、文件工具或进程执行器之后用。跑变异测试，并正确解读存活的变异。
---

# 变异测试

## 什么时候必须跑

改动碰到这几处任何一个：

- `crates/riot-permissions/`（决策链、规则匹配、Bash AST 分析、安全检查）
- `src-tauri/src/fence.rs`（路径围栏）
- `crates/riot-tools/src/tools/{read,write,edit,bash}.rs`
- `crates/riot-runtime/` 的进程执行器

这些地方的 bug 不表现为崩溃或报错，表现为**一条防线静默失效**。普通测试
全绿说明不了什么 —— 要问的是"如果我把这行改坏，有测试会红吗"。

## 怎么跑

```bash
python3 scripts/mutate.py                  # 全部 53 个
python3 scripts/mutate.py permissions      # 只跑一层
python3 scripts/mutate.py --check-anchors  # 只校验锚点还在
```

重构之后先跑 `--check-anchors`。锚点静默失效时脚本会报"全部通过"，
因为它什么都没改成 —— **虚假的安全感比测试失败危险得多**。

## 怎么读结果

**全绿的那次没有产出信息。价值全在存活的那几个上。**

存活的变异有三种，处理方式完全不同：

1. **真缺口** —— 那一行没有任何测试守着。补一个断言在**正确的理由**上失败
   的测试。注意：只断言 `!is_ok` 往往不够。`Write` 那个真实案例里，变异后
   工具仍然拒绝，只是理由从"你还没读过"变成了"文件内容对不上" —— 这个区别
   对模型是实打实的（前者让它先读，后者让它以为有人在并发改文件，白跑一轮）。

2. **等价变异** —— 当前实现下两种写法行为完全一样。**不要**为了让脚本变绿
   去写一个只能杀死这一个变异的测试。该做的是把它揭示的那条隐含耦合写进
   `docs/ARCHITECTURE.md`（Bash 分析层就有一个：宽松遍历的正确性依赖于
   扫描阶段的完备性）。

3. **锚点失效** —— 变异根本没注入成功。修脚本里的锚点，别修测试。

## 收尾检查

变异脚本用 `try/finally` 恢复源码，但那**挡不住 SIGKILL**。跑完之后确认
工作区干净：

```bash
git status --short
ls .mutate-backup/ 2>/dev/null
```

历史上有一次被强杀的运行把"Unix 下不包进程组"留在了 `proc.rs` 里 ——
编译通过、绝大多数测试照样绿，唯一症状是 `cargo test --workspace` 从几秒
变成六十多秒。**能扛住强杀的清理必须落盘，进程内的 finally 不算数。**
