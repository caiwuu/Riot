//! 可下载的能力包。
//!
//! Riot 的安装包里不带 Python、Node、LibreOffice —— 那是几百 MB,而且大多数
//! 用户用不到。文档能力做成一个按需下载的包:用户在设置里点一下,下完解压到
//! 配置目录,skill、MCP server、PATH 三条线自动接上。
//!
//! 关键约束是**目标机器上没有开发环境**。包内所有二进制互相之间用相对路径
//! 引用,不依赖系统的 python / node / brew,也不在安装时编译任何东西。
//!
//! 磁盘布局:
//! ```text
//! <config_dir>/riot/packs/
//!   doc-runtime/                当前安装。pack.json 在里面,版本从它读
//!   .cache/<file>.part          断点续传的半成品
//!   .staging-<id>-<nonce>/      解压中间态,校验通过后原子 rename 过去
//! ```
//!
//! 豁免理由：宿主层，这个模块的职责就是操作真实的网络、磁盘和进程 ——
//! 下载、解压、把包里的二进制真的跑起来自检。

#![allow(clippy::disallowed_methods)]

mod download;
mod install;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use install::InstallError;
// 读取端在内核里 —— 内核要按它注入 PATH 和注册 MCP，而那不需要网络和解压。
pub use riot_kernel::packs::{InstalledPack, PackManifest, PackMcpServer, doc_runtime, installed};

/// 发布清单的地址。
///
/// 清单读仓库文件而不是 release 资产:改清单的场合多半是下线某个坏掉的版本或
/// 改个下载地址,那种时候不该被迫再发一版。包体本身仍然走 Releases —— 两百多
/// MB 的东西提交不进 git(GitHub 单文件上限 100MB)。
const MANIFEST_URL: &str = "https://raw.githubusercontent.com/caiwuu/riot-pkg/main/packs.json";

/// 清单地址的覆盖开关。
///
/// 存在的理由不只是测试:包有几百 MB,发布前必须能拿真的安装流程指着一份
/// 预发清单跑一遍。没有这个开关的话,验证"下载—校验—解压—自检"这条链路
/// 就只能靠先把包推上正式地址,推错了所有用户立刻就下到了。
const MANIFEST_URL_ENV: &str = "RIOT_PACKS_MANIFEST_URL";

fn manifest_url() -> String {
    #[allow(clippy::disallowed_methods)]
    std::env::var(MANIFEST_URL_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| MANIFEST_URL.to_owned())
}

/// 内置的能力包目录。目前只有文档一个,但结构留给后来的。
///
/// 说明文字放这里而不放远端清单:没网或清单拉不到时,设置页也该能告诉用户
/// 这个包是干什么的。
pub const CATALOG: &[CatalogEntry] = &[CatalogEntry {
    id: "doc-runtime",
    name: "文档能力",
    description: "创建和编辑 Word、Excel、PowerPoint、PDF。自带 Python、Node、\
                  LibreOffice 和中文字体,不需要你的电脑上装任何开发环境。",
}];

pub struct CatalogEntry {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
}

// ── 目录 ──────────────────────────────────────────────

/// 所有能力包的根目录。路径约定在内核的 config 里，和 `profiles_dir` 并列。
pub fn packs_dir() -> PathBuf {
    crate::config::packs_dir(&crate::config::config_path())
}

fn pack_dir(id: &str) -> PathBuf {
    packs_dir().join(id)
}

fn cache_dir() -> PathBuf {
    packs_dir().join(".cache")
}

// ── 远端清单 ──────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct RemoteManifest {
    #[serde(default)]
    pub packs: std::collections::BTreeMap<String, RemotePack>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemotePack {
    pub version: String,
    #[serde(default)]
    pub platforms: std::collections::BTreeMap<String, RemoteAsset>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAsset {
    pub url: String,
    pub sha256: String,
    pub size: u64,
    /// 解压后占多少盘。设置页拿它提示用户,免得下完才发现空间不够。
    #[serde(default)]
    pub installed_size: u64,
}

/// 当前平台在发布清单里的键。必须和构建脚本产出的一致。
pub fn platform_key() -> String {
    let os = if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "win"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    };
    format!("{os}-{arch}")
}

async fn fetch_manifest() -> Result<RemoteManifest, String> {
    let res = reqwest::Client::new()
        .get(manifest_url())
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| format!("拉取能力包清单失败：{e}"))?;
    if !res.status().is_success() {
        return Err(format!("能力包清单返回 {}", res.status()));
    }
    // 不用 reqwest 的 json()：那要开 `json` feature，而 workspace 里的 reqwest
    // 是所有 crate 共用的，为一处调用加特性不划算。
    let body = res
        .text()
        .await
        .map_err(|e| format!("读取能力包清单失败：{e}"))?;
    serde_json::from_str(&body).map_err(|e| format!("能力包清单解析失败：{e}"))
}

