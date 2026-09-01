//! 解包、原子切换、装完自检。
//!
//! 豁免理由：宿主层。自检更是非用真进程不可 —— 它要回答的就是"这台机器上
//! 这个二进制到底跑不跑得起来"，注入 mock 会让这个检查完全失去意义。

#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

use riot_kernel::packs::{InstalledPack, PackManifest};

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("{0}")]
    Manifest(String),
    #[error("清单里没有能力包「{0}」")]
    NotFound(String),
    #[error("这个平台（{0}）暂时没有对应的能力包")]
    Unsupported(String),
    #[error("{0}")]
    Network(String),
    #[error(
        "下载的文件校验不通过（期望 {expected}，实际 {actual}）。可能下到了半截或被中间网络改写，重试一次试试。"
    )]
    Checksum { expected: String, actual: String },
    #[error("{0}失败：{1}")]
    Io(String, #[source] std::io::Error),
    #[error("能力包解压后结构不对：{0}")]
    Layout(String),
    #[error("能力包装好了但跑不起来：{0}")]
    SelfCheck(String),
    /// 解压 / 自检那个 blocking 任务本身没能跑完（panic 或 runtime 正在关）。
    /// 和"包坏了"分开，不然用户会去重下一个没问题的包。
    #[error("{0}")]
    Task(String),
}

impl serde::Serialize for InstallError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

/// 解压 tar.zst 并原子替换掉 `dest`。返回最终的包根目录。
///
/// 先解到同一个文件系统上的临时目录再 rename:直接往 `dest` 里解压的话,
/// 解到一半断电或用户退出,留下的是一个"看起来装好了、实际缺文件"的包 ——
/// 那比没装更糟,因为状态显示已安装,用起来却在各种地方随机报错。
pub fn unpack(archive: &Path, dest: &Path) -> Result<PathBuf, InstallError> {
    let parent = dest
        .parent()
        .ok_or_else(|| InstallError::Layout(format!("{} 没有父目录", dest.display())))?;
    std::fs::create_dir_all(parent).map_err(|e| InstallError::Io("建能力包目录".into(), e))?;

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let staging = parent.join(format!(
        ".staging-{}-{nonce}",
        dest.file_name().unwrap_or_default().to_string_lossy()
    ));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| InstallError::Io("建临时解压目录".into(), e))?;

    // 出任何岔子都要把临时目录收掉,否则失败几次就在用户盘上堆了几个 GB。
    let result = extract_into(archive, &staging).and_then(|()| swap_in(&staging, dest));
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result?;
    Ok(dest.to_path_buf())
}

