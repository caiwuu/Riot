---
name: protocol-change
description: 改了 crates/riot-protocol 里的类型之后用。重新生成绑定，并检查 serde tag 撞名这类 Rust 类型层面看不出来的问题。
---

# 改协议类型之后

`riot-protocol` 是宿主、内核、前端共享的契约，也是依赖图的叶子。Rust 那
两侧的一致性由**编译器**保证；TypeScript 那侧不行，它靠生成。

## 1. 重新生成

```bash
pnpm gen
```

产物 `schemas/protocol.json` 和 `src/bridge/generated.ts` **都进版本库**，
不要手改。CI 跑 `pnpm gen && git diff --exit-code`，忘了生成直接红。

## 2. tag 撞名检查

`pnpm gen` 里带了这个检查，但要看懂它在说什么：

serde 的 internally-tagged 表示下，newtype variant 会把内层字段**摊平**。
内外层 tag 同名时，产物是一份重复 key 的 JSON —— 反序列化直接失败，而这
在 Rust 类型层面**完全看不出来**。

真实案例：`StreamDelta` 和 `AgentEvent` 的 tag 撞名，后果是前端一个 token
都收不到。抓到它的是 roundtrip 测试，不是 review。

## 3. 加字段要能读旧数据

`[约束]` 缺字段不能让整份配置/transcript 解析失败。那表现为用户升级之后
"我配的东西全没了"。

新字段一律带 `#[serde(default)]`，或者给一个 `default_xxx()` 函数。加完之后
补一个"老格式也能读"的测试 —— `config.rs` 里有现成的写法可以照抄
（搜 `老配置缺`）。

## 4. 枚举加 variant 要顺着编译器走一遍

穷举 match 比通配安全：加了新 variant，所有该重审的地方会被编译器点名。
如果某处确实想忽略新情况，写明理由，不要图省事改成 `_ => {}`。

## 5. 验证

跑 `verify` 技能的全部五层。协议改动会同时波及 Rust 两侧和前端，
只跑一边不够。