// ── 给前端的状态 ──────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackStatus {
    pub id: String,
    pub name: String,
    pub description: String,
    /// 已装版本。null = 没装。
    pub installed_version: Option<String>,
    /// 远端可装版本。null = 清单没拉到,或这个平台没有对应的包。
    pub available_version: Option<String>,
    /// 下载体积,字节。
    pub download_size: u64,
    /// 解压后体积,字节。
    pub installed_size: u64,
    /// 这个平台有没有对应的包。
    pub supported: bool,
    /// 清单拉取失败的原因。有值时前端显示"离线",而不是"没有可用更新"。
    pub manifest_error: Option<String>,
}

/// 能力包列表。设置页轮询它。
///
/// 清单拉不到时**不报错** —— 已装的包照样要显示、照样能用,只是看不到更新。
/// 把网络故障升级成整页报错会让离线用户以为功能坏了。
pub async fn status() -> Vec<PackStatus> {
    let platform = platform_key();
    let (remote, err) = match fetch_manifest().await {
        Ok(m) => (Some(m), None),
        Err(e) => {
            tracing::debug!(error = %e, "能力包清单拉取失败");
            (None, Some(e))
        }
    };

    CATALOG
        .iter()
        .map(|entry| {
            let asset = remote
                .as_ref()
                .and_then(|m| m.packs.get(entry.id))
                .and_then(|p| p.platforms.get(&platform).map(|a| (p.version.clone(), a)));
            PackStatus {
                id: entry.id.to_owned(),
                name: entry.name.to_owned(),
                description: entry.description.to_owned(),
                installed_version: installed(entry.id).map(|p| p.manifest.version),
                available_version: asset.as_ref().map(|(v, _)| v.clone()),
                download_size: asset.as_ref().map_or(0, |(_, a)| a.size),
                installed_size: asset.as_ref().map_or(0, |(_, a)| a.installed_size),
                supported: remote
                    .as_ref()
                    .and_then(|m| m.packs.get(entry.id))
                    .is_none_or(|p| p.platforms.contains_key(&platform)),
                manifest_error: err.clone(),
            }
        })
        .collect()
}

// ── 安装进度 ──────────────────────────────────────────

/// 安装过程推给前端的进度。
///
/// 分这么细是因为各阶段的耗时量级差着数量级:下载几分钟、解压十几秒、
/// 接线一瞬间。只报一个百分比的话,进度条会在 100% 处静止十几秒,
/// 用户以为卡死了。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PackProgress {
    #[serde(rename_all = "camelCase")]
    Downloading {
        received: u64,
        total: u64,
    },
    Verifying,
    Extracting,
    /// 校验二进制在这台机器上真的能跑起来。
    SelfCheck,
    Done {
        version: String,
    },
    Failed {
        error: String,
    },
}

/// 下载并安装一个能力包。已装同版本则直接返回。
///
/// 调用方负责在成功后做接线(skills / MCP / PATH)—— 那些要碰 `AppState`,
/// 不属于这一层。
pub async fn install(
    id: &str,
    progress: impl Fn(PackProgress) + Send + Sync + 'static,
) -> Result<InstalledPack, InstallError> {
    let platform = platform_key();
    let manifest = fetch_manifest().await.map_err(InstallError::Manifest)?;
    let pack = manifest
        .packs
        .get(id)
        .ok_or_else(|| InstallError::NotFound(id.to_owned()))?;
    let asset = pack
        .platforms
        .get(&platform)
        .ok_or_else(|| InstallError::Unsupported(platform.clone()))?;

    if let Some(existing) = installed(id)
        && existing.manifest.version == pack.version
    {
        progress(PackProgress::Done {
            version: pack.version.clone(),
        });
        return Ok(existing);
    }

    let cache = cache_dir();
    std::fs::create_dir_all(&cache).map_err(|e| InstallError::Io("建缓存目录".into(), e))?;
    let archive = cache.join(format!("{id}-{}.tar.zst", pack.version));

    download::fetch(&asset.url, &archive, asset.size, &asset.sha256, &progress).await?;

    progress(PackProgress::Extracting);
    let root = install::unpack(&archive, &pack_dir(id))?;

    progress(PackProgress::SelfCheck);
    let installed = install::finalize(&root)?;

    // 装完就把压缩包删掉。它和解压出来的内容加起来是两份几百 MB,
    // 留着只在"同一版本重装"这一个场景有用,不值这个盘。
    let _ = std::fs::remove_file(&archive);

    progress(PackProgress::Done {
        version: installed.manifest.version.clone(),
    });
    Ok(installed)
}

