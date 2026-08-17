//! riot 内核:独立进程入口(ARCHITECTURE.md §2.2 阶段 B)。
//!
//! 这个二进制被宿主 spawn,通过 stdin/stdout 上的 JSON-RPC 通信。它是薄壳:
//! 装好日志与 panic hook,然后把 stdin/stdout 交给 [`riot_kernel::serve`]。

/// 顶层 panic hook。
///
/// `[约束]`(ARCHITECTURE.md §2.4)内核不允许静默死亡。任何逃逸到顶层的
/// panic 都要:把现场写进日志(stderr,宿主会收),并通过 RPC 通知宿主 ——
/// 后者据此决定重启,而不是对着一个突然沉默的进程干等到超时。
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // 先尽力把"内核要死了"送出去。放在默认 hook 之前:默认 hook 在
        // panic=abort 下可能直接结束进程,排在它后面就发不出去了。
        riot_kernel::report_kernel_error(format!("内核 panic:{info}"), true);
        default(info);
    }));
}

#[tokio::main]
async fn main() {
    // `[约束]` 日志走 stderr。stdout 是 JSON-RPC 协议通道,一行日志混进去
    // 就会被宿主的读取器当成非法 JSON。宿主的 supervisor 已经把内核 stderr
    // 接进它自己的 tracing。
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    install_panic_hook();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "内核启动,等待 stdin 上的 JSON-RPC"
    );
    riot_kernel::serve(tokio::io::stdin(), tokio::io::stdout()).await;
    tracing::info!("stdin 关闭,内核退出");
}
