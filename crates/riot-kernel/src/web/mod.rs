//! 宿主侧的联网装配。
//!
//! [`riot_protocol::web::WebAccess`] 的三个方法各有各的归属：
//!
//! | 方法 | 谁来做 | 为什么在宿主 |
//! |---|---|---|
//! | `get` | `riot-runtime` 的 `SystemWebClient` | 只是转发，但要看开关 |
//! | `search` | [`searxng`] | 后端地址是用户配置 |
//! | `distill` | [`distill`] | 辅助模型是用户配置 |
//!
//! # 每轮重建，不做缓存
//!
//! [`HostWeb`] 由 `AppState::send_turn` 在每一轮开始时按当时的配置构造。
//! 用户在对话中途填上 SearXNG 地址，下一轮就能搜 —— 不用重启，也不用为
//! "配置变了"再搭一套通知机制。构造成本是两个 `reqwest::Client`
//! （内部连接池是 `Arc`，克隆很便宜），相对于一轮模型调用可以忽略。

pub mod distill;
pub mod searxng;

#[cfg(test)]
mod tests;

use async_trait::async_trait;
use riot_protocol::web::{
    DistillRequest, SearchHit, SearchQuery, WebAccess, WebError, WebRequest, WebResponse,
};
use riot_runtime::SystemWebClient;
use tokio_util::sync::CancellationToken;

use crate::config::AppConfig;
use distill::Distiller;

/// 发给外部服务的 UA。
///
/// 写明自己是谁：一部分站点会挡掉空 UA 或明显的脚本 UA，而伪装成浏览器
/// 是另一个方向的错误 —— 站点管理员应该能从日志里认出这是个 agent。
const USER_AGENT: &str = concat!("Riot/", env!("CARGO_PKG_VERSION"), " (+agent)");

pub struct HostWeb {
    /// `None` = 用户关掉了抓取。
    fetch: Option<SystemWebClient>,
    /// `None` = 搜索没开或没填地址。
    search: Option<Searxng>,
    /// `None` = 没配辅助模型。抓取会降级成截断原文，不算失败。
    distiller: Option<Distiller>,
}

struct Searxng {
    client: reqwest::Client,
    base_url: String,
}

impl HostWeb {
    /// 按当前配置装一套联网能力。
    ///
    /// 不返回 `Result`：任何一块配坏了都只让那一块变成"未配置"，其余的
    /// 照常能用。搜索地址填错不该连带着让 WebFetch 也用不了。
    pub fn from_config(cfg: &AppConfig) -> Self {
        let fetch = cfg.web.fetch_enabled.then(SystemWebClient::new).and_then(|r| {
            r.inspect_err(|e| tracing::warn!(error = %e, "抓取客户端没建起来"))
                .ok()
        });

        let search = cfg.web.search_ready().then(|| Searxng {
            client: search_client(),
            base_url: cfg.web.searxng_url.trim().trim_end_matches('/').to_owned(),
        });

        let distiller = cfg.web.distill_target().and_then(|(pid, model)| {
            let resolved = cfg
                .resolve_named(pid, model)
                .inspect_err(|e| tracing::warn!(error = %e, "辅助模型解析失败"))
                .ok()?;
            crate::session::provider_for(&resolved)
                .inspect_err(|e| tracing::warn!(error = %e, "辅助模型的 provider 建不出来"))
                .ok()
                .map(|p| Distiller::new(p, resolved.model))
        });

        Self {
            fetch,
            search,
            distiller,
        }
    }