fn extract_into(archive: &Path, staging: &Path) -> Result<(), InstallError> {
    let file = std::fs::File::open(archive)
        .map_err(|e| InstallError::Io("打开能力包压缩文件".into(), e))?;
    let decoder = zstd::stream::read::Decoder::new(std::io::BufReader::new(file))
        .map_err(|e| InstallError::Io("初始化 zstd 解压器".into(), e))?;
    let mut tar = tar::Archive::new(decoder);
    // 可执行位必须保留 —— 丢了的话 bin 里的 shim 全都跑不了。
    tar.set_preserve_permissions(true);
    tar.set_overwrite(true);

    // 逐条解而不是 `unpack()`，为的是把 AppleDouble 条目挡在外面。
    //
    // macOS 的 tar 会为带扩展属性的文件额外写一条 `._<原名>`，解包时由它自己
    // 还原成 xattr、不落地成文件。别的实现不认这套，会老老实实把它们当普通
    // 文件写出来 —— 于是 LibreOffice 的 program/ 和 share/registry/ 里凭空
    // 多出一堆 `._*`，它扫目录时把这些二进制垃圾当成配置去解析，直接抛
    // UNO 异常挂掉。报错里完全看不出和解压有关系。
    //
    // 构建脚本那边已经用 COPYFILE_DISABLE 从源头掐掉了，这里再挡一道：
    // 用户手上可能有老脚本打的包，而这个故障的排查成本高得离谱。
    for entry in tar
        .entries()
        .map_err(|e| InstallError::Io("读能力包条目".into(), e))?
    {
        let mut entry = entry.map_err(|e| InstallError::Io("读能力包条目".into(), e))?;
        let path = entry.path().map(|p| p.into_owned()).ok();
        let is_apple_double = path.as_ref().is_some_and(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("._"))
        });
        if is_apple_double {
            continue;
        }
        let mode = entry.header().mode().unwrap_or(0);
        let is_file = entry.header().entry_type().is_file();
        let unpacked = entry
            .unpack_in(staging)
            .map_err(|e| InstallError::Io("解压能力包".into(), e))?;

        #[cfg(unix)]
        if unpacked && is_file && mode & 0o6000 != 0 {
            // setuid / setgid 位一律掩掉。归档里的这两位会被原样保留（可执行
            // 位必须保留，见上面），于是"下载一个包"就变成了"在用户机器上放
            // 一个 setuid 程序"—— 归档是远端来的，包体内容变了签名不会变。
            // 掉的只是这两位，rwx 照旧。
            if let Some(p) = path.as_ref() {
                use std::os::unix::fs::PermissionsExt as _;
                let target = staging.join(p);
                let safe = std::fs::Permissions::from_mode(mode & 0o777);
                if let Err(e) = std::fs::set_permissions(&target, safe) {
                    return Err(InstallError::Io("收掉 setuid 位".into(), e));
                }
                tracing::warn!(path = %target.display(), mode = format!("{mode:o}"),
                    "能力包里带 setuid/setgid 位，已掩掉");
            }
        }
        #[cfg(not(unix))]
        let _ = (unpacked, is_file, mode);
    }
    Ok(())
}

/// 把解压结果搬到最终位置。
fn swap_in(staging: &Path, dest: &Path) -> Result<(), InstallError> {
    // 压缩包里裹了一层带版本和平台的顶层目录,剥掉它 —— 定位器按固定路径
    // 找包,路径里带版本号的话每次升级都要改代码。
    //
    // 判断"是不是只有一层"时要跳过点开头的东西。macOS 的 tar 会为带扩展属性
    // 的文件额外写一个 `._xxx` 的 AppleDouble 条目,而它自己列归档时又把这些
    // 藏起来 —— 归档看着只有一个顶层目录,别的 tar 实现解出来却是两个条目。
    // 不跳过的话这里会误判成"没有外层目录",然后在压缩包根下找 pack.json。
    let entries: Vec<_> = std::fs::read_dir(staging)
        .map_err(|e| InstallError::Io("读临时解压目录".into(), e))?
        .filter_map(Result::ok)
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .collect();
    let root = match entries.as_slice() {
        [only] if only.path().is_dir() => only.path(),
        _ => staging.to_path_buf(),
    };
    if !root.join("pack.json").exists() {
        // 把实际看到的东西带出来。只说"找不到 pack.json"的话，是"下错了文件"
        // 还是"压缩包多裹了一层目录"完全分不清，而这两者的排查方向相反。
        let found: Vec<_> = std::fs::read_dir(&root)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        return Err(InstallError::Layout(format!(
            "压缩包里找不到 pack.json，这多半不是一个 Riot 能力包。\
             解压后 {} 下有：{}",
            root.display(),
            if found.is_empty() {
                "（空）".to_owned()
            } else {
                found.join("、")
            }
        )));
    }

    // macOS 侧的隔离标记要在搬进去之前清,不然第一次执行会被 Gatekeeper 拦。
    // reqwest 下载本身不打这个标记,但用户手工放进来的包会带,便宜的保险。
    #[cfg(target_os = "macos")]
    clear_quarantine(&root);

    // rename 不能覆盖非空目录,所以先把旧的挪开。挪开这一步是原子的,
    // 中间态最多是"旧的还在 .old 里、新的还没就位",下次安装会清掉。
    let backup = dest.with_extension("old");
    let _ = std::fs::remove_dir_all(&backup);
    if dest.exists() {
        std::fs::rename(dest, &backup).map_err(|e| InstallError::Io("挪开旧版本".into(), e))?;
    }
    if let Err(e) = std::fs::rename(&root, dest) {
        // 新的没就位就把旧的放回去,不能让用户既失去旧版本又没装上新的。
        if backup.exists() {
            let _ = std::fs::rename(&backup, dest);
        }
        return Err(InstallError::Io("切换到新版本".into(), e));
    }
    let _ = std::fs::remove_dir_all(&backup);
    let _ = std::fs::remove_dir_all(staging);
    Ok(())
}

