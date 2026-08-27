//! 能力包从"远端有个 tar.zst"到"模型能用上"的整条链路。
//!
//! 这一条覆盖的是真实用户唯一会走的那条路径：点安装 → HTTP 下载 → sha256
//! 校验 → 解压原子切换 → 跑自检 → skill 被发现、MCP 被注册、PATH 被注入。
//!
//! 分层的单测各自都过，不等于这条链路通 —— 它们中间靠的是**约定**：构建脚本
//! 写出的 `pack.json` 字段名要和 Rust 的 serde 对得上，`selfCheck` 里的相对
//! 路径要和解压后的实际布局对得上，`pathPrepend` 要和 `DocPackRunner` 拼 PATH
//! 的方式对得上。这些约定跨了 JS 和 Rust 两侧、跨了四个模块，任何一处改名都
//! 不会有编译错误，只会在用户装完包之后表现为"装是装上了，模型说找不到工具"。
//!
//! 所以这里不 mock 任何一层：起一个真的 HTTP 服务器，喂一个真的 tar.zst，
//! 跑真的 `packs::install`。
//!
//! 用的是一个几 KB 的合成包而不是真的 900MB 文档包：这条链路要验的是接线，
//! 不是 LibreOffice 能不能渲染。后者由 `scripts/doc-pack/verify-pack.mjs` 负责。

#![cfg(unix)]
#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use riot_host_lib::packs;

/// 合成一个能力包目录。结构和 `scripts/build-doc-pack.mjs` 产出的一致。
fn build_pack(dir: &Path, version: &str) {
    let bin = dir.join("bin");
    let path_dir = dir.join("path");
    std::fs::create_dir_all(&bin).expect("建 bin");
    std::fs::create_dir_all(&path_dir).expect("建 path");
    std::fs::create_dir_all(dir.join("skills/demo")).expect("建 skills");

    // 自检要跑得起来的真脚本。带一个可辨识的输出，失败时能看出跑的是哪个。
    for (dir, name) in [(&bin, "fake-python3"), (&path_dir, "fake-soffice")] {
        let p = dir.join(name);
        std::fs::write(&p, "#!/bin/sh\necho ok-$(basename \"$0\")\n").expect("写脚本");
        set_exec(&p);
    }

    std::fs::write(
        dir.join("skills/demo/SKILL.md"),
        "---\nname: 能力包演示\ndescription: 由能力包带进来的技能，用于验证发现链路。\n---\n正文。\n",
    )
    .expect("写 SKILL.md");

    std::fs::write(
        dir.join("pack.json"),
        serde_json::json!({
            "name": "doc-runtime",
            "version": version,
            "platform": packs::platform_key(),
            "env": {
                "RUNTIME_BIN_DIR": "bin",
                "RUNTIME_NODE": "bin/fake-python3",
            },
            "pathPrepend": ["path"],
            "selfCheck": [
                { "command": "bin/fake-python3", "args": ["--version"] },
            ],
            "mcpServers": [{
                "id": "doc-artifact-tool",
                "command": "bin/fake-python3",
                "args": ["--stdio", "skills/demo/SKILL.md"],
            }],
            "skills": ["demo"],
        })
        .to_string(),
    )
    .expect("写 pack.json");
}

fn set_exec(p: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).expect("加可执行位");
}

/// 打成 tar.zst，外面裹一层顶层目录 —— 构建脚本就是这么打的，安装时要剥掉。
fn tarball(src: &Path, out: &Path, top: &str) -> (u64, String) {
    use sha2::{Digest as _, Sha256};

    let file = std::fs::File::create(out).expect("建压缩文件");
    let enc = zstd::stream::write::Encoder::new(file, 3).expect("建 zstd 编码器");
    let mut builder = tar::Builder::new(enc);
    builder.follow_symlinks(false);
    builder.append_dir_all(top, src).expect("打 tar");
    builder
        .into_inner()
        .expect("收尾 tar")
        .finish()
        .expect("收尾 zstd");

    let bytes = std::fs::read(out).expect("读回压缩文件");
    let mut h = Sha256::new();
    h.update(&bytes);
    (bytes.len() as u64, format!("{:x}", h.finalize()))
}

