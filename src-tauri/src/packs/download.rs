//! 能力包下载。流式落盘、断点续传、边下边算 sha256。
//!
//! 包有几百 MB,而目标用户里有相当一部分网络不稳。三个设计后果:
//!   - 不能整包读进内存再写盘
//!   - 断了要能接着下,不能从头再来
//!   - 拼出来的东西必须能证明和发布时是同一份
//!
//! 豁免理由：宿主层，真的在下载文件。进度限流用的是真时钟 —— 它节流的是
//! 前端重绘频率，注入的假时钟对这件事没有意义。

#![allow(clippy::disallowed_methods)]

use std::io::Write as _;
use std::path::Path;

use futures::StreamExt as _;
use sha2::{Digest as _, Sha256};

use super::{InstallError, PackProgress};

/// 下载到 `dest`,校验 sha256。`dest` 已经存在且校验通过就直接返回。
pub async fn fetch(
    url: &str,
    dest: &Path,
    expected_size: u64,
    expected_sha256: &str,
    progress: &(impl Fn(PackProgress) + Send + Sync),
) -> Result<(), InstallError> {
    // 上次装到一半留下的完整包,校验过就不用再下。
    if dest.exists() {
        progress(PackProgress::Verifying);
        if sha256_file(dest)? == expected_sha256 {
            tracing::info!(path = %dest.display(), "能力包已在缓存里且校验通过");
            return Ok(());
        }
        // 校验不过说明是坏的或旧版本,删掉重下 —— 对它做续传会拼出一堆垃圾。
        let _ = std::fs::remove_file(dest);
    }

    let part = dest.with_extension("part");
    let mut done = part.metadata().map(|m| m.len()).unwrap_or(0);
    // 半成品比目标还大,只可能是上游换了内容,续传没有意义。
    if done > expected_size {
        let _ = std::fs::remove_file(&part);
        done = 0;
    }

    let client = reqwest::Client::builder()
        // 不设总超时:几百 MB 在慢网络上可能要跑很久,一刀切的总超时会把
        // 本来能成功的下载砍掉。用连接超时挡住"连不上"的情况就够了。
        .connect_timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| InstallError::Network(e.to_string()))?;

    let mut req = client.get(url);
    if done > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={done}-"));
    }
    let res = req
        .send()
        .await
        .map_err(|e| InstallError::Network(format!("下载失败：{e}")))?;

    // 服务端不认 Range 就会回 200 并从头给,这时必须丢掉半成品重新写,
    // 否则等于把新数据追加在旧数据后面。
    let resuming = res.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    if !res.status().is_success() {
        return Err(InstallError::Network(format!(
            "下载返回 {}：{url}",
            res.status()
        )));
    }
    if !resuming {
        done = 0;
    }

    let total = if resuming {
        done + res.content_length().unwrap_or(expected_size - done)
    } else {
        res.content_length().unwrap_or(expected_size)
    };

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(resuming)
        .truncate(!resuming)
        .open(&part)
        .map_err(|e| InstallError::Io("打开下载临时文件".into(), e))?;

    let mut received = done;
    let mut last_report = std::time::Instant::now();
    progress(PackProgress::Downloading { received, total });

    let mut stream = res.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| InstallError::Network(format!("下载中断：{e}")))?;
        file.write_all(&chunk)
            .map_err(|e| InstallError::Io("写下载临时文件".into(), e))?;
        received += chunk.len() as u64;
        // 限流:一个块可能只有几 KB,每块发一次事件会把 IPC 打满,
        // 前端忙着重绘进度条反而更慢。
        if last_report.elapsed() >= std::time::Duration::from_millis(200) {
            last_report = std::time::Instant::now();
            progress(PackProgress::Downloading { received, total });
        }
    }
    file.flush()
        .map_err(|e| InstallError::Io("flush 下载临时文件".into(), e))?;
    drop(file);
    progress(PackProgress::Downloading {
        received,
        total: received.max(total),
    });

    // 校验放在续传拼接之后:每一段单独看都可能是好的,拼错了才出问题。
    progress(PackProgress::Verifying);
    let actual = sha256_file(&part)?;
    if actual != expected_sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(InstallError::Checksum {
            expected: expected_sha256.to_owned(),
            actual,
        });
    }

    std::fs::rename(&part, dest).map_err(|e| InstallError::Io("重命名下载文件".into(), e))?;
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, InstallError> {
    let mut file =
        std::fs::File::open(path).map_err(|e| InstallError::Io("打开文件算校验和".into(), e))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)
        .map_err(|e| InstallError::Io("读文件算校验和".into(), e))?;
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 校验和是标准_sha256() {
        let dir = tempfile::tempdir().expect("临时目录");
        let f = dir.path().join("x");
        std::fs::write(&f, b"abc").expect("写文件");
        assert_eq!(
            sha256_file(&f).expect("算校验和"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
