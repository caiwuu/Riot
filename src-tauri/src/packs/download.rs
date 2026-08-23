//! 能力包下载。流式落盘、断点续传、边下边算 sha256。
//!
//! 包有几百 MB,而目标用户里有相当一部分网络不稳。三个设计后果:
//!   - 不能整包读进内存再写盘
//!   - 断了要能接着下,不能从头再来
//!   - 拼出来的东西必须能证明和发布时是同一份
//!
//! 续传不是"用户再点一次才接上"。弱网会把一条 TCP 连接掐好几次,一次
//! `fetch` 里就要自己带着 `.part` 重发 Range,直到传完或连续几次完全没进展。
//! 包体走 GitHub Releases,每次都是 302 到 CDN —— Range 必须打在最终地址上,
//! 交给 reqwest 自动跟重定向的话,有的跳转会把 Range 丢掉,半成品就被 200
//! 整包覆盖,看起来像"断点续传没生效"。
//!
//! 豁免理由：宿主层，真的在下载文件。进度限流用的是真时钟 —— 它节流的是
//! 前端重绘频率，注入的假时钟对这件事没有意义。

#![allow(clippy::disallowed_methods)]

use std::io::Write as _;
use std::path::Path;
use std::time::Duration;

use futures::StreamExt as _;
use reqwest::header::{ACCEPT_ENCODING, CONTENT_RANGE, LOCATION, RANGE};
use sha2::{Digest as _, Sha256};

use super::{InstallError, PackProgress};

/// 连续这么多次完全没把半成品变大,才认输让用户再点。
///
/// 次数按"没进展"计,不按连接次数计:弱网下 200MB 可能要断十几次,每次
/// 都写下一段就该继续,不能用一个总次数把已经下到一半的进度扔掉。
const MAX_STALLS: u32 = 5;

/// 两个数据块之间最多等这么久。连着挂着不报错,前端进度条停住,用户以为
/// 还在下,其实 TCP 已经半开了;设了这个,卡住会变成一次可续传的失败。
const IDLE_TIMEOUT: Duration = Duration::from_secs(45);

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
    let client = download_client()?;

    let mut stalls = 0;
    let mut best = part_len(&part);

    loop {
        let before = part_len(&part);
        match download_attempt(
            &client,
            url,
            dest,
            &part,
            expected_size,
            expected_sha256,
            progress,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(e) if !retryable(&e) => return Err(e),
            Err(e) => {
                let after = part_len(&part);
                if after > best {
                    best = after;
                    stalls = 0;
                } else {
                    stalls += 1;
                }
                tracing::warn!(
                    error = %e,
                    saved = after,
                    stalls,
                    "能力包下载中断，从断点重试"
                );
                if stalls >= MAX_STALLS {
                    return Err(InstallError::Network(format!(
                        "{e} 已保存 {}，再点一次会从断点继续。",
                        pretty_bytes(after)
                    )));
                }
                let delay = if after > before {
                    Duration::from_millis(if cfg!(test) { 10 } else { 400 })
                } else {
                    backoff(stalls)
                };
                tokio::time::sleep(delay).await;
            }
        }
    }
}

fn download_client() -> Result<reqwest::Client, InstallError> {
    reqwest::Client::builder()
        // 不设总超时:几百 MB 在慢网络上可能要跑很久,一刀切的总超时会把
        // 本来能成功的下载砍掉。用连接超时挡住"连不上"的情况就够了。
        .connect_timeout(Duration::from_secs(20))
        // 半开连接靠这个被内核发现,否则只能干等到 IDLE_TIMEOUT。
        .tcp_keepalive(Duration::from_secs(30))
        // 刚失败的那条连接不要进池。弱网里复用半死连接,下一次立刻再断,
        // 看起来像续传怎么都接不上。
        .pool_max_idle_per_host(0)
        // HTTP/2 在弱网上动不动 RST stream,reqwest 一律报
        // "error decoding response body"。大文件走 HTTP/1.1 稳得多。
        .http1_only()
        // 自己跟重定向,好把 Range 打在 CDN 的最终地址上。
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| InstallError::Network(e.to_string()))
}