/// 这几个用例都要改 `XDG_CONFIG_HOME` 和清单地址，而环境变量是进程级的。
/// 同进程里并行跑会互相把对方的配置目录和清单地址覆盖掉。
///
/// 用 tokio 的锁而不是 `std::sync::Mutex`：这几个用例都要跨 `.await` 持锁，
/// 拿 std 的锁会把执行器线程整个堵住。
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 一个只认 GET 的极简 HTTP 服务器。
///
/// 先绑端口再挂路由：清单里要写下载地址，而地址得等端口分配完才知道。
/// 不引 axum 之类，要的只是"能被 reqwest 正常下下来"。
struct Server {
    listener: Option<tokio::net::TcpListener>,
    base: String,
}

impl Server {
    async fn bind() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("监听端口");
        let base = format!("http://{}", listener.local_addr().expect("取地址"));
        Self {
            listener: Some(listener),
            base,
        }
    }

    fn serve(mut self, routes: Vec<(String, Vec<u8>)>) {
        serve_on(self.listener.take().expect("listener 还在"), routes);
    }
}

fn serve_on(listener: tokio::net::TcpListener, routes: Vec<(String, Vec<u8>)>) {
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let routes = routes.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .split('?')
                    .next()
                    .unwrap_or("/")
                    .to_owned();

                let body = routes.iter().find(|(p, _)| *p == path).map(|(_, b)| b);
                let head = match body {
                    Some(b) => format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        b.len()
                    ),
                    None => {
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_owned()
                    }
                };
                let _ = sock.write_all(head.as_bytes()).await;
                if let Some(b) = body {
                    let _ = sock.write_all(b).await;
                }
                let _ = sock.flush().await;
            });
        }
    });
}

/// 清单正常给,包体第一次只写前 `drop_after` 字节就关连接(Content-Length
/// 仍是全长,复现线上的 "error decoding response body"),之后认 Range。
fn serve_flaky(
    listener: tokio::net::TcpListener,
    manifest: Vec<u8>,
    archive: Vec<u8>,
    drop_after: usize,
) {
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let mut dropped = false;
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req
                .split_whitespace()
                .nth(1)
                .unwrap_or("/")
                .split('?')
                .next()
                .unwrap_or("/")
                .to_owned();
            let range_start = req.lines().find_map(|line| {
                let (k, v) = line.split_once(':')?;
                if !k.trim().eq_ignore_ascii_case("Range") {
                    return None;
                }
                let rest = v.trim().strip_prefix("bytes=")?;
                let (s, _) = rest.split_once('-')?;
                s.trim().parse::<usize>().ok()
            });

            if path == "/packs.json" {
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    manifest.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(&manifest).await;
                let _ = sock.shutdown().await;
                continue;
            }
            if path != "/doc-runtime.tar.zst" {
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
                let _ = sock.shutdown().await;
                continue;
            }

            let start = range_start.unwrap_or(0).min(archive.len());
            if !dropped && start == 0 {
                dropped = true;
                let n = drop_after.min(archive.len());
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    archive.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(&archive[..n]).await;
                let _ = sock.shutdown().await;
                continue;
            }
            if start > 0 {
                let slice = &archive[start..];
                let end = archive.len().saturating_sub(1);
                let head = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    archive.len(),
                    slice.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(slice).await;
            } else {
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    archive.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(&archive).await;
            }
            let _ = sock.shutdown().await;
        }
    });
}