/// 卸载。幂等 —— 没装也算成功。
pub fn uninstall(id: &str) -> Result<(), InstallError> {
    let dir = pack_dir(id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| InstallError::Io("删除能力包目录".into(), e))?;
    }
    Ok(())
}

// ── MCP 自动注册 ──────────────────────────────────────

/// 让配置里的 MCP 服务器与"当前装了哪些包"对齐。返回是否改动了配置。
///
/// 装完包还要用户自己去 MCP 设置里手填一条命令的话，这个功能等于没做 ——
/// 目标用户根本不知道 MCP 是什么。
///
/// 归属判断按 command 路径是否落在能力包目录下，而不是靠 id 前缀或者在
/// `McpServerConfig` 上加字段：id 前缀会泄漏到工具名里（模型看到
/// `pack__doc_artifact_tool` 这种东西），加字段则要动一个用户会手编的
/// 配置结构。路径是现成的、不会撒谎的事实。
///
/// `enabled` 跨升级保留 —— 用户特意关掉过的服务器，不该因为包升级又自己开回来。
pub fn sync_mcp(config: &mut riot_kernel::config::AppConfig) -> bool {
    sync_mcp_in(config, &packs_dir())
}

/// 参数化版本，给测试用（同 `config::profiles_dir` 的理由）。
fn sync_mcp_in(config: &mut riot_kernel::config::AppConfig, root: &std::path::Path) -> bool {
    let before = config.mcp_servers.clone();

    // 先收掉所有属于能力包的条目，再按当前实际装了什么整体重建。增量比对
    // 要处理"包升级后路径变了""包卸载了"等一堆情况，重建只有一种情况。
    let previous: Vec<_> = config
        .mcp_servers
        .iter()
        .filter(|s| std::path::Path::new(&s.command).starts_with(root))
        .cloned()
        .collect();
    config
        .mcp_servers
        .retain(|s| !std::path::Path::new(&s.command).starts_with(root));

    for entry in CATALOG {
        let Some(pack) = riot_kernel::packs::installed_in(root, entry.id) else {
            continue;
        };
        for spec in &pack.manifest.mcp_servers {
            let mut env = spec.env.clone();
            // artifact-tool 把会话快照写在 PLUGIN_DATA 下。不给的话它会往
            // 自己的安装目录里写，而那个目录在升级时会被整个替换掉。
            env.entry("PLUGIN_DATA".to_owned()).or_insert_with(|| {
                root.join(".data")
                    .join(&pack.manifest.name)
                    .display()
                    .to_string()
            });
            config
                .mcp_servers
                .push(riot_kernel::config::McpServerConfig {
                    id: spec.id.clone(),
                    name: format!("{}（能力包）", entry.name),
                    command: pack.resolve(&spec.command).display().to_string(),
                    args: spec.args.iter().map(|a| resolve_arg(&pack, a)).collect(),
                    env,
                    enabled: previous
                        .iter()
                        .find(|p| p.id == spec.id)
                        .is_none_or(|p| p.enabled),
                });
        }
    }

    config.mcp_servers != before
}

