//! 主窗口的导航闸：应用的 webview 只许停在应用自己的页面上。
//!
//! # 堵的是什么
//!
//! Riot 的界面跑在系统 WebView 里，而 WebView 对一个 `<a href>` 的默认反应是
//! **把整个窗口导航到目标网址**。真发生过：在聊天里的链接上右键 →
//! "Open Link"，Riot 瞬间变成一个无边框浏览器，里面是那个网页，没有任何
//! 回来的路 —— 应用的 JS 状态全没了，只能重启。
//!
//! 前端在 `click` 事件上有一层兜底（`App.tsx` 里那个全局监听），能挡住
//! 左键点击。但 WebView 原生右键菜单的 "Open Link"、把链接拖进窗口、中键
//! 点击，**都不经过 DOM 事件**，是 WebView 自己直接发起的导航 —— JS 层
//! 根本看不见。所以关口只能设在宿主：Tauri 的 `on_navigation` 钩子在
//! WebView 做出任何导航之前问一句"放不放行"。
//!
//! `[约束]` 这层和前端那层**都要有**。宿主这层是真正的边界（什么都绕不
//! 过），前端那层给的是即时反馈（点了链接系统浏览器立刻弹出来，而不是
//! 悄无声息）。少了前端那层，被这里拦下的左键点击看起来像"链接点不动"。

use tauri::Url;

/// 对一次顶层导航的裁决。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 应用自己的页面，放行。
    Allow,
    /// 外部网址：不许在窗口里开，转给系统浏览器。
    OpenExternally,
    /// 别的一切（`file:`、`data:`、`javascript:`……）：直接拦下。
    ///
    /// 这些导航过去只会是裸文件页或白屏，而 `javascript:` 是经典的自我
    /// XSS。没有正当用途，也就没有必要为它们做任何事。
    Block,
}

/// 这次导航该怎么办。
///
/// `dev_url` 是 `tauri.conf.json` 里的 `build.devUrl`（开发时界面由 Vite
/// 提供），只在调试构建里算作应用自己的页面：发布版的用户机器上那个端口
/// 上跑的是谁都不知道，放行它等于让任何本地服务都能顶掉 Riot 的界面。
pub fn decide(url: &Url, dev_url: Option<&Url>) -> Verdict {
    // 生产环境下应用页面的两种形态：macOS / Linux 是自定义协议
    // `tauri://localhost`，Windows 走 `http://tauri.localhost`。
    if url.scheme() == "tauri" || url.host_str() == Some("tauri.localhost") {
        return Verdict::Allow;
    }
    // WebView 初始化时会先停在空白页，拦它没有意义。
    if url.as_str() == "about:blank" {
        return Verdict::Allow;
    }
    if cfg!(debug_assertions)
        && let Some(dev) = dev_url
        && url.scheme() == dev.scheme()
        && url.host_str() == dev.host_str()
        && url.port_or_known_default() == dev.port_or_known_default()
    {
        return Verdict::Allow;
    }
    match url.scheme() {
        "http" | "https" | "mailto" => Verdict::OpenExternally,
        _ => Verdict::Block,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).expect("合法 URL")
    }

    #[test]
    fn 应用自己的页面放行() {
        assert_eq!(decide(&u("tauri://localhost/"), None), Verdict::Allow);
        assert_eq!(decide(&u("tauri://localhost/index.html#x"), None), Verdict::Allow);
        assert_eq!(decide(&u("http://tauri.localhost/"), None), Verdict::Allow);
        assert_eq!(decide(&u("about:blank"), None), Verdict::Allow);
    }

    /// 这就是那次事故：聊天里的链接上右键 "Open Link"。
    #[test]
    fn 外部网址转给系统浏览器而不是在窗口里开() {
        for s in [
            "https://example.com/watch?v=1",
            "http://192.168.1.10:8080/admin",
            "mailto:someone@example.com",
        ] {
            assert_eq!(decide(&u(s), None), Verdict::OpenExternally, "{s}");
        }
    }

    /// 没有正当用途的协议直接拦。`javascript:` 是自我 XSS，`file:` 和
    /// `data:` 导航过去是裸文件页 / 白屏，和前端 click 兜底的判断一致。
    #[test]
    fn 其它协议直接拦下() {
        for s in [
            "file:///etc/passwd",
            "data:text/html,<h1>x</h1>",
            "javascript:alert(1)",
            "asset://localhost/x.png",
            "http://asset.localhost/x.png",
        ] {
            assert_ne!(decide(&u(s), None), Verdict::Allow, "{s} 不该放行");
        }
        assert_eq!(decide(&u("javascript:alert(1)"), None), Verdict::Block);
        assert_eq!(decide(&u("file:///etc/passwd"), None), Verdict::Block);
    }

    /// 开发服务器只在调试构建里算自己人，而且要整个 origin 对得上。
    #[test]
    fn 开发服务器按_origin_放行() {
        let dev = u("http://127.0.0.1:1420");
        let same = decide(&u("http://127.0.0.1:1420/index.html"), Some(&dev));
        if cfg!(debug_assertions) {
            assert_eq!(same, Verdict::Allow);
        } else {
            assert_eq!(same, Verdict::OpenExternally, "发布版里本地端口不是自己人");
        }
        // 端口不同就是另一个服务 —— 用户自己项目的 dev server 常常就在隔壁端口。
        assert_eq!(
            decide(&u("http://127.0.0.1:5173/"), Some(&dev)),
            Verdict::OpenExternally
        );
        assert_eq!(
            decide(&u("http://localhost:1420/"), Some(&dev)),
            Verdict::OpenExternally,
            "host 也要严格相等，localhost 和 127.0.0.1 不互认"
        );
    }
}