#[tokio::test]
async fn 从远端清单一路装到能用() {
    let _guard = SERIAL.lock().await;
    let tmp = tempfile::tempdir().expect("临时目录");
    // 整个测试进程共用这一个配置目录。config_path 走 XDG_CONFIG_HOME，
    // 一定要在碰任何 packs API 之前设好 —— 那些路径是每次现算的，但
    // 用真实的家目录跑一次就会往用户盘上写几百 MB。
    unsafe { std::env::set_var("XDG_CONFIG_HOME", tmp.path()) };

    let src = tmp.path().join("src");
    build_pack(&src, "1.0.0");
    let archive = tmp.path().join("doc-runtime-1.0.0.tar.zst");
    let (size, sha256) = tarball(&src, &archive, "doc-runtime-1.0.0-any");

    let server = Server::bind().await;
    let manifest = serde_json::json!({
        "schemaVersion": 1,
        "packs": {
            "doc-runtime": {
                "version": "1.0.0",
                "platforms": {
                    packs::platform_key(): {
                        "url": format!("{}/doc-runtime.tar.zst", server.base),
                        "sha256": sha256,
                        "size": size,
                        "installedSize": 4096,
                    }
                }
            }
        }
    });
    unsafe {
        std::env::set_var(
            "RIOT_PACKS_MANIFEST_URL",
            format!("{}/packs.json", server.base),
        )
    };
    server.serve(vec![
        ("/packs.json".to_owned(), manifest.to_string().into_bytes()),
        (
            "/doc-runtime.tar.zst".to_owned(),
            std::fs::read(&archive).expect("读压缩包"),
        ),
    ]);

    // —— 装之前 ————————————————————————————————————————
    let before = packs::status().await;
    let entry = before
        .iter()
        .find(|p| p.id == "doc-runtime")
        .expect("目录里应该有 doc-runtime");
    assert_eq!(entry.installed_version, None, "还没装");
    assert_eq!(
        entry.available_version.as_deref(),
        Some("1.0.0"),
        "清单里应该看得到可装版本，实际 {entry:?}"
    );

    // —— 装 ————————————————————————————————————————————
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = seen.clone();
    let pack = packs::install("doc-runtime", move |p| {
        sink.lock().expect("锁").push(format!("{p:?}"));
    })
    .await
    .expect("安装");

    let stages = seen.lock().expect("锁").join(" | ");
    for want in [
        "Downloading",
        "Verifying",
        "Extracting",
        "SelfCheck",
        "Done",
    ] {
        assert!(stages.contains(want), "进度里缺 {want}：{stages}");
    }

    // 顶层目录被剥掉了：定位器按固定路径找包，多一层就全找不到。
    assert!(
        pack.root.join("pack.json").is_file(),
        "pack.json 应该直接在包根下，实际 {}",
        pack.root.display()
    );
    assert_eq!(pack.manifest.version, "1.0.0");

    // 可执行位必须活过 tar 往返，否则 shim 全都跑不了（自检其实已经证明了
    // 这点 —— 它真的把 bin/fake-python3 跑起来了 —— 这里再断言一次是为了
    // 万一以后有人把自检改成可选）。
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::metadata(pack.root.join("bin/fake-python3"))
        .expect("读 shim 元信息")
        .permissions()
        .mode();
    assert!(mode & 0o111 != 0, "shim 丢了可执行位，mode={mode:o}");

    // —— 接线一：技能被发现 ————————————————————————————
    let skills = riot_kernel::skills::list(None);
    let demo = skills
        .iter()
        .find(|s| s.name == "能力包演示")
        .expect("能力包带来的技能应该被发现");
    assert_eq!(demo.source, "pack", "来源应标成 pack，实际 {}", demo.source);

    // —— 接线二：MCP 注册，相对路径展开成绝对路径 ————————
    let mut config = riot_kernel::config::AppConfig::default();
    assert!(packs::sync_mcp(&mut config), "应该改动了配置");
    let server = config
        .mcp_servers
        .iter()
        .find(|s| s.id == "doc-artifact-tool")
        .expect("MCP 服务器应该被注册");
    assert_eq!(
        server.command,
        pack.root.join("bin/fake-python3").display().to_string(),
        "command 必须是绝对路径 —— 相对路径会按宿主的 cwd 解析，那是用户的项目目录"
    );
    assert_eq!(
        server.args[0], "--stdio",
        "不是路径的参数不能被当成路径拼进包目录"
    );
    assert_eq!(
        server.args[1],
        pack.root.join("skills/demo/SKILL.md").display().to_string(),
        "是路径的参数要展开"
    );
    assert!(
        server.env.contains_key("PLUGIN_DATA"),
        "会话数据要落在包外，否则升级时连同包目录一起被替换掉"
    );

    // —— 接线三：PATH 注入只放 pathPrepend 声明的目录 ————————
    let dirs = pack.path_dirs();
    assert_eq!(dirs, vec![pack.root.join("path")], "实际 {dirs:?}");
    assert!(
        !dirs.contains(&pack.root.join("bin")),
        "bin 里有 python / node，进 PATH 会盖掉用户项目的虚拟环境"
    );

    // —— 幂等：同版本重装直接返回 ————————————————————————
    let again = packs::install("doc-runtime", |_| {}).await.expect("重装");
    assert_eq!(again.manifest.version, "1.0.0");

    let after = packs::status().await;
    let entry = after
        .iter()
        .find(|p| p.id == "doc-runtime")
        .expect("还在目录里");
    assert_eq!(entry.installed_version.as_deref(), Some("1.0.0"));

    // —— 卸载：包没了，MCP 条目也跟着摘掉 ————————————————
    packs::uninstall("doc-runtime").expect("卸载");
    assert!(!pack.root.exists(), "包目录应该被删掉");
    assert!(packs::sync_mcp(&mut config), "卸载后配置应该再次改动");
    assert!(
        !config
            .mcp_servers
            .iter()
            .any(|s| s.id == "doc-artifact-tool"),
        "卸载后不能留下一条指向已删目录的 MCP 配置 —— 那会让 MCP 面板\
         永远显示一个连不上的服务器，而用户在设置里找不到它是哪来的"
    );
    assert!(
        !riot_kernel::skills::list(None)
            .iter()
            .any(|s| s.name == "能力包演示"),
        "卸载后技能也要消失"
    );
}