    /// 从 RPC 传入的 [`riot_protocol::WebSetup`] 装一套联网能力(拆进程后
    /// 内核走这条,不碰 AppConfig)。语义和 [`Self::from_config`] 一致。
    pub fn from_setup(setup: &riot_protocol::WebSetup) -> Self {
        let fetch = setup.fetch_enabled.then(SystemWebClient::new).and_then(|r| {
            r.inspect_err(|e| tracing::warn!(error = %e, "抓取客户端没建起来"))
                .ok()
        });
        let search = (setup.search_enabled && !setup.searxng_url.trim().is_empty()).then(|| {
            Searxng {
                client: search_client(),
                base_url: setup.searxng_url.trim().trim_end_matches('/').to_owned(),
            }
        });
        let distiller = setup.distill.as_ref().and_then(|ep| {
            crate::session::provider_from_endpoint(ep)
                .inspect_err(|e| tracing::warn!(error = %e, "辅助模型的 provider 建不出来"))
                .ok()
                .map(|p| Distiller::new(p, ep.model.clone()))
        });
        Self {
            fetch,
            search,
            distiller,
        }
    }
}

/// 专供搜索后端的 HTTP 客户端。
///
/// `[约束]` 这个客户端**没有**内网拦截解析器，而抓取用的那个有。
/// 差别是有意的：SearXNG 最常见的部署就是 `http://127.0.0.1:8080`，
/// 套上内网拦截等于这个功能不存在。
///
/// 两者的区别在于 URL 从哪来 —— 抓取的 URL 由模型给出（不可信），
/// 搜索的地址由用户在设置里手填（可信），模型只能影响被 URL 编码过的
/// 查询串。把这两个客户端合成一个，就等于把这条区别弄丢了。
fn search_client() -> reqwest::Client {
    #[allow(clippy::disallowed_methods)]
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .user_agent(USER_AGENT)
        .build()
        // Client::builder() 只在 TLS 后端初始化失败时才会出错，那种情况下
        // 默认客户端也一样起不来。这里不值得把错误往上传一路。
        .unwrap_or_default()
}

#[async_trait]
impl WebAccess for HostWeb {
    async fn get(
        &self,
        req: WebRequest,
        cancel: &CancellationToken,
    ) -> Result<WebResponse, WebError> {
        match &self.fetch {
            Some(c) => c.get(req, cancel).await,
            None => Err(WebError::NotConfigured {
                what: "网页抓取（在「设置 → 联网」里打开）".to_owned(),
            }),
        }
    }

    async fn search(
        &self,
        query: SearchQuery,
        cancel: &CancellationToken,
    ) -> Result<Vec<SearchHit>, WebError> {
        let Some(s) = &self.search else {
            return Err(WebError::NotConfigured {
                what: "搜索后端".to_owned(),
            });
        };
        searxng::search(&s.client, &s.base_url, &query, cancel).await
    }

    async fn distill(
        &self,
        req: DistillRequest,
        cancel: &CancellationToken,
    ) -> Result<String, WebError> {
        match &self.distiller {
            Some(d) => d.run(req, cancel).await,
            None => Err(WebError::NotConfigured {
                what: "辅助模型".to_owned(),
            }),
        }
    }
}

/// 测一下 SearXNG 地址能不能用。设置页的「测试」按钮走这里。
///
/// 用一个真实查询而不是打首页：首页返回 200 只说明有个 web 服务在那，
/// 说明不了 JSON 输出开没开 —— 而那正是最容易配错的一处。
pub async fn test_searxng(base_url: &str) -> Result<String, String> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("先填 SearXNG 的地址，比如 http://127.0.0.1:8080".to_owned());
    }
    if !base.starts_with("http://") && !base.starts_with("https://") {
        return Err(format!("地址要带上协议：http://{base}"));
    }

    let hits = searxng::search(
        &search_client(),
        base,
        &SearchQuery {
            query: "hello".to_owned(),
            max_results: 3,
            ..Default::default()
        },
        &CancellationToken::new(),
    )
    .await
    .map_err(|e| e.to_string())?;

    if hits.is_empty() {
        // 通了但没结果：JSON 是对的，是上游引擎那边的问题。这两种情况
        // 对用户来说要做的事完全不同，不能都报"成功"。
        return Err("连上了，但没有返回任何结果。检查 SearXNG 里启用的搜索引擎。".to_owned());
    }
    Ok(format!("连接正常，返回了 {} 条结果", hits.len()))
}
