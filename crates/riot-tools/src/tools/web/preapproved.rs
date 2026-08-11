//! 免确认的域名白名单。
//!
//! 抓取任意站点默认都要用户点确认。但如果连查个 `docs.rs` 都要点一次，
//! 用户会在第三次的时候直接开"全部允许"，那比这份白名单危险得多。
//!
//! # 边界
//!
//! `[约束]` 这份名单**只对 GET 抓取生效**，不能被 Bash 的网络策略或沙箱
//! 规则复用。名单里像 `huggingface.co`、`kaggle.com` 这些站点是允许上传的，
//! 把它们加进通用网络白名单等于开了一条数据外带通道。
//!
//! 名单本身按 Claude Code 的 `PREAPPROVED_HOSTS` 整理，补上了 Rust 生态
//! 和几个中文技术站。

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// 白名单条目。带 `/` 的是路径前缀限定，如 `github.com/rust-lang`。
const ENTRIES: &[&str] = &[
    // ── 语言官方文档 ──
    "doc.rust-lang.org",
    "docs.rs",
    "crates.io",
    "rust-lang.github.io",
    "docs.python.org",
    "pkg.go.dev",
    "go.dev",
    "developer.mozilla.org",
    "en.cppreference.com",
    "docs.oracle.com",
    "learn.microsoft.com",
    "www.php.net",
    "docs.swift.org",
    "kotlinlang.org",
    "ruby-doc.org",
    "www.typescriptlang.org",
    // ── 前端框架 ──
    "react.dev",
    "vuejs.org",
    "angular.dev",
    "svelte.dev",
    "nextjs.org",
    "nodejs.org",
    "bun.sh",
    "deno.land",
    "docs.deno.com",
    "vitejs.dev",
    "vite.dev",
    "tailwindcss.com",
    "getbootstrap.com",
    "redux.js.org",
    "webpack.js.org",
    "jestjs.io",
    "vitest.dev",
    "reactrouter.com",
    "expressjs.com",
    "d3js.org",
    "threejs.org",
    // ── Rust 生态常用 ──
    "tokio.rs",
    "serde.rs",
    "tauri.app",
    "v2.tauri.app",
    "rust-analyzer.github.io",
    // ── Python 生态 ──
    "docs.djangoproject.com",
    "flask.palletsprojects.com",
    "fastapi.tiangolo.com",
    "pandas.pydata.org",
    "numpy.org",
    "www.tensorflow.org",
    "pytorch.org",
    "scikit-learn.org",
    "matplotlib.org",
    "requests.readthedocs.io",
    "jupyter.org",
    "pypi.org",
    // ── JVM / .NET / PHP ──
    "docs.spring.io",
    "hibernate.org",
    "gradle.org",
    "maven.apache.org",
    "laravel.com",
    "symfony.com",
    "dotnet.microsoft.com",
    "asp.net",
    // ── 移动端 ──
    "reactnative.dev",
    "docs.flutter.dev",
    "developer.apple.com",
    "developer.android.com",
    // ── 数据库 ──
    "www.postgresql.org",
    "dev.mysql.com",
    "www.sqlite.org",
    "redis.io",
    "www.mongodb.com",
    "graphql.org",
    "www.prisma.io",
    // ── 云与运维 ──
    "docs.aws.amazon.com",
    "cloud.google.com",
    "kubernetes.io",
    "docs.docker.com",
    "developer.hashicorp.com",
    "docs.github.com",
    "vercel.com/docs",
    "docs.netlify.com",
    // ── AI / MCP ──
    "modelcontextprotocol.io",
    "platform.openai.com",
    "docs.anthropic.com",
    "huggingface.co",
    // ── 工具 ──
    "git-scm.com",
    "nginx.org",
    "httpd.apache.org",
    "man7.org",
    "stackoverflow.com",
    // ── 中文技术站 ──
    "developer.aliyun.com",
    "cloud.tencent.com",
    "www.runoob.com",
];

struct Index {
    hosts: HashSet<&'static str>,
    /// host → 允许的路径前缀。
    paths: HashMap<&'static str, Vec<&'static str>>,
}

fn index() -> &'static Index {
    static IDX: OnceLock<Index> = OnceLock::new();
    IDX.get_or_init(|| {
        let mut hosts = HashSet::new();
        let mut paths: HashMap<&'static str, Vec<&'static str>> = HashMap::new();
        for e in ENTRIES {
            match e.find('/') {
                None => {
                    hosts.insert(*e);
                }
                Some(i) => paths.entry(&e[..i]).or_default().push(&e[i..]),
            }
        }
        Index { hosts, paths }
    })
}

/// 这个 host + path 是否免确认。
pub fn is_preapproved(host: &str, path: &str) -> bool {
    let idx = index();
    if idx.hosts.contains(host) {
        return true;
    }
    let Some(prefixes) = idx.paths.get(host) else {
        return false;
    };
    prefixes.iter().any(|p| is_path_prefix(path, p))
}

/// 前缀必须落在路径分隔符上。
///
/// 不检查边界的话，`/anthropics` 会匹配上 `/anthropics-evil/malware` ——
/// 任何人注册一个前缀相同的组织名就能白嫖白名单。
fn is_path_prefix(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 命中纯域名条目() {
        assert!(is_preapproved("docs.rs", "/tokio/latest"));
        assert!(is_preapproved("react.dev", "/"));
    }

    #[test]
    fn 未收录的域名不免确认() {
        assert!(!is_preapproved("evil.com", "/"));
        // 子域名不继承父域的授权
        assert!(!is_preapproved("evil.docs.rs.attacker.com", "/"));
    }

    #[test]
    fn 路径前缀必须落在分隔符上() {
        // 不检查边界的话，注册一个 vercel.com/docs-evil 就白嫖了白名单
        assert!(is_preapproved("vercel.com", "/docs"));
        assert!(is_preapproved("vercel.com", "/docs/functions"));
        assert!(!is_preapproved("vercel.com", "/docs-evil/malware"));
        assert!(!is_preapproved("vercel.com", "/dashboard"));
    }

    #[test]
    fn 名单里没有重复项() {
        // 重复不会报错，只会让人以为改了一处就够了
        let mut seen = HashSet::new();
        for e in ENTRIES {
            assert!(seen.insert(*e), "白名单里 `{e}` 重复了");
        }
    }

    #[test]
    fn 名单条目格式合法() {
        for e in ENTRIES {
            // 按 `://` 判，不能按 `http` 判 —— httpd.apache.org 是个合法条目
            assert!(!e.contains("://"), "`{e}` 不该带协议前缀");
            assert!(!e.ends_with('/'), "`{e}` 不该以斜杠结尾");
            assert!(e.contains('.'), "`{e}` 看起来不是域名");
        }
    }
}