/// 校验和对不上就必须拒绝，而且不能在磁盘上留下半个包。
#[tokio::test]
async fn 校验和不匹配时拒绝安装() {
    let _guard = SERIAL.lock().await;
    let tmp = tempfile::tempdir().expect("临时目录");
    unsafe { std::env::set_var("XDG_CONFIG_HOME", tmp.path()) };

    let src = tmp.path().join("src");
    build_pack(&src, "1.0.0");
    let archive = tmp.path().join("p.tar.zst");
    let (size, _) = tarball(&src, &archive, "p-1.0.0");

    let server = Server::bind().await;
    let manifest = serde_json::json!({
        "packs": { "doc-runtime": { "version": "1.0.0", "platforms": { packs::platform_key(): {
            // 一个语法合法但内容对不上的哈希：中间人改写或下到半截都长这样。
            "url": format!("{}/p.tar.zst", server.base),
            "sha256": "0".repeat(64),
            "size": size,
        }}}}
    });
    unsafe {
        std::env::set_var(
            "RIOT_PACKS_MANIFEST_URL",
            format!("{}/packs.json", server.base),
        )
    };
    server.serve(vec![
        ("/packs.json".to_owned(), manifest.to_string().into_bytes()),
        (
            "/p.tar.zst".to_owned(),
            std::fs::read(&archive).expect("读压缩包"),
        ),
    ]);

    let err = packs::install("doc-runtime", |_| {})
        .await
        .expect_err("应该拒绝");
    assert!(
        matches!(err, packs::InstallError::Checksum { .. }),
        "实际是 {err:?}"
    );
    assert!(
        !packs::packs_dir().join("doc-runtime").exists(),
        "校验失败不能留下一个半装的包 —— 状态会显示已安装，用起来到处报错"
    );
}

