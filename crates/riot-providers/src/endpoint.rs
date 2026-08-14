//! 把接口路径接到用户填的 base URL 后面。
//!
//! # 为什么这件事需要一个模块
//!
//! `[约束]` 不能无条件补 `/v1`。
//!
//! 各家的 API 根路径不一样:OpenAI 是 `/v1`，智谱是 `/api/paas/v4`，
//! Ollama 是 `/v1`，各种中转还会带自己的前缀。无条件补的结果是
//! `https://open.bigmodel.cn/api/paas/v4/v1/chat/completions` —— 服务方回
//! 一个 404，而那个 404 里没有任何线索指向"我们多接了一段路径"。用户看到的
//! 只是"测试连接失败 404"，然后会去反复检查 key 和模型名。
//!
//! # 规则
//!
//! base 里已经有路径，就当它是 API 根，只接尾巴；只有主机名时补上默认版本段。
//!
//! `[取舍]` 后半条是给最常见的粘贴方式兜底 —— 很多人只复制到域名
//! （`https://api.deepseek.com`）。前半条则和所有 OpenAI 兼容 SDK 的约定
//! 一致:base_url 就是 API 根，客户端只接 `/chat/completions`。
//!
//! 代价是一种猜错:某个中转的根是 `https://proxy.test/openai`、而它期待
//! `/openai/v1/chat/completions`。那种情况用户把 base 填成
//! `https://proxy.test/openai/v1` 就对了 —— 而这也正是那些中转文档里写的。

/// 拼出完整的接口地址。
///
/// - `base` 用户填的地址，可以带或不带尾斜杠。
/// - `default_version` base 只有主机名时补的版本段，如 `v1`。
/// - `tail` 接口路径，如 `chat/completions`。
pub fn api_url(base: &str, default_version: &str, tail: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    // 有没有路径:去掉 scheme 之后还剩 '/' 就说明有。
    let after_scheme = base.split_once("://").map_or(base, |(_, rest)| rest);
    if after_scheme.contains('/') {
        format!("{base}/{tail}")
    } else {
        format!("{base}/{default_version}/{tail}")
    }
}

/// 用户显式指定了路径时，直接按它拼。
///
/// `[取舍]` 猜路径永远猜不全。上面那条规则已经踩过两次:智谱的对话在
/// `/api/paas/v4/chat/completions`（不能带 `/v1`），而它的完整模型清单偏偏在
/// `/api/paas/v4/v1/models`。中转、自建网关的花样只会更多。
///
/// 所以路径是可配置的:填了就照填的走，界面上还能直接看到拼出来的完整地址；
/// 空着才回落到那套猜测（[`api_url`]）—— 大多数人不需要关心这件事。
///
/// `path` 前面有没有斜杠都接受。少写一个斜杠就把请求打到别的地方去，
/// 而报错只会是一个 404。
pub fn api_url_with(base: &str, path: &str, default_version: &str, tail: &str) -> String {
    let path = path.trim();
    if path.is_empty() {
        return api_url(base, default_version, tail);
    }
    format!(
        "{}/{}",
        base.trim().trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn 只给主机名时补上默认版本段() {
        // 最常见的粘贴方式:只复制到域名。
        assert_eq!(
            api_url("https://api.deepseek.com", "v1", "chat/completions"),
            "https://api.deepseek.com/v1/chat/completions"
        );
        // 尾斜杠不该产生双斜杠 —— 有些网关会把 `//` 当成不同的路径。
        assert_eq!(
            api_url("https://api.deepseek.com/", "v1", "chat/completions"),
            "https://api.deepseek.com/v1/chat/completions"
        );
    }

    #[test]
    fn base_已经带路径时不再补版本段() {
        // `[约束]` 这条盯的是智谱那个 404。它的 API 根是 /api/paas/v4，
        // 无条件补 /v1 会拼出 .../api/paas/v4/v1/chat/completions。
        assert_eq!(
            api_url("https://open.bigmodel.cn/api/paas/v4", "v1", "chat/completions"),
            "https://open.bigmodel.cn/api/paas/v4/chat/completions"
        );
        // 自己带了 /v1 的也不能变成 /v1/v1。
        assert_eq!(
            api_url("https://api.openai.com/v1", "v1", "chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            api_url("http://127.0.0.1:11434/v1", "v1", "chat/completions"),
            "http://127.0.0.1:11434/v1/chat/completions"
        );
    }

    #[test]
    fn 端口不会被当成路径() {
        // 冒号后面那串数字里没有 '/'，判断只看路径分隔符。
        assert_eq!(
            api_url("http://localhost:8000", "v1", "models"),
            "http://localhost:8000/v1/models"
        );
    }

    #[test]
    fn 两个协议的尾巴各自独立() {
        assert_eq!(
            api_url("https://api.anthropic.com", "v1", "messages"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            api_url("https://gateway.test/anthropic", "v1", "messages"),
            "https://gateway.test/anthropic/messages"
        );
    }
}