async fn download_attempt(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    part: &Path,
    expected_size: u64,
    expected_sha256: &str,
    progress: &(impl Fn(PackProgress) + Send + Sync),
) -> Result<(), InstallError> {
    let mut done = part_len(part);
    // 半成品比目标还大,只可能是上游换了内容,续传没有意义。
    if done > expected_size {
        let _ = std::fs::remove_file(part);
        done = 0;
    }

    // 上次传到完整长度但没来得及改名(进程被杀、校验前崩溃)。再发
    // `Range: bytes={size}-` 会吃 416,被当成下载失败。
    if done == expected_size && done > 0 {
        return finalize_part(part, dest, expected_size, expected_sha256, progress);
    }

    let res = send_get(client, url, done).await?;

    if res.status() == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        if done == expected_size && done > 0 {
            return finalize_part(part, dest, expected_size, expected_sha256, progress);
        }
        // 偏移对不上,半成品不可信。删掉让下一轮从头来,免得对着 416 空转。
        let _ = std::fs::remove_file(part);
        return Err(InstallError::Network(
            "服务端拒绝续传（416），将重新下载".into(),
        ));
    }
    if !res.status().is_success() {
        return Err(InstallError::Network(format!(
            "下载返回 {}：{url}",
            res.status()
        )));
    }

    let content_len = res.content_length();
    let mut resuming = false;
    if done > 0 {
        if res.status() == reqwest::StatusCode::PARTIAL_CONTENT {
            match content_range_start(res.headers()) {
                Some(start) if start == done => resuming = true,
                Some(0) => done = 0,
                Some(start) => {
                    return Err(InstallError::Network(format!(
                        "续传起点对不上（本地 {done}，服务端 {start}）"
                    )));
                }
                // 没给 Content-Range 的 206:按声明相信它,错了校验会拦住。
                None => resuming = true,
            }
        } else if content_len == Some(expected_size.saturating_sub(done)) {
            // 少数 CDN 不回 206,但 body 长度刚好是剩下那截 —— 接上,别整段重来。
            resuming = true;
        } else {
            // 服务端不认 Range,或者跳转把 Range 吃了,回了完整 200。
            // 必须丢掉半成品重新写,否则等于把新数据追加在旧数据后面。
            done = 0;
        }
    }

    let total = if resuming {
        done + content_len.unwrap_or(expected_size.saturating_sub(done))
    } else {
        content_len.unwrap_or(expected_size)
    };

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(resuming)
        .truncate(!resuming)
        .open(part)
        .map_err(|e| InstallError::Io("打开下载临时文件".into(), e))?;

    let mut received = done;
    let mut last_report = std::time::Instant::now();
    progress(PackProgress::Downloading { received, total });

    let mut stream = res.bytes_stream();
    loop {
        let next = tokio::time::timeout(IDLE_TIMEOUT, stream.next()).await;
        let chunk = match next {
            Err(_) => {
                let _ = file.flush();
                return Err(InstallError::Network("下载中断：等待数据超时".into()));
            }
            Ok(None) => break,
            Ok(Some(Err(e))) => {
                let _ = file.flush();
                return Err(InstallError::Network(format!(
                    "下载中断：{}",
                    explain_body(&e)
                )));
            }
            Ok(Some(Ok(chunk))) => chunk,
        };
        file.write_all(&chunk)
            .map_err(|e| InstallError::Io("写下载临时文件".into(), e))?;
        received += chunk.len() as u64;
        // 限流:一个块可能只有几 KB,每块发一次事件会把 IPC 打满,
        // 前端忙着重绘进度条反而更慢。
        if last_report.elapsed() >= Duration::from_millis(200) {
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

    finalize_part(part, dest, expected_size, expected_sha256, progress)
}

/// 自己跟 3xx,把 Range 带到最终 URL。
///
/// 包体在 GitHub Releases:`/releases/download/...` 一定 302 到
/// `release-assets.githubusercontent.com`。Range 打在第一跳上再交给
/// reqwest 自动跟的话,有的跳转会剥掉这个头,CDN 回 200 整包,调用方
/// 只能 truncate 重来 —— 弱网用户每断一次就从头下。
async fn send_get(
    client: &reqwest::Client,
    url: &str,
    done: u64,
) -> Result<reqwest::Response, InstallError> {
    let mut current = url.to_owned();
    for _ in 0..10 {
        let mut req = client.get(&current).header(ACCEPT_ENCODING, "identity");
        if done > 0 {
            req = req.header(RANGE, format!("bytes={done}-"));
        }
        let res = req
            .send()
            .await
            .map_err(|e| InstallError::Network(format!("下载失败：{e}")))?;
        if res.status().is_redirection() {
            let loc = res
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| InstallError::Network("重定向没有 Location".into()))?;
            current = join_url(&current, loc)?;
            continue;
        }
        return Ok(res);
    }
    Err(InstallError::Network("重定向次数过多".into()))
}