/// 自检跑不起来要当成安装失败。
///
/// 这是整个方案里最可能在用户机器上翻车的一环：包里的二进制是从 Codex 运行时
/// 提取的，`soffice` 和 `python3.12` 只有 ad-hoc 签名。真被系统拦下时，要在
/// 用户刚点完"安装"的时候报出来，而不是几天后让模型撞上一条看不懂的报错。
#[tokio::test]
async fn 自检失败会让安装失败() {
    let _guard = SERIAL.lock().await;
    let tmp = tempfile::tempdir().expect("临时目录");
    unsafe { std::env::set_var("XDG_CONFIG_HOME", tmp.path()) };

    let src = tmp.path().join("src");
    build_pack(&src, "1.0.0");
    // 模拟"文件在，但这台机器上执行不了"：丢掉可执行位。
    let broken = src.join("bin/fake-python3");
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o644)).expect("去可执行位");

    let archive = tmp.path().join("p.tar.zst");
    let (size, sha256) = tarball(&src, &archive, "p-1.0.0");

    let server = Server::bind().await;
    let manifest = serde_json::json!({
        "packs": { "doc-runtime": { "version": "1.0.0", "platforms": { packs::platform_key(): {
            "url": format!("{}/p.tar.zst", server.base), "sha256": sha256, "size": size,
        }}}}
    });
    unsafe {
        std::env::set_var(
            "RIOT_PACKS_MANIFEST_URL",
            format!("{}/packs.json", server.base),
        )
    };
    server.serve(vec![
        ("/packs.json".to_owned(), manifest.to_string().into_bytes()),
        (
            "/p.tar.zst".to_owned(),
            std::fs::read(&archive).expect("读压缩包"),
        ),
    ]);

    let err = packs::install("doc-runtime", |_| {})
        .await
        .expect_err("应该失败");
    assert!(
        matches!(err, packs::InstallError::SelfCheck(_)),
        "实际是 {err:?}"
    );
}