/// 参数里的相对路径要展开成绝对路径，但不能把 `--flag` 这种也当路径拼 ——
/// 那会生成 `<pack>/--flag`，服务器起不来而且报错完全指不到原因。
/// 判据是"拼出来的东西真的存在"，比按前缀猜可靠。
fn resolve_arg(pack: &InstalledPack, arg: &str) -> String {
    let joined = pack.resolve(arg);
    if joined.exists() {
        joined.display().to_string()
    } else {
        arg.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 平台键与构建脚本一致() {
        let k = platform_key();
        assert!(
            [
                "darwin-arm64",
                "darwin-x64",
                "win-x64",
                "win-arm64",
                "linux-x64",
                "linux-arm64"
            ]
            .contains(&k.as_str()),
            "平台键 {k} 不在已知集合里"
        );
    }

    /// 远端清单也是构建脚本写的 camelCase。`installedSize` 掉了的话，设置页
    /// 会显示"需要 0 MB 空间"，用户点下去才发现盘不够。
    #[test]
    fn 远端清单按_camel_case_解析() {
        let raw = r#"{
          "schemaVersion": 1,
          "packs": {
            "doc-runtime": {
              "version": "0.1.0",
              "platforms": {
                "darwin-arm64": {
                  "url": "https://example.com/doc-runtime.tar.zst",
                  "sha256": "abc",
                  "size": 123,
                  "installedSize": 456
                }
              }
            }
          }
        }"#;
        let m: RemoteManifest = serde_json::from_str(raw).expect("解析清单");
        let asset = &m.packs["doc-runtime"].platforms["darwin-arm64"];
        assert_eq!(asset.size, 123);
        assert_eq!(asset.installed_size, 456);
    }

    #[test]
    fn 未安装时返回_none() {
        // 真实配置目录下不该有这个 id,借它验证"读不到就是没装"这条路径。
        assert!(installed("这个包不存在").is_none());
    }

    // ── MCP 自动注册 ──────────────────────────────

    /// 铺一个假的 doc-runtime，只有 pack.json 和被 args 指到的那个文件。
    fn fake_pack(root: &std::path::Path) {
        let dir = root.join("doc-runtime");
        let server = dir.join("node/node_modules/@oai/artifact-tool/dist/server.mjs");
        std::fs::create_dir_all(server.parent().expect("有父目录")).expect("建目录");
        std::fs::write(&server, "//").expect("写 server.mjs");
        std::fs::write(
            dir.join("pack.json"),
            serde_json::json!({
                "name": "doc-runtime",
                "version": "0.1.0",
                "platform": "darwin-arm64",
                "mcpServers": [{
                    "id": "doc-artifact-tool",
                    "command": "bin/node",
                    "args": ["node/node_modules/@oai/artifact-tool/dist/server.mjs", "--stdio"],
                }],
            })
            .to_string(),
        )
        .expect("写 pack.json");
    }

    #[test]
    fn 装上包会注册_mcp_并把相对路径展开() {
        let t = tempfile::tempdir().expect("临时目录");
        fake_pack(t.path());
        let mut config = riot_kernel::config::AppConfig::default();

        assert!(sync_mcp_in(&mut config, t.path()), "应该报告有改动");

        let s = config
            .mcp_servers
            .iter()
            .find(|s| s.id == "doc-artifact-tool")
            .expect("要注册上");
        assert_eq!(
            s.command,
            t.path().join("doc-runtime/bin/node").display().to_string()
        );
        assert_eq!(
            s.args[0],
            t.path()
                .join("doc-runtime/node/node_modules/@oai/artifact-tool/dist/server.mjs")
                .display()
                .to_string(),
            "相对路径要展开"
        );
        assert_eq!(s.args[1], "--stdio", "非路径参数要原样保留");
        assert!(
            s.env.contains_key("PLUGIN_DATA"),
            "会话快照要写到包外，否则升级时连同包目录一起被替换掉"
        );
    }

    #[test]
    fn 卸载后对应的_mcp_条目消失而用户自己的留着() {
        let t = tempfile::tempdir().expect("临时目录");
        fake_pack(t.path());
        let mut config = riot_kernel::config::AppConfig::default();
        config
            .mcp_servers
            .push(riot_kernel::config::McpServerConfig {
                id: "my-own".into(),
                name: String::new(),
                command: "/usr/local/bin/whatever".into(),
                args: vec![],
                env: Default::default(),
                enabled: true,
            });
        sync_mcp_in(&mut config, t.path());
        assert_eq!(config.mcp_servers.len(), 2);

        std::fs::remove_dir_all(t.path().join("doc-runtime")).expect("卸载");
        assert!(sync_mcp_in(&mut config, t.path()), "应该报告有改动");

        assert_eq!(
            config.mcp_servers.iter().map(|s| &s.id).collect::<Vec<_>>(),
            vec!["my-own"],
            "只该收掉能力包自己那条"
        );
    }

    /// 用户特意关掉过的服务器，不该因为包升级又自己开回来。
    #[test]
    fn 升级保留用户关闭的状态() {
        let t = tempfile::tempdir().expect("临时目录");
        fake_pack(t.path());
        let mut config = riot_kernel::config::AppConfig::default();
        sync_mcp_in(&mut config, t.path());
        config.mcp_servers[0].enabled = false;

        sync_mcp_in(&mut config, t.path());

        assert!(!config.mcp_servers[0].enabled, "关掉的状态要跨同步保留");
    }

    #[test]
    fn 没有变化时不报告改动() {
        let t = tempfile::tempdir().expect("临时目录");
        fake_pack(t.path());
        let mut config = riot_kernel::config::AppConfig::default();
        sync_mcp_in(&mut config, t.path());

        assert!(
            !sync_mcp_in(&mut config, t.path()),
            "重复同步不该被当成改动 —— 否则每次启动都会白存一次配置并重连 MCP"
        );
    }
}