fn finalize_part(
    part: &Path,
    dest: &Path,
    expected_size: u64,
    expected_sha256: &str,
    progress: &(impl Fn(PackProgress) + Send + Sync),
) -> Result<(), InstallError> {
    // 校验放在续传拼接之后:每一段单独看都可能是好的,拼错了才出问题。
    progress(PackProgress::Verifying);
    let actual = sha256_file(part)?;
    if actual == expected_sha256 {
        std::fs::rename(part, dest).map_err(|e| InstallError::Io("重命名下载文件".into(), e))?;
        return Ok(());
    }
    let size = part_len(part);
    // 连接被干净关掉时,HTTP/1.1 可能把半截 body 当成完整响应。这时校验
    // 必然失败 —— 但半成品是好的,删掉等于让用户从零再来。留下,下一轮 Range。
    if size < expected_size {
        return Err(InstallError::Network(format!(
            "下载不完整（{size}/{expected_size}），已保存进度。"
        )));
    }
    let _ = std::fs::remove_file(part);
    Err(InstallError::Checksum {
        expected: expected_sha256.to_owned(),
        actual,
    })
}

fn content_range_start(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let raw = headers.get(CONTENT_RANGE)?.to_str().ok()?;
    // `bytes 20000-29999/30000`
    let rest = raw.trim().strip_prefix("bytes")?.trim();
    let (range, _) = rest.split_once('/')?;
    let (start, _) = range.split_once('-')?;
    start.trim().parse().ok()
}

fn join_url(base: &str, location: &str) -> Result<String, InstallError> {
    if location.starts_with("https://") || location.starts_with("http://") {
        return Ok(location.to_owned());
    }
    reqwest::Url::parse(base)
        .and_then(|u| u.join(location))
        .map(|u| u.to_string())
        .map_err(|e| InstallError::Network(format!("重定向地址无效：{e}")))
}

fn retryable(err: &InstallError) -> bool {
    matches!(err, InstallError::Network(_))
}

fn backoff(stalls: u32) -> Duration {
    let unit_ms: u64 = if cfg!(test) { 20 } else { 1000 };
    Duration::from_millis(unit_ms.saturating_mul(1u64 << stalls.min(4)))
}

fn part_len(path: &Path) -> u64 {
    path.metadata().map(|m| m.len()).unwrap_or(0)
}

