---
name: verify
description: 改完代码要验证时用。按这个仓库的分层防线依次跑，并说明每一层拦的是什么。别只跑 cargo build 就当验证过了。
---

# 验证一次改动

按顺序跑。**每一层拦的东西不一样，跳过任何一层都会漏掉只有那一层能抓到的问题。**

## 1. 编译（迭代时用 check，不用 build）

```bash
cargo check --workspace
```

`build` 比 `check` 慢得多，而这一步要的只是"类型对不对"。只有真要跑起来时才 build。

只动了一个 crate 就带上 `-p`：`cargo check -p riot-host --all-targets`。
`--all-targets` 不能省 —— 不带它测试代码不参与编译，改了公共结构体的字段
会在下一次跑测试时才炸出来。

## 2. 测试（必须在 debug 下跑）

```bash
cargo test --workspace
```

**`[约束]` 不能只跑 `--release`。** 不变量断言用 `debug_assert` 系列，release
下整个编译掉 —— 那一层防线在 release 里根本不存在，全绿等于什么都没测。

## 3. Clippy（警告即错误）

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

这里不只是风格检查。`clippy.toml` 里禁掉的东西是**架构约束**：内核里
直接调 `SystemTime::now()` / `std::fs` / `std::process` 会被拒绝编译，
因为黄金回放测试要靠注入的 trait 才能成立。被这类规则拦下时，正确的
反应是走注入的那条路，不是加 `#[allow]`。

## 4. 前端

```bash
pnpm typecheck
```

## 5. 改过 `crates/riot-protocol/` 的类型才需要

```bash
pnpm gen
```

CI 会跑 `pnpm gen && git diff --exit-code`：改了 Rust 协议类型却忘了重新
生成，直接红。生成物（`schemas/protocol.json`、`src/bridge/generated.ts`）
**进版本库**，不要手改。

具体注意事项见 `protocol-change` 技能。

## 报告结果时

- 哪几层跑了、哪几层因为改动范围不需要跑（说明理由）；
- 失败的话，先说是**哪一层**报的 —— 这直接决定问题的性质（clippy 拦的是
  架构违规，测试拦的是行为错误，两者的修法完全不同）；
- 不要把"编译过了"说成"验证过了"。这个仓库的 bug 基本都是"编译通过、
  类型正确、看起来合理"的那种。
