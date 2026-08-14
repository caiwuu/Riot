//! 对着真实 API 打一次，在终端看结果。
//!
//! GUI 会把配置问题藏起来 —— key 没读到、base URL 写错、模型名不存在，
//! 在界面上都表现为"发了消息没反应"。这个例子把整条链路的每一步都打出来。
//!
//! ```bash
//! export DEEPSEEK_API_KEY=sk-...
//! cargo run -p riot-providers --example smoke
//!
//! # 换模型 / 换服务
//! MODEL=deepseek-reasoner cargo run -p riot-providers --example smoke
//! BASE_URL=https://api.moonshot.cn MODEL=kimi-k2-turbo-preview \
//!   KEY_ENV=MOONSHOT_API_KEY cargo run -p riot-providers --example smoke
//! ```

#![allow(clippy::disallowed_methods)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use riot_protocol::event::StreamDelta;
use riot_protocol::id::MessageId;
use riot_protocol::message::{AssistantContent, Message, MessageMeta, UserContent};
use riot_protocol::provider::{
    Provider, ProviderEvent, ProviderRequest, ThinkingConfig, ToolSpec,
};
use riot_providers::watchdog::TokioClock;
use riot_providers::{OpenAiConfig, OpenAiProvider, ReqwestTransport};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    let base_url = env("BASE_URL", "https://api.deepseek.com");
    let model = env("MODEL", "deepseek-chat");
    let key_env = env("KEY_ENV", "DEEPSEEK_API_KEY");
    let prompt = env("PROMPT", "用一句话说明什么是 TOCTOU 漏洞。");

    let Ok(api_key) = std::env::var(&key_env) else {
        eprintln!("没有环境变量 {key_env}。先 export 它再跑。");
        std::process::exit(1);
    };

    // 和真正发请求走同一套拼接，否则这行打印会在带路径的 base 上说谎。
    println!(
        "端点   {}",
        riot_providers::endpoint::api_url(&base_url, "v1", "chat/completions")
    );
    println!("模型   {model}");
    println!("密钥   {key_env}（{} 字符）", api_key.trim().len());
    println!("提问   {prompt}\n");

    let provider = OpenAiProvider::new(
        Arc::new(ReqwestTransport::new().expect("建 HTTP 客户端")),
        Arc::new(TokioClock),
        Vec::new(),
        OpenAiConfig {
            base_url,
            api_key: api_key.trim().to_owned(),
            idle_timeout: Duration::from_secs(60),
            ..Default::default()
        },
    );

    // 带一个工具定义。工具调用是最容易在适配层出问题的地方，冒烟测试
    // 不带它的话，"能聊天"和"能干活"之间的差距要等到用的时候才发现。
    let req = ProviderRequest {
        model,
        messages: vec![Message::User {
            id: MessageId::from_raw("u1"),
            content: vec![UserContent::Text { text: prompt }],
            meta: MessageMeta::default(),
        }],
        system: "你是一个简洁的助手。".into(),
        tools: vec![ToolSpec {
            name: "Read".into(),
            description: "读取一个文件的内容".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"],
            }),
        }],
        max_output_tokens: Some(512),
        thinking: ThinkingConfig::Off,
    };

    let started = Instant::now();
    let mut first_byte: Option<Duration> = None;
    let mut failed = false;

    let stream = provider.stream(req, CancellationToken::new());
    futures::pin_mut!(stream);

    while let Some(ev) = stream.next().await {
        match ev {
            ProviderEvent::Delta(StreamDelta::Text { text, .. }) => {
                first_byte.get_or_insert_with(|| started.elapsed());
                print!("{text}");
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            ProviderEvent::Delta(StreamDelta::Thinking { text, .. }) => {
                first_byte.get_or_insert_with(|| started.elapsed());
                eprint!("\x1b[2m{text}\x1b[0m");
            }
            ProviderEvent::Delta(StreamDelta::ToolInput { .. }) => {}
            ProviderEvent::Message(Message::Assistant { content, .. }) => {
                for c in &content {
                    if let AssistantContent::ToolUse { name, input, .. } = c {
                        println!("\n\n[工具调用] {name} {input}");
                    }
                }
            }
            ProviderEvent::Message(_) => {}
            ProviderEvent::Usage(u) => {
                println!(
                    "\n\n用量   输入 {} / 输出 {} / 缓存命中 {}",
                    u.input_tokens, u.output_tokens, u.cache_read_tokens
                );
            }
            ProviderEvent::Error(e) => {
                eprintln!("\n\n失败：{e}");
                failed = true;
            }
        }
    }

    println!(
        "耗时   首字 {:?} / 总计 {:?}",
        first_byte.unwrap_or_default(),
        started.elapsed()
    );

    if failed {
        std::process::exit(1);
    }
}

fn env(k: &str, default: &str) -> String {
    std::env::var(k)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_owned())
}
