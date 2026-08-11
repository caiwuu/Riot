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
}

// 豁免理由：这是构建期的代码生成器，不是内核代码。它的产物本身就是
// 磁盘文件，没有注入 FileSystem 的意义，也不参与黄金回放。
#[allow(clippy::disallowed_methods)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = workspace_root();
    let out_dir = root.join("schemas");
    std::fs::create_dir_all(&out_dir)?;

    let schema = schemars::schema_for!(ProtocolRoot);
    let value = serde_json::to_value(&schema)?;

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

    let mut json = serde_json::to_string_pretty(&schema)?;
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

fn workspace_root() -> &'static Path {
    // CARGO_MANIFEST_DIR = crates/riot-protocol
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
}
