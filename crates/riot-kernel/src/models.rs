//! 模型端点的装配与探测：配置 → Provider 实例，以及设置页的
//! 「拉模型清单」「测试连接」。
//!
//! 从 session.rs 拆出来的独立职责。这里的函数全是**配置期**操作：
//! 不碰会话状态，不进主循环 —— 会话运行时只在装配阶段调一次
//! [`provider_for`]。
//!
//! 命名成 `models` 而不是 `providers`，是为了和外部 crate
//! `riot_providers` 在搜索时区分开：这里管的是"用户配置里的模型
//! 条目"，那边是各协议的流式实现。

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use riot_protocol::id::MessageId;
use riot_protocol::message::{Message, MessageMeta, UserContent};
use riot_protocol::provider::Provider;
use riot_providers::anthropic::request::SystemSection;
use riot_providers::{
    AnthropicConfig, AnthropicProvider, OpenAiConfig, OpenAiProvider, ReqwestTransport,
};

use crate::config::ResolvedModel;
use crate::prompt::Flavor;

/// 按配置构建 provider。会话和"测试连接"共用 —— 两处各写一遍的话，
/// 测试通过而正式请求失败（或反过来）这种事迟早发生。
pub fn provider_for(model: &ResolvedModel) -> Result<Arc<dyn Provider>, String> {
    provider_from_endpoint(&model.to_endpoint().map_err(|e| e.to_string())?)
}

/// 从一个解析好的端点(含明文 key)建 Provider。
///
/// 这是建 Provider 的核心入口。阶段 B 拆进程后内核走这条:宿主把
/// [`riot_protocol::ModelEndpoint`] 随 RPC 传进来,内核不碰 auth.json。
/// 内嵌期的 [`provider_for`] 也经它 —— 只是多一步从 `ResolvedModel` 把 key
/// 解析出来。两处共用一份建构逻辑,避免"内嵌能连、RPC 连不上"这类分叉。
pub fn provider_from_endpoint(
    model: &riot_protocol::ModelEndpoint,
) -> Result<Arc<dyn Provider>, String> {
    let key = model.api_key.clone();
    // 空 key = 宿主没解析出密钥(环境变量 / auth.json 都没有)。在这里立即
    // 失败,不建 provider、不发请求 —— 和拆进程前 provider_for 缺 key 的行为
    // 一致(那时靠 ResolvedModel::api_key() 报 MissingKey)。
    if key.trim().is_empty() {
        return Err("缺少 API key".to_owned());
    }
    let transport = Arc::new(ReqwestTransport::new().map_err(|e| e.to_string())?);
    let clock = Arc::new(riot_providers::watchdog::TokioClock);

    let sampling = riot_providers::SamplingParams {
        temperature: model.sampling.temperature,
        top_p: model.sampling.top_p,
        top_k: model.sampling.top_k,
    };

    if model.is_anthropic() {
        return Ok(Arc::new(AnthropicProvider::new(
            transport,
            clock,
            vendor_sections(model),
            AnthropicConfig {
                base_url: model.base_url.clone(),
                api_path: model.api_path.clone(),
                api_key: key,
                fallback_model: model.fallback_model.clone(),
                sampling,
                ..Default::default()
            },
        )));
    }

    Ok(Arc::new(OpenAiProvider::new(
        transport,
        clock,
        vendor_sections(model),
        OpenAiConfig {
            base_url: model.base_url.clone(),
            api_path: model.api_path.clone(),
            api_key: key,
            fallback_model: model.fallback_model.clone(),
            sampling,
            ..Default::default()
        },
    )))
}

/// provider 级的 system 分节：**只对某一家成立**的补充说明挂在这里。
///
/// 会话那份完整的 system prompt 不走这条路 —— 它逐会话不同（工作目录、
/// venv、用户补充指令），而 provider 是按端点建的、跨会话复用，把会话内容
/// 塞进来只会让两者的生命周期对不上。它走 `ProviderRequest::system`，在
/// [`crate::prompt::system_prompt`] 里按分节装配好，用
/// [`riot_providers::anthropic::SYSTEM_SECTION_BOUNDARY`] 标出缓存边界。
///
/// 这里留给真正的厂商差异：某一家特有的工具调用怪癖、某一家需要额外一句
/// 才肯遵守的格式约定。
///
/// TODO(prompt): 目前两家都返回空。要往里加内容前先确认它**只**对这一家
/// 成立 —— 通用的话属于 `prompt.rs` 的分节，写在这里等于让另一半后端拿不到。
/// 已知的候选：
/// - Anthropic：并行工具调用的服从度最高，可以给更激进的批量指引；
/// - OpenAI 兼容（DeepSeek、智谱）：对 `<system-reminder>` 这类 XML 包装的
///   服从度偏弱，可能需要一句「被 `<system-reminder>` 包住的内容和用户
///   本人说的话等权」。分节外壳的差异已经由 [`crate::prompt::Flavor`] 处理。
fn vendor_sections(model: &riot_protocol::ModelEndpoint) -> Vec<SystemSection> {
    match flavor_for(model) {
        // 分支合并着写是因为两家现在都空。留着这个 match 而不是直接返回
        // `Vec::new()`，是为了让「加一句只对某家说的话」有个明确的落点 ——
        // 没有落点的话，下一个人会把它塞进通用提示词里。
        Flavor::Anthropic | Flavor::OpenAiCompatible => Vec::new(),
    }
}

