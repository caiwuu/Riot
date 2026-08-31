//! 从 Rust 类型生成 JSON Schema。
//!
//! 这是协议契约的唯一真源。生成物提交进版本库，CI 里跑
//! `git diff --exit-code` 确保没人手改。
//!
//! 用法：
//!   cargo run -p riot-protocol --bin gen_schema
//!
//! 见 docs/VERIFICATION.md §2

use riot_protocol::{
    AgentEvent, Message, PermissionAsk, PermissionResponse, RpcNotification, RpcRequest,
    RpcResponse,
};
use schemars::JsonSchema;
use std::path::Path;

/// 把所有顶层类型收进一个 root，让生成的 schema 共享同一份 `$defs`。
/// 这样下游的 TS 生成器能产出一个类型互相引用的完整文件。
#[derive(JsonSchema)]
#[allow(dead_code)]
struct ProtocolRoot {
    agent_event: AgentEvent,
    message: Message,
    rpc_request: RpcRequest,
    rpc_response: RpcResponse,
    rpc_notification: RpcNotification,
    permission_ask: PermissionAsk,
    permission_response: PermissionResponse,
    // ProviderEvent 不进前端，但要过 tag 撞名检查 —— 黄金用例是手写 JSON，
    // 撞名的话用例会以「反序列化失败」的形式报错，很难联想到根因。
    provider_event: riot_protocol::provider::ProviderEvent,
    // 定时任务：列表视图、错过提示、宿主 emit 的运行事件、编辑补丁。
    scheduled_task: riot_protocol::ScheduledTask,
    missed_run: riot_protocol::MissedRun,
    schedule_run: riot_protocol::ScheduleRun,
    schedule_patch: riot_protocol::SchedulePatch,
}

// 豁免理由：这是构建期的代码生成器，不是内核代码。它的产物本身就是
// 磁盘文件，没有注入 FileSystem 的意义，也不参与黄金回放。
#[allow(clippy::disallowed_methods)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = workspace_root();
    let out_dir = root.join("schemas");
    std::fs::create_dir_all(&out_dir)?;

    let schema = schemars::schema_for!(ProtocolRoot);
    let mut value = serde_json::to_value(&schema)?;

    let clashes = find_tag_clashes(&value);
    if !clashes.is_empty() {
        eprintln!("协议里有摊平后撞名的 tag：");
        for c in &clashes {
            eprintln!("  - {c}");
        }
        eprintln!(
            "\n这类字段序列化出来是重复 key，反序列化会报 duplicate field。\n\
             修法：给内层类型换一个 tag 名（约定用 `kind`）。"
        );
        return Err("tag 撞名".into());
    }

    title_tagged_variants(&mut value);
    hoist_ref_siblings(&mut value);

    let mut json = serde_json::to_string_pretty(&value)?;
    json.push('\n');

    let path = out_dir.join("protocol.json");
    std::fs::write(&path, &json)?;

    println!("wrote {}", path.display());
    println!("下一步：pnpm gen:types  （由 schemas/protocol.json 生成 src/bridge/generated.ts）");
    Ok(())
}

/// 找出 internally-tagged newtype variant 里摊平后撞名的 tag。
///
/// 起因是一个真实 bug：`AgentEvent::Delta(StreamDelta)` 内外层都用
/// `tag = "type"`，产物是 `{"type":"delta","type":"text",...}` —— 重复 key，
/// 反序列化直接失败，前端一个 token 都收不到。
///
/// 这种错在 Rust 类型层面完全看不出来，`cargo check` 也不会报。放在这里检查是
/// 因为 schema 恰好把摊平结构显式表达成了「`$ref` 与 `properties` 并存」，
/// 一眼就能认出来。
fn find_tag_clashes(schema: &serde_json::Value) -> Vec<String> {
    let defs = schema.get("$defs").and_then(|d| d.as_object());
    let mut out = Vec::new();
    walk(schema, defs, &mut out);
    out.sort();
    out.dedup();
    return out;

    fn walk(
        node: &serde_json::Value,
        defs: Option<&serde_json::Map<String, serde_json::Value>>,
        out: &mut Vec<String>,
    ) {
        if let Some(obj) = node.as_object() {
            // 同时有 $ref 和 properties，说明内层字段被摊平到了这一层。
            if let (Some(r), Some(props)) = (
                obj.get("$ref").and_then(|r| r.as_str()),
                obj.get("properties").and_then(|p| p.as_object()),
            ) && let Some(name) = r.strip_prefix("#/$defs/")
                && let Some(target) = defs.and_then(|d| d.get(name))
            {
                let inner = collect_property_names(target);
                for outer in props.keys() {
                    if inner.contains(outer) {
                        out.push(format!("`{name}` 摊平后与外层的 `{outer}` 字段撞名"));
                    }
                }
            }

            for v in obj.values() {
                walk(v, defs, out);
            }
        } else if let Some(arr) = node.as_array() {
            for v in arr {
                walk(v, defs, out);
            }
        }
    }

    /// 收集一个 def 所有分支可能产生的属性名。
    fn collect_property_names(node: &serde_json::Value) -> std::collections::BTreeSet<String> {
        let mut names = std::collections::BTreeSet::new();
        if let Some(obj) = node.as_object() {
            if let Some(props) = obj.get("properties").and_then(|p| p.as_object()) {
                names.extend(props.keys().cloned());
            }
            for key in ["oneOf", "anyOf", "allOf"] {
                if let Some(arr) = obj.get(key).and_then(|v| v.as_array()) {
                    for branch in arr {
                        names.extend(collect_property_names(branch));
                    }
                }
            }
        }
        names
    }
}

