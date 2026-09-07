//! 全应用共享的那一个浏览器进程。
//!
//! # 为什么会话之间必须共用一个进程
//!
//! `[约束]` profile 目录全应用只有一份（[`crate::config::browser_profile_dir`]），
//! 而 CEF 120 起在 `root_cache_path` 上建了**进程单例锁** —— 第二个进程指
//! 同一个根时会在 initialize 阶段直接退出。宿主那边看到的只是"事件流断了"，
//! 没有任何一条报错指向 profile。
//!
//! 所以"每个会话各起一个进程"和"共享登录态"这两件事只能二选一。选了后者
//! （见 `browser_profile_dir` 的取舍），进程就必须共用。
//!
//! # 共享的是进程，不是视图
//!
//! `[约束]` 每个会话仍然有自己的 [`HostBrowser`]：自己的标签页清单、活动页、
//! 快照编号、面板画面。共享到视图那一层的话，模型在 A 会话里切一次标签，
//! B 会话的面板跟着跳 —— 而两边的模型都不知道对方存在，那种现象没法倒推。
//!
//! 这一层因此只做三件事：把进程管起来、发全局唯一的标签页号、把事件按号
//! 派给对应的会话。
//!
//! # 号和路由必须在一起
//!
//! `[约束]` 标签页号由这里独占分配。留在各个 `HostBrowser` 里的话，两个会话
//! 都会从 1 开始发号 —— 而子进程只认号，于是两个会话的"第一页"在 CEF 那边
//! 是同一个 browser：两个面板画着同一个页面，谁导航都影响对方。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use riot_protocol::browser::{BrowserUnavailable, Event, TabId};
use serde_json::Value;
use tokio::sync::{Mutex, mpsc};

use super::Browser;
use super::access::HostBrowser;

/// 等 CEF 就绪的上限。进程要拉起六个子进程、初始化整个 Chromium，
/// 慢机器上几秒是正常的；超过这个数基本就是起不来了。
const READY_TIMEOUT: Duration = Duration::from_secs(30);

pub struct BrowserHub {
    /// 指回自己。事件循环要用（它比任何一次调用都活得久），
    /// 用 `Weak` 是为了让应用退出时这个结构能真的释放。
    me: Weak<Self>,
    /// 打包产物的位置。
    app: PathBuf,
    /// profile 目录。全应用一份。
    profile: PathBuf,
    /// 进程。第一次真的用到时才填上（惰性启动，见 [`HostBrowser`] 的模块注释）。
    inner: Mutex<Option<Arc<Browser>>>,
    /// 标签页号分配器。
    ///
    /// `[约束]` 只增不减，而且**崩溃重开之后也不重置**。从 1 重新发的话，
    /// 新进程的 1 号和刚消失的 1 号同号，那些按号索引的表（路由、等待者、
    /// 抓包）分不出两者 —— 旧页的延迟事件会落到新页上。
    next_tab: AtomicU32,
    /// 标签页号 → 开这一页的会话。
    ///
    /// 正常路径上条目由会话自己摘（关页、删会话时的 `close_all`）。用 `Weak`
    /// 是兜底：万一哪条路漏了摘，升不上来就是"它已经没了"，由
    /// [`Self::owner_of`] 顺手清掉，不至于让一个死会话的条目永远占着。
    ///
    /// 崩溃时要通知的会话也从这里取，不另存一张成员表：一个会话只要还记着
    /// 任何标签页状态（清单、正在等的开页、抓包、快照编号），它的号就一定
    /// 在这里 —— 号是 [`Self::claim_tab`] 发的，而发号在开页之前。反过来，
    /// 号都被摘干净的会话本来就没有要清的东西。
    routes: Mutex<HashMap<TabId, Weak<HostBrowser>>>,
}

