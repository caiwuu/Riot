//! 浏览器的高层操作。工具层调这里，不直接拼 CDP。
//!
//! 每个函数就是一两条 CDP 命令加一点结果整形。放在这一层而不是工具里，
//! 是因为"用哪个 CDP 方法、参数怎么填"属于浏览器知识，而工具层该只关心
//! "给模型什么"。
//!
//! # 给模型的东西要克制
//!
//! 页面的原始 DOM 动辄几十万字符，截图动辄几 MB。直接塞给模型的结果是
//! 一次调用吃掉整个上下文，后面几轮全靠压缩苟活。所以这里的每个函数都
//! 自带上限，而且宁可截断也不放行。

use serde_json::{Value, json};

use super::{Browser, BrowserError};

/// 可访问性树的节点数上限。
///
/// 一个中等复杂的页面能有几千个节点。全给模型意味着几万 token，而它真正
/// 需要的通常是"页面上有哪些可交互的东西"。超出就截断并说明。
const MAX_A11Y_NODES: usize = 400;

/// console 一次最多回多少条。
const MAX_CONSOLE: usize = 100;

/// 导航并等页面加载完。
///
/// `[约束]` 必须等 `Page.loadEventFired`，不能发完 `Page.navigate` 就返回。
/// 单页应用尤其明显:立刻截图会拍到白屏，而模型会认真地分析那张白屏。
pub async fn navigate(browser: &Browser, url: &str) -> Result<(), BrowserError> {
    // Page 域要先 enable，否则 loadEventFired 不会送出来。
    browser.cdp("Page.enable", json!({})).await?;
    browser.cdp("Page.navigate", json!({ "url": url })).await?;
    Ok(())
}

/// 整页截图，返回 PNG 的 base64。
///
/// `[取舍]` 用 `captureBeyondViewport` 拍完整页面而不是只拍视口。模型看
/// 视口截图会漏掉折叠下方的内容，然后自信地说"页面上没有那个按钮"。
pub async fn screenshot(browser: &Browser) -> Result<String, BrowserError> {
    let r = browser
        .cdp(
            "Page.captureScreenshot",
            json!({ "format": "png", "captureBeyondViewport": true }),
        )
        .await?;
    Ok(r["data"].as_str().unwrap_or_default().to_owned())
}

/// 页面的可访问性快照。
///
/// `[取舍]` 用 a11y 树而不是 DOM。理由是**信噪比**:a11y 树给的是角色、
/// 名字、状态（button "提交"、textbox "邮箱" required），正好是模型要
/// 决策"点哪儿"需要的信息；而 DOM 里九成是 class 名和布局容器。同样的
/// token 预算下，a11y 树能表达的页面结构多一个数量级。
pub async fn snapshot(browser: &Browser) -> Result<String, BrowserError> {
    browser.cdp("Accessibility.enable", json!({})).await?;
    let r = browser.cdp("Accessibility.getFullAXTree", json!({})).await?;

    let nodes = r["nodes"].as_array().map_or(&[][..], Vec::as_slice);
    let total = nodes.len();

    let mut out = String::new();
    for node in nodes.iter().take(MAX_A11Y_NODES) {
        let Some(line) = describe_node(node) else {
            continue;
        };
        out.push_str(&line);
        out.push('\n');
    }

    if total > MAX_A11Y_NODES {
        out.push_str(&format!(
            "\n（页面共 {total} 个节点，只显示前 {MAX_A11Y_NODES} 个。\
             要看具体区域请用 CSS 选择器缩小范围。）\n"
        ));
    }
    Ok(out)
}

/// 一个 a11y 节点的单行描述。忽略没有信息量的节点。
fn describe_node(node: &Value) -> Option<String> {
    if node["ignored"].as_bool().unwrap_or(false) {
        return None;
    }
    let role = node["role"]["value"].as_str().unwrap_or_default();
    // 纯结构性的容器对"点哪儿"没有帮助，滤掉能省下大量篇幅。
    if matches!(role, "" | "none" | "generic" | "InlineTextBox" | "StaticText") {
        return None;
    }
    let name = node["name"]["value"].as_str().unwrap_or_default().trim();
    if name.is_empty() {
        return Some(role.to_owned());
    }
    // 名字可能很长（比如一整段说明文字），截断。
    let name: String = name.chars().take(80).collect();
    Some(format!("{role} \"{name}\""))
}