fn pretty_bytes(n: u64) -> String {
    if n >= 1_048_576 {
        format!("{:.1} MB", n as f64 / 1_048_576.0)
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

/// reqwest 开了 gzip 特性之后,读 body 失败一律叫 "error decoding response body",
/// 用户看着像文件坏了。其实就是网断了。
fn explain_body(err: &reqwest::Error) -> String {
    let raw = err.to_string();
    if raw.contains("decode") || raw.contains("connection") {
        format!("网络中断（{raw}）")
    } else {
        raw
    }
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
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

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

    fn hex_sha256(bytes: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(bytes);
        format!("{:x}", h.finalize())
    }

    fn sample_body() -> Vec<u8> {
        (0..64_u32 * 1024).map(|i| (i % 251) as u8).collect()
    }

    #[derive(Clone, Copy)]
    enum Mode {
        /// 第一次(无 Range)只给前 `after` 字节就关连接,Content-Length 仍是全长。
        /// 之后认 Range,回 206。
        Cut { after: usize },
        /// 第一次给一个"完整"的短响应(Content-Length = 短 body)。之后认 Range。
        Short { after: usize },
        /// `/file` 302 到 `/real`,`/real` 按 Cut 来。
        Redirect { after: usize },
    }

    #[derive(Debug, Clone)]
    struct Hit {
        path: String,
        range_start: Option<u64>,
        accept_encoding: Option<String>,
    }

    async fn spawn_server(body: Vec<u8>, mode: Mode) -> (String, Arc<Mutex<Vec<Hit>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("监听");
        let base = format!("http://{}", listener.local_addr().expect("地址"));
        let hits = Arc::new(Mutex::new(Vec::new()));
        let recorded = hits.clone();
        let origin = base.clone();
        tokio::spawn(async move {
            let mut first_drop_done = false;
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let req = read_headers(&mut sock).await;
                let (path, range_start, accept_encoding) = parse_req(&req);
                recorded.lock().expect("锁").push(Hit {
                    path: path.clone(),
                    range_start,
                    accept_encoding,
                });

                let after = match mode {
                    Mode::Cut { after } | Mode::Short { after } | Mode::Redirect { after } => after,
                };

                if matches!(mode, Mode::Redirect { .. }) && path == "/file" {
                    let loc = format!("{origin}/real");
                    let head = format!(
                        "HTTP/1.1 302 Found\r\nLocation: {loc}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.shutdown().await;
                    continue;
                }

                let start = range_start.unwrap_or(0) as usize;
                let start = start.min(body.len());
                let should_drop = !first_drop_done && start == 0;
                if should_drop {
                    first_drop_done = true;
                    match mode {
                        Mode::Short { .. } => {
                            let slice = &body[..after.min(body.len())];
                            let head = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                slice.len()
                            );
                            let _ = sock.write_all(head.as_bytes()).await;
                            let _ = sock.write_all(slice).await;
                            let _ = sock.shutdown().await;
                        }
                        Mode::Cut { .. } | Mode::Redirect { .. } => {
                            let n = after.min(body.len());
                            let head = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            let _ = sock.write_all(head.as_bytes()).await;
                            let _ = sock.write_all(&body[..n]).await;
                            let _ = sock.shutdown().await;
                        }
                    }
                    continue;
                }

                if start > 0 {
                    let end = body.len().saturating_sub(1);
                    let slice = &body[start..];
                    let head = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len(),
                        slice.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(slice).await;
                } else {
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(&body).await;
                }
                let _ = sock.shutdown().await;
            }
        });
        (base, hits)
    }

    async fn read_headers(sock: &mut tokio::net::TcpStream) -> String {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1024];
        loop {
            let n = sock.read(&mut tmp).await.unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 16 * 1024 {
                break;
            }
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    fn parse_req(req: &str) -> (String, Option<u64>, Option<String>) {
        let mut lines = req.lines();
        let path = lines
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .unwrap_or("/")
            .split('?')
            .next()
            .unwrap_or("/")
            .to_owned();
        let mut range_start = None;
        let mut accept_encoding = None;
        for line in lines {
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            let k = k.trim();
            let v = v.trim();
            if k.eq_ignore_ascii_case("Range") {
                if let Some(rest) = v.strip_prefix("bytes=")
                    && let Some((s, _)) = rest.split_once('-')
                {
                    range_start = s.trim().parse().ok();
                }
            } else if k.eq_ignore_ascii_case("Accept-Encoding") {
                accept_encoding = Some(v.to_owned());
            }
        }
        (path, range_start, accept_encoding)
    }

    async fn fetch_ok(url: &str, dest: &Path, body: &[u8]) {
        fetch(url, dest, body.len() as u64, &hex_sha256(body), &|_| {})
            .await
            .unwrap_or_else(|e| panic!("下载应当成功：{e}"));
        assert_eq!(
            std::fs::read(dest).expect("读成品"),
            body,
            "拼出来的文件不对"
        );
    }

    #[tokio::test]
    async fn 中途断线会自动续传拼出完整文件() {
        let body = sample_body();
        let (base, hits) = spawn_server(body.clone(), Mode::Cut { after: 20_000 }).await;
        let dir = tempfile::tempdir().expect("临时目录");
        let dest = dir.path().join("pack.bin");
        fetch_ok(&format!("{base}/file"), &dest, &body).await;

        let hits = hits.lock().expect("锁").clone();
        assert!(
            hits.iter().any(|h| h.range_start == Some(20_000)),
            "第二跳必须带 Range: bytes=20000-，实际 {hits:?}"
        );
        assert!(
            hits.iter()
                .any(|h| h.accept_encoding.as_deref() == Some("identity")),
            "必须关掉 HTTP 压缩,否则 Range 的偏移是解压后的、对不上线上字节。实际 {hits:?}"
        );
    }

    #[tokio::test]
    async fn 短响应被当成完整时也要留下半成品再续() {
        let body = sample_body();
        let (base, hits) = spawn_server(body.clone(), Mode::Short { after: 20_000 }).await;
        let dir = tempfile::tempdir().expect("临时目录");
        let dest = dir.path().join("pack.bin");
        fetch_ok(&format!("{base}/file"), &dest, &body).await;

        let hits = hits.lock().expect("锁").clone();
        assert!(
            hits.iter().any(|h| h.range_start == Some(20_000)),
            "校验失败但不能删半成品。实际请求 {hits:?}"
        );
    }

    #[tokio::test]
    async fn 重定向之后_range_打在最终地址上() {
        let body = sample_body();
        let (base, hits) = spawn_server(body.clone(), Mode::Redirect { after: 20_000 }).await;
        let dir = tempfile::tempdir().expect("临时目录");
        let dest = dir.path().join("pack.bin");
        fetch_ok(&format!("{base}/file"), &dest, &body).await;

        let hits = hits.lock().expect("锁").clone();
        assert!(
            hits.iter()
                .any(|h| h.path == "/real" && h.range_start == Some(20_000)),
            "Range 必须打在 302 之后的 /real 上,不能停在 /file。实际 {hits:?}"
        );
    }

    #[tokio::test]
    async fn 半成品已经完整且校验通过就不再请求() {
        let body = b"hello-pack";
        let dir = tempfile::tempdir().expect("临时目录");
        let dest = dir.path().join("x.bin");
        std::fs::write(dest.with_extension("part"), body).expect("写半成品");
        fetch(
            "http://127.0.0.1:1/does-not-matter",
            &dest,
            body.len() as u64,
            &hex_sha256(body),
            &|_| {},
        )
        .await
        .expect("不该去联网");
        assert_eq!(std::fs::read(&dest).expect("读成品"), body);
    }
}