impl BrowserHub {
    /// 直接回 `Arc`：事件循环要一个指回来的弱引用（见 [`Self::me`]），
    /// 而那个引用只能在 `Arc` 建好的同时拿到。
    pub fn new(app: PathBuf, profile: PathBuf) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            me: me.clone(),
            app,
            profile,
            inner: Mutex::new(None),
            next_tab: AtomicU32::new(1),
            routes: Mutex::default(),
        })
    }

    /// 领一个标签页号，并记下它归谁。
    ///
    /// `[约束]` 必须在发 `OpenTab` **之前**调。反过来的话，`TabOpened` 可能
    /// 在路由登记之前就到了，那条事件于是找不到归属被丢掉 —— 而开页的那一方
    /// 正在等它，表现是"新建标签页转半天然后失败"。
    pub(super) async fn claim_tab(&self, owner: Weak<HostBrowser>) -> TabId {
        let id = self.next_tab.fetch_add(1, Ordering::Relaxed);
        self.routes.lock().await.insert(id, owner);
        id
    }

    /// 这个号不再属于任何人。关页、以及开页失败时调。
    pub(super) async fn release_tab(&self, tab: TabId) {
        self.routes.lock().await.remove(&tab);
    }

    /// 这个号归谁。会话已经销毁时顺手把条目清掉。
    async fn owner_of(&self, tab: TabId) -> Option<Arc<HostBrowser>> {
        let mut routes = self.routes.lock().await;
        match routes.get(&tab).and_then(Weak::upgrade) {
            Some(owner) => Some(owner),
            None => {
                routes.remove(&tab);
                None
            }
        }
    }

    /// 活着的进程，`None` = 没起来，或者起过但已经不在了。
    ///
    /// 那些"不该为它起进程"的路径都走这里 —— 信息性查询和事件驱动的清理。
    /// 直接读 `inner` 的话，进程崩掉之后拿到的是个死句柄。
    ///
    /// `pub` 是给端到端测试用的：验"删会话之后它的页真的从共享进程里消失了"
    /// 只能拿进程级的 CDP（`Target.getTargets`）去数，而那要一个裸的进程句柄。
    pub async fn live(&self) -> Option<Arc<Browser>> {
        self.inner.lock().await.clone().filter(|b| b.alive())
    }

    /// 拿到进程，没起就起，崩了就重开。
    ///
    /// `[约束]` 整个过程持锁。并发的两次调用都发现"还没起"的话会各起一个，
    /// 而它们指向同一个 profile 目录 —— 第二个拿不到单例锁直接退出，表现为
    /// "偶尔有个工具报浏览器不可用"。会话共用一个进程之后这条更要紧：抢的
    /// 不再是同一个会话里的两次调用，而是两个会话各自的第一次调用，撞上的
    /// 概率高得多。
    ///
    /// `[约束]` 崩掉的句柄必须换掉。CEF 会崩（渲染恶意页面、显存耗尽、被
    /// 系统内存压力杀掉），而这个槽位一旦填上就没有别的地方会清它 —— 交出
    /// 死句柄的结果是**所有**会话的浏览器永久不可用，而用户唯一的出路是
    /// 重启应用。
    ///
    /// `[取舍]` 重开是惰性的：等下一次真的用到才做。崩溃常常发生在没人看的
    /// 时候（面板关着、模型早转去改代码了），那时拉起六个进程几百 MB 纯属
    /// 白付。也因此不需要退避——重开由真实调用驱动，起来就崩的循环最多跟着
    /// 调用频率转，不会自己打满 CPU。
    pub(super) async fn get(&self) -> Result<Arc<Browser>, BrowserUnavailable> {
        let mut slot = self.inner.lock().await;
        if let Some(b) = slot.as_ref() {
            if b.alive() {
                return Ok(Arc::clone(b));
            }
            // 丢掉死句柄，然后照常往下走 —— 下面那段起进程的代码不必知道
            // 这是首次启动还是崩溃之后的重开。
            tracing::warn!("浏览器进程已经不在了，重开一个");
            *slot = None;
            self.forget_crashed().await;
        }

        let (tx, mut rx) = mpsc::unbounded_channel();
        let browser = Browser::spawn(self.app.clone(), Some(self.profile.clone()), tx)
            .await
            .map_err(|e| BrowserUnavailable(e.to_string()))?;
        let browser = Arc::new(browser);

        // 事件流必须一直有人排空 —— 通道是无界的，事件会持续来。
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let acker = Arc::clone(&browser);
        let hub = self.me.clone();
        tokio::spawn(async move {
            let mut ready = Some(ready_tx);
            while let Some(ev) = rx.recv().await {
                // 进程级的两条不查路由：`Ready` 是这个进程的启动信号，
                // `Error` 是它自己报的问题，都不属于任何标签页。
                match ev {
                    Event::Ready => {
                        if let Some(t) = ready.take() {
                            let _ = t.send(());
                        }
                        continue;
                    }
                    Event::Error { message } => {
                        tracing::warn!(message, "浏览器报错");
                        continue;
                    }
                    _ => {}
                }
                // 升不上来 = 应用正在退出，这些事也就没意义了。
                let Some(hub) = hub.upgrade() else { break };
                hub.dispatch(&acker, ev).await;
            }
            // 进程没了。每个挂着的会话都要清自己那份状态 —— 只清一个的话，
            // 别的会话面板上留着一排幻影标签，而发给它们的每条命令都在
            // 子进程那边以"标签页不存在"被丢掉。
            if let Some(hub) = hub.upgrade() {
                hub.forget_crashed().await;
            }
        });

        // 等 CEF 就绪。没等到就发命令的话，命令会落在一个还没有消息循环的
        // 进程上，全部静默丢掉。
        //
        // 这一刻还没有任何标签页 —— 开页由各个会话自己驱动。
        tokio::time::timeout(READY_TIMEOUT, ready_rx)
            .await
            .map_err(|_| {
                BrowserUnavailable(format!(
                    "browser did not become ready within {}s",
                    READY_TIMEOUT.as_secs()
                ))
            })?
            .map_err(|_| BrowserUnavailable("browser exited during startup".into()))?;

        *slot = Some(Arc::clone(&browser));
        Ok(browser)
    }

    /// 进程没了 —— 路由整个作废，每个会话各自清自己记着的东西。
    ///
    /// `[约束]` 路由必须清空。那些号在新进程里一个都不存在，留着只会让旧条目
    /// 一直占着内存，而且它们指向的会话已经在下面被通知过"你那些页都没了"。
    ///
    /// `[约束]` 每个会话都要通知到，不能只清触发这次重开的那一个。别的会话
    /// 什么都没做，但它们的面板同样留着一排幻影标签 —— 点哪个都静默失败，
    /// 而那个会话里没有任何线索指向"浏览器进程刚才崩过"。
    async fn forget_crashed(&self) {
        // 先把路由整个摘下来（同时就把表清空了），再逐个通知。会话的
        // `forget_crashed` 里要拿它自己的一堆锁，持着路由锁跨 await 去拿
        // 它们是自找死锁。
        let routes = std::mem::take(&mut *self.routes.lock().await);
        let mut owners: Vec<Arc<HostBrowser>> = Vec::new();
        for weak in routes.into_values() {
            // 一个会话通常有好几页，去重免得它把自己清好几遍。会话数是
            // 个位数到几十，线性找比为此引一套按指针哈希的容器划算。
            if let Some(owner) = weak.upgrade()
                && !owners.iter().any(|o| Arc::ptr_eq(o, &owner))
            {
                owners.push(owner);
            }
        }
        for owner in owners {
            owner.forget_crashed().await;
        }
    }

    /// 把一条事件派给它归属的会话。
    ///
    /// `[约束]` 需要等待的那几条必须另起任务，不能在事件循环里 await。开一页
    /// 要等 `TabOpened`，而那条事件正是这个循环派发的 —— 在循环里等它就是
    /// 等自己，只能等到超时。
    async fn dispatch(self: &Arc<Self>, browser: &Arc<Browser>, ev: Event) {
        match ev {
            Event::TabOpened { tab } => {
                if let Some(owner) = self.owner_of(tab).await {
                    owner.on_tab_opened(tab).await;
                }
            }
            Event::TabClosed { tab } => {
                let owner = self.owner_of(tab).await;
                // 号不复用，所以摘掉之后不会再有人认领它。留着的话，那一页
                // 的延迟事件还会继续找到这个会话，而它已经不管这一页了。
                self.release_tab(tab).await;
                if let Some(owner) = owner {
                    tokio::spawn(async move { owner.forget_tab(tab).await });
                }
            }
            Event::PopupRequested {
                source,
                url,
                background,
            } => {
                if let Some(owner) = self.owner_of(source).await {
                    // 页面要求开的新页归发起它的那个会话 —— 和"在当前窗口
                    // 里点开一个链接"是同一件事，不该跑到别的会话去。
                    tokio::spawn(async move {
                        owner.open_popup(source, &url, background).await;
                    });
                }
            }
            Event::Cdp { tab, payload } => self.dispatch_cdp(browser, tab, &payload).await,
            // OSR 的帧元数据现在没人用 —— 画面走 screencast。留着不删是因为
            // 它是"渲染还活着"的独立信号，screencast 卡住时能用来分清是编码
            // 还是渲染的问题。
            Event::Frame { .. } | Event::LoadEnd { .. } | Event::LoadError { .. } => {}
            // 上面的循环已经处理掉了，走不到这里。
            Event::Ready | Event::Error { .. } => {}
        }
    }

    /// CDP 事件的派发。
    ///
    /// `[约束]` 有三件事必须**先于路由**做完，因为它们和"这一页归谁"无关，
    /// 而漏掉任何一件的后果都是页面永久卡住：
    ///
    /// - JS 对话框（alert/confirm/beforeunload）会**阻塞页面**直到有人应答。
    ///   没人应答的话，紧跟着的每一条 CDP 都超时，表现是"点了个按钮之后
    ///   浏览器整个僵住"。
    /// - screencast 帧必须 ack。Chromium 只在上一帧被确认后才发下一帧，
    ///   漏一次那一页的画面就永久停在那一帧 —— 而且不报错，看起来像页面
    ///   卡住了。哪怕这一帧属于一个已经销毁的会话也要 ack：那一页还在，
    ///   之后可能被别人接手。
    /// - Fetch 暂停的请求必须逐一放行，同样是阻塞页面。
    ///
    /// 前两件在这里就地做完，第三件在会话里做（要按它的拦截规则判），
    /// 但会话不在时这里兜底放行。
    async fn dispatch_cdp(&self, browser: &Arc<Browser>, tab: TabId, payload: &Value) {
        let method = payload
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();

        if method == "Page.javascriptDialogOpening" {
            // 不问模型、不看订阅，收到就 accept。prompt 给空文本（accept 但
            // 不填），beforeunload accept 等于允许离开，都是自动化里想要的默认。
            let _ = browser.cdp_no_wait(
                tab,
                "Page.handleJavaScriptDialog",
                serde_json::json!({ "accept": true }),
            );
            return;
        }

        if method == "Page.screencastFrame"
            && let Some(sid) = payload["params"].get("sessionId")
        {
            let _ = browser.cdp_no_wait(
                tab,
                "Page.screencastFrameAck",
                serde_json::json!({ "sessionId": sid }),
            );
        }

        match self.owner_of(tab).await {
            Some(owner) => owner.on_cdp(browser, tab, payload).await,
            // 没人认领。会话销毁和标签页真的关掉之间有一小段窗口，那期间
            // 的事件会落到这里。多数直接丢掉即可，但 paused 的请求不行 ——
            // 漏在那儿的话那个页面在关掉之前一直卡着。
            None if method == "Fetch.requestPaused" => {
                if let Some(id) = payload["params"]["requestId"].as_str() {
                    let _ = browser.cdp_no_wait(
                        tab,
                        "Fetch.continueRequest",
                        serde_json::json!({ "requestId": id }),
                    );
                }
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一个不会真的起进程的 hub。启动是惰性的，所以路径不存在无所谓 ——
    /// 这几个用例验的是发号、路由和崩溃连带，都不需要 Chromium。
    fn hub() -> Arc<BrowserHub> {
        BrowserHub::new(
            PathBuf::from("/nonexistent/riot-browser.app"),
            PathBuf::from("/nonexistent/profile"),
        )
    }

    /// 两个会话领到的号绝不能撞。
    ///
    /// 这条是把号从各个 `HostBrowser` 收到 hub 里的全部理由。各发各的话
    /// 两边都从 1 开始，而子进程只认号 —— 两个会话的"第一页"于是是同一个
    /// CEF browser：两个面板画着同一个页面，谁导航都影响对方，而两边的
    /// 模型都不知道对方存在。
    #[tokio::test]
    async fn 两个会话领到的号互不相同() {
        let hub = hub();
        let a = HostBrowser::new(Arc::clone(&hub));
        let b = HostBrowser::new(Arc::clone(&hub));

        let mut 号 = Vec::new();
        for _ in 0..3 {
            号.push(hub.claim_tab(Arc::downgrade(&a)).await);
            号.push(hub.claim_tab(Arc::downgrade(&b)).await);
        }

        let mut 去重 = 号.clone();
        去重.sort_unstable();
        去重.dedup();
        assert_eq!(去重.len(), 号.len(), "号撞了：{号:?}");
    }

    /// 事件要落回开这一页的那个会话。
    #[tokio::test]
    async fn 每个号只认自己那个会话() {
        let hub = hub();
        let a = HostBrowser::new(Arc::clone(&hub));
        let b = HostBrowser::new(Arc::clone(&hub));
        let 甲 = hub.claim_tab(Arc::downgrade(&a)).await;
        let 乙 = hub.claim_tab(Arc::downgrade(&b)).await;

        assert!(Arc::ptr_eq(&hub.owner_of(甲).await.expect("甲有主"), &a));
        assert!(Arc::ptr_eq(&hub.owner_of(乙).await.expect("乙有主"), &b));
    }

    /// 会话销毁之后，它名下的号自己作废。
    ///
    /// 删会话不再通知 hub（profile 和进程都不跟着会话走），所以这条自清理
    /// 是路由表唯一的回收途径 —— 少了它，那张表会随着"用过多少会话"单调
    /// 增长，而里面每一条都指向一个已经没了的会话。
    #[tokio::test]
    async fn 会话没了它的号跟着作废() {
        let hub = hub();
        let 短命 = HostBrowser::new(Arc::clone(&hub));
        let tab = hub.claim_tab(Arc::downgrade(&短命)).await;
        drop(短命);

        assert!(hub.owner_of(tab).await.is_none());
        assert!(
            hub.routes.lock().await.is_empty(),
            "查一次就该顺手把死条目清掉"
        );
    }

    /// 进程崩了要通知**每一个**挂着的会话。
    ///
    /// 盯着的是共享进程特有的一种失败：崩溃由 A 会话的下一次调用发现，而
    /// B 会话什么都没做、也不会自己察觉。漏掉 B 的话，它的面板上留着一排
    /// 幻影标签，点哪个都静默失败，而那个会话里没有任何线索指向"浏览器
    /// 刚才崩过"。
    #[tokio::test]
    async fn 崩溃清理通知每一个会话() {
        let hub = hub();
        let 发现的 = HostBrowser::new(Arc::clone(&hub));
        let 旁观的 = HostBrowser::new(Arc::clone(&hub));
        for h in [&发现的, &旁观的] {
            let tab = hub.claim_tab(Arc::downgrade(h)).await;
            h.假装开了一页(tab).await;
        }

        hub.forget_crashed().await;

        assert_eq!(发现的.标签页数().await, 0);
        assert_eq!(旁观的.标签页数().await, 0, "旁观的会话留下了幻影标签");
        assert!(hub.routes.lock().await.is_empty(), "路由整份都该作废");
    }

    /// 删会话要把它的号从路由里摘干净，而且只摘它自己的。
    ///
    /// 这条盯的是"删掉一个会话，别的会话的页跟着失联"：路由是按号存的，
    /// 摘错一条另一个会话的那一页就再也收不到事件 —— 帧不来、抓包不累积，
    /// 而标签栏上那一页看着完全正常。真的关掉 CEF 里的页要起进程才验得到，
    /// 在端到端用例里（`删会话之后它的标签页不留在共享进程里`）。
    #[tokio::test]
    async fn 删会话只摘自己的号() {
        let hub = hub();
        let 要删的 = HostBrowser::new(Arc::clone(&hub));
        let 留下的 = HostBrowser::new(Arc::clone(&hub));
        let 甲 = hub.claim_tab(Arc::downgrade(&要删的)).await;
        要删的.假装开了一页(甲).await;
        let 乙 = hub.claim_tab(Arc::downgrade(&留下的)).await;
        留下的.假装开了一页(乙).await;

        要删的.close_all().await;

        assert_eq!(要删的.标签页数().await, 0);
        assert!(hub.owner_of(甲).await.is_none(), "删掉的会话的号该作废");
        assert!(
            Arc::ptr_eq(&hub.owner_of(乙).await.expect("乙还有主"), &留下的),
            "别的会话的路由不能被连累"
        );
        assert_eq!(留下的.标签页数().await, 1);
    }

    /// 崩溃重开之后号要接着往下发。
    ///
    /// 从 1 重发的话，新进程的 1 号和刚消失的 1 号同号 —— 那些按号索引的表
    /// （路由、等待者、抓包、拦截规则）分不出两者，旧页的延迟事件会落到
    /// 新页上。
    #[tokio::test]
    async fn 崩溃之后号接着往下发() {
        let hub = hub();
        let h = HostBrowser::new(Arc::clone(&hub));
        let 崩之前 = hub.claim_tab(Arc::downgrade(&h)).await;

        hub.forget_crashed().await;

        assert!(hub.claim_tab(Arc::downgrade(&h)).await > 崩之前);
    }
}