/// 给 internally-tagged 枚举的「$ref + tag」分支补上显式 title。
///
/// 起因：json2ts 合并 `allOf: [$ref X]` 分支时会把 **X 的名字**用在
/// 合并产物上 —— `AgentEvent::Message` 那个包装分支抢走了 `Message`，
/// 真正的 Message def 被挤成 `Message1`。前端于是同时看到 `Message` /
/// `Message1` / `Message2` 三个类型，哪个是哪个全靠猜（真实踩过：
/// 会话历史被标成包装类型，结构恰好重叠所以编译过，语义是错的）。
///
/// 给这类分支起名 `{枚举名}{Tag值Pascal}`（如 `AgentEventDelta`），
/// 包装类型有了自己的名字，被引用的 def 保住原名。
fn title_tagged_variants(schema: &mut serde_json::Value) {
    let Some(defs) = schema
        .get_mut("$defs")
        .and_then(|d| d.as_object_mut())
    else {
        return;
    };
    for (enum_name, def) in defs.iter_mut() {
        // 两种 union 形态都要覆盖（schemars 对可缺省字段的分支用 anyOf）。
        for key in ["oneOf", "anyOf"] {
            let Some(branches) = def.get_mut(key).and_then(|v| v.as_array_mut()) else {
                continue;
            };
            for branch in branches {
                let Some(obj) = branch.as_object_mut() else {
                    continue;
                };
                if obj.contains_key("title") {
                    continue; // 已有名字的不动
                }
                // 只处理"$ref 带 sibling tag 常量"的分支 —— 正是 json2ts
                // 会偷名字的那种形状（schemars 对 internally-tagged
                // newtype 变体的产物）。
                if !obj.contains_key("$ref") {
                    continue;
                }
                let Some(tag_value) = obj.get("properties").and_then(|p| p.as_object()).and_then(
                    |props| {
                        props
                            .values()
                            .find_map(|v| v.get("const").and_then(|c| c.as_str()))
                    },
                ) else {
                    continue;
                };
                let title = format!("{enum_name}{}", pascal(tag_value));

                // 重排成「纯 allOf 双成员」。不能只加 title：json2ts 对
                // 「$ref + sibling properties」的分支一旦有了名字，会把
                // sibling 整个丢掉 —— 产物里连 tag 判别字段都没了，
                // 前端的 Extract<AgentEvent, {type:"…"}> 全变 never。
                let reff = obj.remove("$ref").expect("上面刚查过");
                let mut tag_member = serde_json::Map::new();
                tag_member.insert("type".into(), serde_json::Value::String("object".into()));
                if let Some(p) = obj.remove("properties") {
                    tag_member.insert("properties".into(), p);
                }
                if let Some(r) = obj.remove("required") {
                    tag_member.insert("required".into(), r);
                }
                obj.remove("type");
                obj.insert(
                    "allOf".into(),
                    serde_json::Value::Array(vec![
                        serde_json::json!({ "$ref": reff }),
                        serde_json::Value::Object(tag_member),
                    ]),
                );
                obj.insert("title".into(), serde_json::Value::String(title));
            }
        }
    }
}

/// 把「`$ref` + sibling 键」统一改写成 `{"allOf":[{"$ref":…}], …}`。
///
/// draft 2020-12 允许 `$ref` 带 sibling（schemars 给带 doc 注释的字段
/// 生成的就是 `{"$ref":…,"description":…}`），但 json2ts 处理这种形态
/// 时不复用被引类型，而是**分叉一份副本**加数字后缀 —— 于是同一个
/// `ModelEndpoint` 在前端变成内容完全相同的 `ModelEndpoint` 和
/// `ModelEndpoint1`，用哪个全靠猜。allOf 形态它就正常复用。
///
/// 要跑在 [`title_tagged_variants`] **之后**：那个 pass 靠「`$ref` 带
/// sibling tag」认 internally-tagged 分支，先归一化会让它认不出来。
fn hoist_ref_siblings(node: &mut serde_json::Value) {
    if let Some(obj) = node.as_object_mut() {
        if obj.contains_key("$ref") && obj.len() > 1 {
            let reff = obj.remove("$ref").expect("刚查过");
            obj.insert(
                "allOf".into(),
                serde_json::Value::Array(vec![serde_json::json!({ "$ref": reff })]),
            );
        }
        for v in obj.values_mut() {
            hoist_ref_siblings(v);
        }
    } else if let Some(arr) = node.as_array_mut() {
        for v in arr {
            hoist_ref_siblings(v);
        }
    }
}

/// `tool_start` → `ToolStart`。
fn pascal(s: &str) -> String {
    s.split(['_', '-'])
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
                .unwrap_or_default()
        })
        .collect()
}

fn workspace_root() -> &'static Path {
    // CARGO_MANIFEST_DIR = crates/riot-protocol
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
}