/// 端点 → 提示词的渲染风格。
///
/// 厂商知识留在这个模块里：`prompt.rs` 只认 [`crate::prompt::Flavor`]，
/// 不认协议枚举 —— 否则每加一个后端都要去改提示词文件。
pub(crate) fn flavor_for(model: &riot_protocol::ModelEndpoint) -> Flavor {
    if model.is_anthropic() {
        Flavor::Anthropic
    } else {
        Flavor::OpenAiCompatible
    }
}

/// 拉取服务方的可用模型列表（`GET /v1/models`，两个协议的响应
/// 恰好都是 `{"data":[{"id":...}]}`）。
///
/// 独立于 Provider trait：列模型是配置期操作，不该走流式管线。
pub async fn list_models(p: &crate::config::ProviderConfig) -> Result<Vec<String>, String> {
    let key = p.api_key().map_err(|e| e.to_string())?;
    let client = reqwest::Client::new();

    let mut ids: Vec<String> = Vec::new();
    let mut first_error: Option<String> = None;

    for url in model_list_urls(&p.base_url, &p.api_path) {
        let req = match p.protocol {
            crate::config::Protocol::Openai => client.get(&url).bearer_auth(key.clone()),
            crate::config::Protocol::Anthropic => client
                .get(&url)
                .header("x-api-key", key.clone())
                .header("anthropic-version", "2023-06-01"),
        };
        match fetch_models(req).await {
            Ok(found) => ids.extend(found),
            Err(e) => {
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
        }
    }

    if ids.is_empty() {
        // 一个都没问到才算失败。报第一条错 —— 它来自最规范的那个路径，
        // 而后面那个只是补充。
        return Err(first_error.unwrap_or_else(|| "服务方没有返回任何模型".to_owned()));
    }

    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// 模型清单可能在哪几个地址上。
///
/// `[约束]` 要问不止一个路径。各家的"清单"和"对话"不一定在同一层:智谱的
/// `/api/paas/v4/models` 只列 8 个模型，而 `/api/paas/v4/v1/models` 列 14 个
/// （视觉模型全在后者里），两个都通 —— 而对话**必须**走不带 `/v1` 的那个，
/// 带上就 404。
///
/// 只问一个的后果是"能用的模型在列表里看不见":用户在设置里找不到
/// `glm-4.6v`，而它明明能对话。实测过这两个路径的返回。
///
/// `[取舍]` 合并两份清单，代价是可能列出对话端点不认的模型。那个由模型弹窗
/// 里的「测试模型」兜底 —— 一次点击就能确认，比"看不见"好排查得多。
fn model_list_urls(base: &str, api_path: &str) -> Vec<String> {
    let root = base.trim().trim_end_matches('/');
    let mut urls = Vec::new();

    // 用户配了对话路径的话，清单大概率和它同一层:把接口那一段换成 models。
    // `/v1/chat/completions` → `/v1/models`。
    //
    // `[约束]` 要按**已知的接口尾巴**剥，不能只剥最后一段。OpenAI 的尾巴是
    // 两段（`chat/completions`），只剥一段会拼出 `/v1/chat/models`。
    if let Some(prefix) = strip_endpoint_tail(api_path)
        && !prefix.is_empty()
    {
        urls.push(format!("{root}/{prefix}/models"));
    }

    urls.push(riot_providers::endpoint::api_url(base, "v1", "models"));
    // 再试一次在同一个根上多接一层 `v1`（智谱那种把 OpenAI 兼容清单挂在
    // `<根>/v1/models` 的布局）。
    urls.push(format!("{root}/v1/models"));

    urls.dedup();
    // 去重要按值，不只是相邻 —— 上面三条在常见配置下会两两相同。
    let mut seen = std::collections::HashSet::new();
    urls.retain(|u| seen.insert(u.clone()));
    urls
}

/// 把对话路径末尾那个接口名剥掉，留下它所在的那一层。
///
/// 认得出的尾巴优先（两个协议各一个）；都不匹配时退回"去掉最后一段"，
/// 那对自定义网关是个合理的猜测。
fn strip_endpoint_tail(api_path: &str) -> Option<&str> {
    let p = api_path
        .trim()
        .trim_start_matches('/')
        .trim_end_matches('/');
    if p.is_empty() {
        return None;
    }
    for tail in ["chat/completions", "messages", "completions"] {
        if let Some(rest) = p.strip_suffix(tail) {
            return Some(rest.trim_end_matches('/'));
        }
    }
    p.rsplit_once('/').map(|(head, _)| head)
}

/// 发一次清单请求。
async fn fetch_models(req: reqwest::RequestBuilder) -> Result<Vec<String>, String> {
    // 等外部服务，真实时钟
    #[allow(clippy::disallowed_methods)]
    let resp = req
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("请求失败：{e}"))?;

    let status = resp.status();
    let body = resp.text().await.map_err(|e| format!("读响应失败：{e}"))?;
    if !status.is_success() {
        // 错误体里常有有用的说明（key 无效、路径不对），截断后带给用户
        let hint: String = body.chars().take(200).collect();
        return Err(format!("HTTP {status}：{hint}"));
    }

    #[derive(serde::Deserialize)]
    struct ModelEntry {
        id: String,
    }
    #[derive(serde::Deserialize)]
    struct ModelList {
        data: Vec<ModelEntry>,
    }

    let list: ModelList =
        serde_json::from_str(&body).map_err(|e| format!("响应不是模型列表：{e}"))?;
    Ok(list.data.into_iter().map(|m| m.id).collect())
}

/// 用当前配置发一个最小请求，验证 base URL、key、模型名这条链路通不通。
///
/// 这是设置页"测试连接"按钮的后端。没有它的话，配置错误的表现是
/// "发消息后转圈很久然后报一长串"—— 用户分不清是网络、key 还是模型名的锅。
pub async fn test_connection(model: &ResolvedModel) -> Result<String, String> {
    use riot_protocol::provider::{ProviderEvent, ProviderRequest};

    let provider = provider_for(model)?;
    let req = ProviderRequest {
        model: model.model.clone(),
        messages: vec![Message::User {
            id: MessageId::from_raw("msg_conn_test"),
            content: vec![UserContent::Text {
                text: "ping".into(),
            }],
            meta: MessageMeta::default(),
        }],
        system: String::new(),
        tools: Vec::new(),
        // 要的是"链路通"，不是回答质量 —— 别让用户为一次握手付整段生成的钱
        max_output_tokens: Some(16),
        thinking: Default::default(),
    };

    let cancel = CancellationToken::new();
    let mut stream = provider.stream(req, cancel.clone());

    use futures::StreamExt;
    // 等的是外部服务，真实时钟。30 秒等不来第一个事件就是链路有问题。
    #[allow(clippy::disallowed_methods)]
    let verdict = tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(ev) = stream.next().await {
            match ev {
                ProviderEvent::Message(_) | ProviderEvent::Usage(_) => {
                    return Ok(());
                }
                ProviderEvent::Error(e) => return Err(format!("{e}")),
                _ => {}
            }
        }
        Err("连接中断，没有收到任何响应".to_owned())
    })
    .await;

    cancel.cancel();
    match verdict {
        Ok(Ok(())) => Ok(format!("连接正常：{} @ {}", model.model, model.base_url)),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("30 秒内没有响应。检查 base URL 和网络。".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 拉模型清单要问两个路径。
    ///
    /// `[约束]` 这条盯的是"能用的模型在列表里看不见"。智谱把 OpenAI 兼容的
    /// 清单挂在 `<根>/v1/models`（14 个，视觉模型全在里面），而它自己的
    /// `<根>/models` 只有 8 个 —— 对话却必须走不带 `/v1` 的根。只问一个路径，
    /// 用户就永远找不到 `glm-4.6v`，而那个模型明明能对话。
    #[test]
    fn 模型清单问两个路径() {
        let urls = model_list_urls("https://open.bigmodel.cn/api/paas/v4", "");
        assert_eq!(
            urls,
            vec![
                "https://open.bigmodel.cn/api/paas/v4/models".to_owned(),
                "https://open.bigmodel.cn/api/paas/v4/v1/models".to_owned(),
            ]
        );

        // 只有主机名时两条会撞成同一个地址，那就只问一次 —— 同一个请求发两遍
        // 只是白等一次超时。
        assert_eq!(
            model_list_urls("https://api.deepseek.com", ""),
            vec!["https://api.deepseek.com/v1/models".to_owned()]
        );
        // 尾斜杠不该产生双斜杠，有些网关把 `//` 当成另一个路径。
        assert_eq!(
            model_list_urls("https://api.deepseek.com/", ""),
            vec!["https://api.deepseek.com/v1/models".to_owned()]
        );
    }

    /// 用户配了对话路径时，清单先按同一层去问。
    ///
    /// `[约束]` 自建网关常常把两个接口挂在同一个前缀下（`/openai/v1/...`），
    /// 而那个前缀我们猜不出来。不跟着用户配的路径走的话，他明明能对话，
    /// 「从 API 获取」却一直失败。
    #[test]
    fn 配了路径时清单跟着同一层去问() {
        let urls = model_list_urls("https://gw.test", "/openai/v1/chat/completions");
        assert_eq!(urls[0], "https://gw.test/openai/v1/models");
        // 后面两条兜底照旧留着 —— 有些网关的清单确实不在那一层。
        assert!(urls.contains(&"https://gw.test/v1/models".to_owned()));

        // 路径只有一段时没有"上一层"，跳过它别拼出 `//models`。
        let urls = model_list_urls("https://gw.test/api", "/completions");
        assert!(
            urls.iter().all(|u| !u.contains("//models")),
            "不该拼出双斜杠：{urls:?}"
        );
    }
}