#[cfg(target_os = "macos")]
fn clear_quarantine(dir: &Path) {
    // 失败不致命:大多数情况下压根没有这个标记,xattr 会报"没有该属性"。
    // 真有标记又清不掉的话,后面的自检会以一个更清楚的错误暴露出来。
    match std::process::Command::new("/usr/bin/xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(dir)
        .output()
    {
        Ok(o) if !o.status.success() => {
            tracing::debug!(status = ?o.status, "清除 quarantine 标记未成功（通常是本来就没有）");
        }
        Err(e) => tracing::debug!(error = %e, "xattr 调不起来"),
        _ => {}
    }
}

/// 读 pack.json,并实际跑一遍包里的关键二进制。
///
/// 自检不是形式主义。这些二进制是从 Codex 运行时提取的,`soffice` 和
/// `python3.12` 只有 ad-hoc 签名 —— 理论上能在 Apple Silicon 上跑,但一旦
/// 因为签名、隔离标记或架构不匹配跑不起来,失败会发生在几天后用户让模型
/// "做个 PPT"的时候,现场是一条模型都看不懂的 Bash 报错。在安装时就跑一次,
/// 把问题摁在用户还知道自己刚点了"安装"的时刻。
pub fn finalize(root: &Path) -> Result<InstalledPack, InstallError> {
    let raw = std::fs::read_to_string(root.join("pack.json"))
        .map_err(|e| InstallError::Io("读 pack.json".into(), e))?;
    let manifest: PackManifest = serde_json::from_str(&raw)
        .map_err(|e| InstallError::Layout(format!("pack.json 解析失败：{e}")))?;

    let pack = InstalledPack {
        root: root.to_path_buf(),
        manifest,
    };

    for check in self_checks(&pack) {
        let program = pack.resolve(&check.0);
        if !program.exists() {
            return Err(InstallError::SelfCheck(format!(
                "{} 不存在",
                program.display()
            )));
        }
        let out = std::process::Command::new(&program)
            .args(&check.1)
            // 只给包内的 bin 和系统基础目录。用宿主完整 PATH 的话,开发机上
            // 的全局 python / node 会把包里缺失的东西兜住,自检就白做了。
            .env("PATH", minimal_path(&pack))
            .env_remove("PYTHONHOME")
            .env_remove("PYTHONPATH")
            .env_remove("VIRTUAL_ENV")
            .output();
        match out {
            Ok(o) if o.status.success() => {
                tracing::debug!(
                    program = %program.display(),
                    out = %String::from_utf8_lossy(&o.stdout).trim(),
                    "能力包自检通过"
                );
            }
            Ok(o) => {
                return Err(InstallError::SelfCheck(format!(
                    "{} 退出码 {:?}：{}",
                    program.display(),
                    o.status.code(),
                    String::from_utf8_lossy(&o.stderr).trim(),
                )));
            }
            Err(e) => {
                return Err(InstallError::SelfCheck(format!(
                    "{} 起不来：{e}",
                    program.display()
                )));
            }
        }
    }

    Ok(pack)
}

/// pack.json 里声明的自检项;没声明就退回到"跑一遍 PATH 目录里的每个 shim"
/// 是不可行的(参数各不相同),所以老包按空处理 —— 不自检总好过误报失败。
fn self_checks(pack: &InstalledPack) -> Vec<(String, Vec<String>)> {
    #[derive(serde::Deserialize)]
    struct Check {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    }
    let raw = std::fs::read_to_string(pack.root.join("pack.json")).unwrap_or_default();
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
    let Some(list) = value.get("selfCheck").and_then(|v| v.as_array()) else {
        return vec![];
    };
    list.iter()
        .filter_map(|v| serde_json::from_value::<Check>(v.clone()).ok())
        .map(|c| (c.command, c.args))
        .collect()
}

fn minimal_path(pack: &InstalledPack) -> std::ffi::OsString {
    let mut dirs = pack.path_dirs();
    if cfg!(windows) {
        dirs.push(PathBuf::from(r"C:\Windows\system32"));
        dirs.push(PathBuf::from(r"C:\Windows"));
    } else {
        dirs.extend([
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
            PathBuf::from("/usr/sbin"),
            PathBuf::from("/sbin"),
        ]);
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_pack(dir: &Path, version: &str) {
        std::fs::create_dir_all(dir).expect("建目录");
        std::fs::write(
            dir.join("pack.json"),
            serde_json::json!({
                "name": "doc-runtime",
                "version": version,
                "platform": "darwin-arm64",
                "env": {},
                "pathPrepend": [],
                "mcpServers": [],
                "skills": [],
            })
            .to_string(),
        )
        .expect("写 pack.json");
    }

    #[test]
    fn 顶层目录会被剥掉() {
        let tmp = tempfile::tempdir().expect("临时目录");
        let staging = tmp.path().join("staging");
        write_pack(&staging.join("doc-runtime-0.1.0-darwin-arm64"), "0.1.0");
        let dest = tmp.path().join("doc-runtime");

        swap_in(&staging, &dest).expect("切换");

        assert!(
            dest.join("pack.json").exists(),
            "pack.json 应该直接在包根下"
        );
        assert!(!staging.exists(), "临时目录应被清掉");
    }

    #[test]
    fn 升级会替换掉旧版本且不留备份() {
        let tmp = tempfile::tempdir().expect("临时目录");
        let dest = tmp.path().join("doc-runtime");
        write_pack(&dest, "0.1.0");
        std::fs::write(dest.join("stale.txt"), "旧版本残留").expect("写残留文件");

        let staging = tmp.path().join("staging");
        write_pack(&staging.join("inner"), "0.2.0");
        swap_in(&staging, &dest).expect("切换");

        let raw = std::fs::read_to_string(dest.join("pack.json")).expect("读 pack.json");
        assert!(raw.contains("0.2.0"), "应该是新版本");
        assert!(
            !dest.join("stale.txt").exists(),
            "旧版本的文件必须消失，否则升级会留下互相矛盾的两代文件"
        );
        assert!(!dest.with_extension("old").exists(), "备份目录应被清掉");
    }

    /// AppleDouble 条目不能落地成文件。
    ///
    /// 真的踩过，而且现场毫无线索：LibreOffice 会扫自己的 `program/` 和
    /// `share/registry/`，多出来的 `._*` 被当成配置去解析，抛 UNO 异常直接
    /// abort，报错里一个字都没提解压。
    #[test]
    fn apple_double_条目不落地并且可执行位不丢() {
        let tmp = tempfile::tempdir().expect("临时目录");
        let archive = tmp.path().join("a.tar.zst");

        {
            let f = std::fs::File::create(&archive).expect("建压缩文件");
            let enc = zstd::stream::write::Encoder::new(f, 1).expect("建编码器");
            let mut b = tar::Builder::new(enc);

            let mut shim = tar::Header::new_gnu();
            shim.set_size(3);
            shim.set_mode(0o755);
            shim.set_cksum();
            b.append_data(&mut shim.clone(), "pack/bin/soffice", &b"#!\n"[..])
                .expect("写 shim");

            let mut junk = tar::Header::new_gnu();
            junk.set_size(4);
            junk.set_mode(0o644);
            junk.set_cksum();
            b.append_data(&mut junk, "pack/bin/._soffice", &b"\x00\x05\x16\x07"[..])
                .expect("写 AppleDouble");

            b.into_inner()
                .expect("收尾 tar")
                .finish()
                .expect("收尾 zstd");
        }

        let staging = tmp.path().join("staging");
        std::fs::create_dir_all(&staging).expect("建目录");
        extract_into(&archive, &staging).expect("解压");

        assert!(
            staging.join("pack/bin/soffice").is_file(),
            "正常文件要解出来"
        );
        assert!(
            !staging.join("pack/bin/._soffice").exists(),
            "AppleDouble 不能落地"
        );
        // 可执行位只在 unix 上有意义，Windows 的 Permissions 没有 mode。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(staging.join("pack/bin/soffice"))
                .expect("读元信息")
                .permissions()
                .mode();
            assert!(mode & 0o111 != 0, "逐条解压不能丢可执行位，mode={mode:o}");
        }
    }

    /// macOS 的 tar 会在顶层多写一个 `._` 开头的 AppleDouble 条目，而它自己
    /// `tar -tf` 时把这些藏起来 —— 归档看着只有一个顶层目录，用别的 tar 解出来
    /// 却是两个。真的踩过：构建脚本产出的包在宿主这边报"找不到 pack.json"。
    #[test]
    fn apple_double_残留不影响剥外层目录() {
        let tmp = tempfile::tempdir().expect("临时目录");
        let staging = tmp.path().join("staging");
        write_pack(&staging.join("doc-runtime-0.1.0-darwin-arm64"), "0.1.0");
        std::fs::write(
            staging.join("._doc-runtime-0.1.0-darwin-arm64"),
            b"\x00\x05\x16\x07",
        )
        .expect("写 AppleDouble 残留");

        let dest = tmp.path().join("doc-runtime");
        swap_in(&staging, &dest).expect("切换");
        assert!(dest.join("pack.json").exists(), "外层目录应该照样被剥掉");
    }

    /// 归档里的 setuid 位不能落到用户盘上。
    ///
    /// 可执行位必须保留（不然 bin 里的 shim 全跑不了），而 tar 的
    /// `set_preserve_permissions` 是一刀切的 —— setuid / setgid 会跟着一起
    /// 进来。包体是从远端下的，"装一个能力包"不该顺带在用户机器上放一个
    /// setuid 程序。
    #[cfg(unix)]
    #[test]
    fn setuid_位不会跟着解压落地() {
        use std::os::unix::fs::PermissionsExt as _;

        let tmp = tempfile::tempdir().expect("临时目录");
        let archive = tmp.path().join("a.tar.zst");
        {
            let f = std::fs::File::create(&archive).expect("建压缩文件");
            let enc = zstd::stream::write::Encoder::new(f, 1).expect("建编码器");
            let mut b = tar::Builder::new(enc);
            let mut h = tar::Header::new_gnu();
            h.set_size(3);
            h.set_mode(0o4755);
            h.set_cksum();
            b.append_data(&mut h, "pack/bin/evil", &b"#!\n"[..])
                .expect("写 setuid 条目");
            b.into_inner()
                .expect("收尾 tar")
                .finish()
                .expect("收尾 zstd");
        }

        let staging = tmp.path().join("staging");
        std::fs::create_dir_all(&staging).expect("建目录");
        extract_into(&archive, &staging).expect("解压");

        let mode = std::fs::metadata(staging.join("pack/bin/evil"))
            .expect("读元信息")
            .permissions()
            .mode();
        assert_eq!(mode & 0o6000, 0, "setuid/setgid 必须被掩掉，mode={mode:o}");
        assert!(mode & 0o111 != 0, "可执行位不能跟着一起掉，mode={mode:o}");
    }

    #[test]
    fn 缺_pack_json_直接拒绝() {
        let tmp = tempfile::tempdir().expect("临时目录");
        let staging = tmp.path().join("staging");
        std::fs::create_dir_all(staging.join("inner")).expect("建目录");
        let err = swap_in(&staging, &tmp.path().join("doc-runtime")).expect_err("应该失败");
        assert!(matches!(err, InstallError::Layout(_)), "实际是 {err:?}");
    }
}