/// 取 console 里累积的消息。
///
/// `[前提]` 依赖 `Log.enable` 已经开着 —— 它只回放**开启之后**的记录。
/// 页面加载期间的报错要在导航前就 enable 才抓得到，那由调用方保证。
pub async fn console(browser: &Browser) -> Result<Vec<String>, BrowserError> {
    let r = browser.cdp("Runtime.evaluate", json!({
        // CDP 没有"把历史 console 拿回来"的方法，Log.entryAdded 是推送式的。
        // 这里读的是注入脚本攒下的缓冲区，没注入过就返回空数组。
        "expression": "globalThis.__riotConsole ? JSON.stringify(globalThis.__riotConsole) : '[]'",
        "returnByValue": true,
    })).await?;

    let raw = r["result"]["value"].as_str().unwrap_or("[]");
    let list: Vec<String> = serde_json::from_str(raw).unwrap_or_default();
    Ok(list.into_iter().take(MAX_CONSOLE).collect())
}

/// 在页面里装一个 console 缓冲区。
///
/// `[约束]` 必须用 `Page.addScriptToEvaluateOnNewDocument` 而不是直接
/// `Runtime.evaluate`:后者注入的东西活不过下一次导航，而 console 报错
/// 恰恰最常出现在页面加载阶段 —— 等模型想起来看的时候早就没了。
pub async fn install_console_hook(browser: &Browser) -> Result<(), BrowserError> {
    const HOOK: &str = r"
        (() => {
          if (globalThis.__riotConsole) return;
          const buf = globalThis.__riotConsole = [];
          for (const level of ['log', 'info', 'warn', 'error']) {
            const orig = console[level].bind(console);
            console[level] = (...args) => {
              if (buf.length < 500) {
                buf.push(level + ': ' + args.map(a => {
                  try { return typeof a === 'string' ? a : JSON.stringify(a); }
                  catch { return String(a); }
                }).join(' '));
              }
              orig(...args);
            };
          }
          addEventListener('error', e => { buf.push('error: ' + e.message); });
          addEventListener('unhandledrejection', e => {
            buf.push('error: unhandled rejection: ' + e.reason);
          });
        })()
    ";
    browser
        .cdp(
            "Page.addScriptToEvaluateOnNewDocument",
            json!({ "source": HOOK }),
        )
        .await?;
    // 当前这个文档是在注入之前加载的，补一次。
    browser
        .cdp("Runtime.evaluate", json!({ "expression": HOOK }))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 结构性节点被滤掉() {
        // 一个页面里 generic / StaticText 能占到八成节点，全留下来的话
        // a11y 快照会比 DOM 还长，那这个取舍就白做了。
        for role in ["generic", "none", "InlineTextBox", "StaticText"] {
            let n = json!({ "role": { "value": role }, "name": { "value": "x" } });
            assert!(describe_node(&n).is_none(), "{role} 应当被滤掉");
        }
    }

    #[test]
    fn 可交互节点带上名字() {
        let n = json!({
            "role": { "value": "button" },
            "name": { "value": "提交" },
        });
        assert_eq!(describe_node(&n).as_deref(), Some(r#"button "提交""#));
    }

    #[test]
    fn 超长的名字会被截断() {
        // 一整段说明文字挂在某个节点上是常见的，不截断的话单个节点
        // 就能吃掉几百 token。
        let long = "很".repeat(300);
        let n = json!({ "role": { "value": "link" }, "name": { "value": long } });
        let out = describe_node(&n).expect("有描述");
        assert!(out.chars().count() < 100, "应当截断，实际 {} 字", out.chars().count());
    }

    #[test]
    fn 被忽略的节点不出现() {
        let n = json!({
            "ignored": true,
            "role": { "value": "button" },
            "name": { "value": "隐藏的" },
        });
        assert!(describe_node(&n).is_none());
    }
}
