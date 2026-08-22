//! 已安装能力包的读取端。
//!
//! 只有"读磁盘上装了什么"这一半在这里 —— 下载、校验、解压、原子切换都在
//! 宿主的 `packs` 模块。这样分是因为内核要用的只是"包在哪、注入哪些环境
//! 变量",而那些都不需要网络和 OS 特权;把安装逻辑也拖进来会让内核依赖
//! reqwest 和解压库,黄金回放里就得连这些一起编。
//!
//! 豁免理由：同 `skills` —— 宿主层，读的是磁盘上装好的包清单。

#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 包根目录下的 `pack.json`。构建脚本写,这里读。
///
/// 里面的路径一律**相对包根**,由 [`InstalledPack`] 解析成绝对路径 ——
/// 包解压到用户的配置目录,构建时不可能知道那是哪。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackManifest {
    pub name: String,
    pub version: String,
    pub platform: String,
    #[serde(default)]
    pub source_runtime: Option<String>,
    /// 注入给工具子进程的环境变量,值是相对路径。
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// 拼到 PATH 最前面的目录,相对路径。
    #[serde(default)]
    pub path_prepend: Vec<String>,
    #[serde(default)]
    pub mcp_servers: Vec<PackMcpServer>,
    #[serde(default)]
    pub skills: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackMcpServer {
    pub id: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
}

/// 一个已经装好的包,连同它的根目录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPack {
    pub root: PathBuf,
    pub manifest: PackManifest,
}

impl InstalledPack {
    /// 把 manifest 里的相对路径解析成绝对路径。
    pub fn resolve(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    /// 注入给工具子进程的环境变量,值已是绝对路径。
    pub fn env(&self) -> Vec<(String, String)> {
        self.manifest
            .env
            .iter()
            .map(|(k, v)| (k.clone(), self.resolve(v).display().to_string()))
            .collect()
    }

    /// 拼到 PATH 前面的绝对路径目录。
    pub fn path_dirs(&self) -> Vec<PathBuf> {
        self.manifest
            .path_prepend
            .iter()
            .map(|p| self.resolve(p))
            .collect()
    }
}

/// 找一个已安装的包。`None` = 没装,或者装坏了(pack.json 读不出来)。
///
/// 每次都重读而不缓存:安装和卸载在运行期改变它,缓存的话用户装完包还得重启
/// 才能用。这文件只有几百字节。
pub fn installed(id: &str) -> Option<InstalledPack> {
    installed_in(&crate::config::packs_dir(&crate::config::config_path()), id)
}

/// 参数化版本,给测试用(同 [`crate::config::profiles_dir`] 的理由)。
pub fn installed_in(packs_root: &Path, id: &str) -> Option<InstalledPack> {
    let root = packs_root.join(id);
    let raw = std::fs::read_to_string(root.join("pack.json")).ok()?;
    match serde_json::from_str::<PackManifest>(&raw) {
        Ok(manifest) => Some(InstalledPack { root, manifest }),
        Err(e) => {
            tracing::warn!(error = %e, id, "pack.json 解析失败，当作未安装");
            None
        }
    }
}

/// 文档能力包。给 PATH 注入和 MCP 自动注册用。
pub fn doc_runtime() -> Option<InstalledPack> {
    installed("doc-runtime")
}

/// 文档能力包的 id。宿主侧的目录清单和这里要对得上。
pub const DOC_RUNTIME: &str = "doc-runtime";

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, id: &str, body: serde_json::Value) {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).expect("建目录");
        std::fs::write(dir.join("pack.json"), body.to_string()).expect("写 pack.json");
    }

    /// 构建脚本写的是 camelCase。按默认的 snake_case 解析会让 `pathPrepend`
    /// 和 `mcpServers` 静默变成空 —— 包看起来装好了，但 PATH 没注入、MCP 没
    /// 注册，模型只会报"找不到 soffice"，谁也想不到是反序列化的问题。
    #[test]
    fn 按_camel_case_解析() {
        let t = tempfile::tempdir().expect("临时目录");
        write(
            t.path(),
            "doc-runtime",
            serde_json::json!({
                "name": "doc-runtime",
                "version": "0.1.0",
                "platform": "darwin-arm64",
                "sourceRuntime": "26.819.11345",
                "env": { "RUNTIME_BIN_DIR": "bin", "RUNTIME_NODE": "bin/node" },
                "pathPrepend": ["bin"],
                "mcpServers": [{
                    "id": "doc-artifact-tool",
                    "command": "bin/node",
                    "args": ["node/node_modules/@oai/artifact-tool/dist/artifact-session-mcp/server.mjs"]
                }],
                "skills": ["documents", "spreadsheets", "presentations", "pdf"],
            }),
        );

        let p = installed_in(t.path(), "doc-runtime").expect("应该找得到");
        assert_eq!(p.manifest.source_runtime.as_deref(), Some("26.819.11345"));
        assert_eq!(p.manifest.path_prepend, vec!["bin"]);
        assert_eq!(p.manifest.mcp_servers.len(), 1);
        assert_eq!(p.manifest.skills.len(), 4);
        assert_eq!(
            p.path_dirs(),
            vec![t.path().join("doc-runtime").join("bin")]
        );
        assert_eq!(
            p.env(),
            vec![
                (
                    "RUNTIME_BIN_DIR".to_owned(),
                    t.path().join("doc-runtime/bin").display().to_string()
                ),
                (
                    "RUNTIME_NODE".to_owned(),
                    t.path().join("doc-runtime/bin/node").display().to_string()
                ),
            ]
        );
    }

    #[test]
    fn 没装或装坏都当作未安装() {
        let t = tempfile::tempdir().expect("临时目录");
        assert!(installed_in(t.path(), "doc-runtime").is_none(), "没装");

        let dir = t.path().join("broken");
        std::fs::create_dir_all(&dir).expect("建目录");
        std::fs::write(dir.join("pack.json"), "{ 这不是 json").expect("写坏文件");
        assert!(
            installed_in(t.path(), "broken").is_none(),
            "装坏了要当作没装，不能让半个包参与接线"
        );
    }
}