/// 拿**真的**能力包走一遍安装。
///
/// 上面那条用的是几 KB 的合成包，验的是接线；这条验的是"从 Codex 运行时提取
/// 出来的那堆二进制，在一个干净的环境里到底跑不跑得起来"—— 也就是整个方案里
/// 最可能翻车的一环：`soffice` 和 `python3.12` 只有 ad-hoc 签名。
///
/// 挂 `#[ignore]` 是因为它依赖本机先跑过构建脚本，而且要解压近 1GB：
/// `node scripts/build-doc-pack.mjs` 之后
/// `cargo test -p riot-host --test packs_e2e -- --ignored --nocapture`
#[tokio::test]
#[ignore = "要先跑构建脚本产出真包，且要解压近 1GB"]
async fn 真包在最小环境里装得上() {
    let _guard = SERIAL.lock().await;
    // 构建脚本把成品写进能力包仓库，不是 workspace 的 dist —— 那个仓库默认在
    // Riot 旁边，和构建脚本的默认输出保持一致。
    let dist = std::env::var_os("RIOT_PKG_REPO")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .and_then(|p| p.parent())
                .expect("workspace 上一级")
                .join("riot-pkg")
        })
        .join("doc-runtime")
        .join(packs::platform_key());
    let manifest_path = dist.join("packs.json");
    // `[约束]` 清单**不存在**是「前置条件没满足」，不是「产品坏了」——跳过，
    // 不 panic。这条测试挂 `#[ignore]` 的理由是要先手动跑构建脚本产出近 1GB
    // 的真包，而 CI 的 chaos-host 跑的是 `-- --ignored`：它把所有 ignored
    // 测试不加区分地一起跑，于是这条必然红，而红的原因和被测的东西无关。
    //
    // 只对 NotFound 跳过。清单在但读不动（权限、损坏）是真问题，照常炸。
    let raw = match std::fs::read_to_string(&manifest_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "{} 不在，跳过。这条要先跑 `node scripts/build-doc-pack.mjs` \
                 产出真包（近 1GB），或用 RIOT_PKG_REPO 指向已有的包仓库。",
                manifest_path.display()
            );
            return;
        }
        Err(e) => panic!("{} 读不动（{e}）", manifest_path.display()),
    };
    let doc = serde_json::from_str::<serde_json::Value>(&raw).expect("解析 packs.json");
    let asset = doc["packs"]["doc-runtime"]["platforms"][packs::platform_key()].clone();
    assert!(
        !asset.is_null(),
        "packs.json 里没有 {} 的包",
        packs::platform_key()
    );

    let file = asset["url"]
        .as_str()
        .and_then(|u| u.rsplit('/').next())
        .expect("url 里取文件名");
    let archive = dist.join(file);
    let bytes =
        std::fs::read(&archive).unwrap_or_else(|e| panic!("{} 读不到（{e}）", archive.display()));

    let tmp = tempfile::tempdir().expect("临时目录");
    unsafe { std::env::set_var("XDG_CONFIG_HOME", tmp.path()) };

    let server = Server::bind().await;
    let local = serde_json::json!({
        "packs": { "doc-runtime": {
            "version": doc["packs"]["doc-runtime"]["version"],
            "platforms": { packs::platform_key(): {
                "url": format!("{}/{file}", server.base),
                "sha256": asset["sha256"],
                "size": asset["size"],
                "installedSize": asset["installedSize"],
            }}
        }}
    });
    unsafe {
        std::env::set_var(
            "RIOT_PACKS_MANIFEST_URL",
            format!("{}/packs.json", server.base),
        )
    };
    server.serve(vec![
        ("/packs.json".to_owned(), local.to_string().into_bytes()),
        (format!("/{file}"), bytes),
    ]);

    let started = std::time::Instant::now();
    // 自检就在 install 里：它会用最小 PATH 真的把 python3、node、soffice、
    // pdftoppm 各跑一遍。这一步过了，就说明 ad-hoc 签名的二进制在这台机器上
    // 确实能执行 —— 那正是这条用例存在的意义。
    let pack = packs::install("doc-runtime", |_| {})
        .await
        .expect("真包应该装得上（失败信息里会写是哪个二进制起不来）");
    eprintln!(
        "装好 {} {}，耗时 {:?}",
        pack.manifest.name,
        pack.manifest.version,
        started.elapsed()
    );

    for skill in &pack.manifest.skills {
        assert!(
            pack.root
                .join("skills")
                .join(skill)
                .join("SKILL.md")
                .is_file(),
            "缺 skill {skill}"
        );
    }
    let names: Vec<_> = riot_kernel::skills::list(None)
        .into_iter()
        .filter(|s| s.source == "pack")
        .map(|s| s.name)
        .collect();
    assert_eq!(
        names.len(),
        pack.manifest.skills.len(),
        "实际发现 {names:?}"
    );

    packs::uninstall("doc-runtime").expect("卸载");
}

/// 构建脚本产出的 `pack.json` 必须能被 Rust 原样读懂。
///
/// 这两侧隔着一个 JSON 文件，字段名对不上不会有任何编译错误 —— 只会在用户
/// 装完之后表现为"MCP 没注册、PATH 没注入"，而 `pack.json` 看上去完全正常。
/// 所以直接拿构建脚本真的产物来解析，而不是手写一份测试用的。
#[test]
fn 构建脚本产出的_pack_json_能被解析() {
    let stage = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace 根")
        .join("dist/doc-pack");
    let Ok(entries) = std::fs::read_dir(&stage) else {
        eprintln!(
            "跳过：{} 不存在，先跑 scripts/build-doc-pack.mjs",
            stage.display()
        );
        return;
    };
    let mut checked = 0;
    for e in entries.filter_map(Result::ok) {
        let f = e.path().join("pack.json");
        if !f.is_file() {
            continue;
        }
        let raw = std::fs::read_to_string(&f).expect("读 pack.json");
        let m: packs::PackManifest = serde_json::from_str(&raw)
            .unwrap_or_else(|err| panic!("{} 解析失败：{err}", f.display()));

        assert!(!m.env.is_empty(), "{} 没有 env", f.display());
        for key in ["RUNTIME_BIN_DIR", "RUNTIME_NODE", "RUNTIME_NODE_MODULES"] {
            assert!(m.env.contains_key(key), "{} 缺 env.{key}", f.display());
        }
        assert!(
            !m.mcp_servers.is_empty(),
            "{} 没有 mcpServers —— camelCase 没对上时它会静默变成空数组",
            f.display()
        );
        assert!(
            !m.path_prepend.is_empty(),
            "{} 没有 pathPrepend —— 同上，静默为空",
            f.display()
        );
        checked += 1;
    }
    if checked == 0 {
        eprintln!("跳过：{} 下没有铺好的包", stage.display());
    }
}

/// 打出来的 tar.zst 必须能被宿主解开。
///
/// 构建脚本走的是 `zstd -19` 命令行（macOS）或 Node 的 zlib（Windows），
/// 宿主用的是 zstd crate。三方对同一个格式的理解不一致的话，用户会在
/// "下载完成"之后才看到解压失败。
#[test]
fn 打出来的压缩包宿主解得开() {
    let tmp = tempfile::tempdir().expect("临时目录");
    let src = tmp.path().join("src");
    build_pack(&src, "2.0.0");
    let archive = tmp.path().join("a.tar.zst");
    tarball(&src, &archive, "a-2.0.0");

    let file = std::fs::File::open(&archive).expect("打开压缩包");
    let dec = zstd::stream::read::Decoder::new(std::io::BufReader::new(file)).expect("建解码器");
    let mut tar = tar::Archive::new(dec);
    let out = tmp.path().join("out");
    tar.unpack(&out).expect("解压");
    assert!(out.join("a-2.0.0/pack.json").is_file());
}

/// 下载中途断线时,`packs::install` 必须自己带着半成品续上,不能把错误抛给
/// 用户再点一次。以前那条"先写一半再 append"的测试只验了文件系统,HTTP
/// Range、重试、校验失败会不会误删 `.part`,一条都没碰到。
#[tokio::test]
async fn 下载中途断线后安装仍能完成() {
    let _guard = SERIAL.lock().await;
    let tmp = tempfile::tempdir().expect("临时目录");
    unsafe { std::env::set_var("XDG_CONFIG_HOME", tmp.path()) };

    let src = tmp.path().join("src");
    build_pack(&src, "1.0.0");
    let archive = tmp.path().join("doc-runtime-1.0.0.tar.zst");
    let (size, sha256) = tarball(&src, &archive, "doc-runtime-1.0.0-any");
    let bytes = std::fs::read(&archive).expect("读压缩包");
    assert!(
        bytes.len() > 60,
        "压缩包太小,砍一截看不出续传。实际 {} 字节",
        bytes.len()
    );

    let mut server = Server::bind().await;
    let manifest = serde_json::json!({
        "packs": { "doc-runtime": { "version": "1.0.0", "platforms": { packs::platform_key(): {
            "url": format!("{}/doc-runtime.tar.zst", server.base),
            "sha256": sha256,
            "size": size,
        }}}}
    });
    unsafe {
        std::env::set_var(
            "RIOT_PACKS_MANIFEST_URL",
            format!("{}/packs.json", server.base),
        )
    };
    let drop_after = bytes.len() / 3;
    serve_flaky(
        server.listener.take().expect("listener 还在"),
        manifest.to_string().into_bytes(),
        bytes,
        drop_after,
    );

    let pack = packs::install("doc-runtime", |_| {})
        .await
        .expect("中途断线后应当自动续传并装完");
    assert_eq!(pack.manifest.version, "1.0.0");
    assert!(pack.root.join("pack.json").is_file());
}
