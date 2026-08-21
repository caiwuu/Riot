//! 模型这一侧的终端访问：把 [`crate::term::Terminals`] 包成工具能用的能力。
//!
//! # 边界：自己起的，加上用户明确交出来的
//!
//! `[约束]` 默认只有本会话 spawn 出来的 id 能碰。用户自己开的 shell 里有他
//! 敲过的密码、私有仓库地址、和这次任务无关的一切 —— 那不是模型该顺手读走
//! 的东西。
//!
//! 例外只有一条：用户在面板上把某个终端**显式共享**给模型
//! （[`crate::term::Terminals::set_shared`]）。加这条是因为"我的 dev server
//! 报错了"是最日常的场景，而让他手动复制粘贴几十行日志是白费功夫。
//!
//! 这条例外的三个约束：
//!
//! - 一次一个终端，不是一个全局开关；
//! - 只能由用户点开，模型这一侧**没有**任何接口能置真 —— [`TerminalAccess`]
//!   里没有 share 方法，这是结构上的不存在，不是靠提示词劝；
//! - 共享只给读。[`TerminalAccess::kill`] 仍然只认自己起的 —— 停掉用户的
//!   shell 是破坏性的，而他共享的意思是"你看"，不是"你随便动"。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

use async_trait::async_trait;
use riot_protocol::terminal::{TerminalAccess, TerminalInfo, TerminalUnavailable};

use crate::term::Terminals;

/// 绑定一个会话的终端访问。
pub struct HostTerminal {
    terms: Terminals,
    /// 服务的工作目录 = 会话的项目根。
    cwd: PathBuf,
    /// 本会话起过的终端。跟着会话活，不能每轮重建 —— 那样上一轮起的
    /// 服务这一轮就不认了。
    owned: Mutex<HashSet<u32>>,
}

impl HostTerminal {
    pub fn new(terms: Terminals, cwd: PathBuf) -> Self {
        Self {
            terms,
            cwd,
            owned: Mutex::new(HashSet::new()),
        }
    }

    fn is_owned(&self, id: u32) -> bool {
        self.owned.lock().expect("owned 锁").contains(&id)
    }

    /// 读的门槛：自己起的，或者用户共享过的。
    fn check_read(&self, id: u32) -> Result<(), TerminalUnavailable> {
        if self.is_owned(id) || self.terms.is_shared(id) {
            return Ok(());
        }
        Err(TerminalUnavailable(format!(
            "终端 {id} 不是你起的，也没被共享给你。用户自己开的终端里可能有密码和\
             与本次任务无关的内容 —— 需要看的话，请他在终端面板上点「共享给 agent」，\
             或者把内容选中发给你。"
        )))
    }

    /// 停的门槛：**只有**自己起的。
    ///
    /// `[约束]` 共享不含这一项。用户共享的意思是"你看"，不是"你随便动" ——
    /// 停掉他正在用的 shell 会丢掉他的工作现场，而那是不可撤销的。
    fn check_kill(&self, id: u32) -> Result<(), TerminalUnavailable> {
        if self.is_owned(id) {
            return Ok(());
        }
        if self.terms.is_shared(id) {
            return Err(TerminalUnavailable(format!(
                "终端 {id} 是用户共享给你看的，你可以读它，但不能停它。\
                 需要停请他自己来。"
            )));
        }
        Err(TerminalUnavailable(format!(
            "终端 {id} 不是你起的，停不了。"
        )))
    }
}

#[async_trait]
impl TerminalAccess for HostTerminal {
    async fn spawn(&self, command: &str, title: &str) -> Result<u32, TerminalUnavailable> {
        let id = self
            .terms
            .spawn(Some(self.cwd.display().to_string()), command, title)
            .map_err(TerminalUnavailable)?;
        self.owned.lock().expect("owned 锁").insert(id);
        Ok(id)
    }

    async fn read(&self, id: u32, lines: usize) -> Result<String, TerminalUnavailable> {
        self.check_read(id)?;
        // 过了 check_read 还读不到，只剩一种情况：id 归本会话管，但条目
        // 已经没了 —— 用户在面板上把它关掉了。底层那句「这个终端已经关了」
        // 是给面板看的，对模型不够：不指路的话，它会拿着旧 id 再试一轮，
        // 或者从零猜。直接告诉它下一步。
        self.terms.read(id, lines).map_err(|_| {
            TerminalUnavailable(format!(
                "终端 {id} 已经不在了（多半是用户在面板上关掉了它），这个 id 不会复活。\
                 还需要这个服务的话，用 Bash 的 background 重新起一个。"
            ))
        })
    }

    async fn kill(&self, id: u32) -> Result<(), TerminalUnavailable> {
        self.check_kill(id)?;
        self.terms.close(id);
        self.owned.lock().expect("owned 锁").remove(&id);
        Ok(())
    }

    async fn list(&self) -> Vec<TerminalInfo> {
        let owned = self.owned.lock().expect("owned 锁").clone();
        self.terms
            .list()
            .into_iter()
            .filter(|t| owned.contains(&t.id) || t.shared)
            .map(|t| TerminalInfo {
                id: t.id,
                title: t.title,
                command: t.command,
                running: t.running,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> HostTerminal {
        HostTerminal::new(Terminals::default(), std::env::temp_dir())
    }

    /// 这条边界是这个模块存在的理由。破了它，模型就能把用户 shell 里
    /// 的历史命令连同密码一起读走。
    #[tokio::test]
    async fn 读不了不是自己起的终端() {
        let h = host();
        let err = h.read(999, 10).await.expect_err("不该给读");
        assert!(err.0.contains("不是你起的"), "{}", err.0);

        let err = h.kill(999).await.expect_err("不该给停");
        assert!(err.0.contains("不是你起的"), "{}", err.0);
    }

    /// 共享是**用户**的动作，而且只给读。
    ///
    /// 两条都要守住：不共享时读不到（默认拒绝），共享后也停不掉（共享的
    /// 语义是"你看"，不是"你随便动" —— 停掉用户正在用的 shell 会丢掉他的
    /// 工作现场，不可撤销）。
    #[tokio::test]
    async fn 用户共享的终端能读但不能停() {
        let terms = Terminals::default();
        let (ch, _probe) = crate::term::testing::probe();
        let his = terms.open(None, 80, 24, ch).expect("用户开一个终端");

        let h = HostTerminal::new(terms.clone(), std::env::temp_dir());

        // 默认：读不到，也不在清单里。
        assert!(h.read(his, 10).await.is_err(), "没共享就不该读得到");
        assert!(h.list().await.is_empty(), "没共享的终端不该出现在清单里");

        // 用户点开共享。
        terms.set_shared(his, true);
        assert!(h.read(his, 10).await.is_ok(), "共享之后该读得到");
        assert_eq!(h.list().await.len(), 1, "共享的终端要出现在清单里");

        let err = h.kill(his).await.expect_err("共享不该附带停的权力");
        assert!(err.0.contains("不能停它"), "理由要说清是为什么：{}", err.0);

        // 收回来。
        terms.set_shared(his, false);
        assert!(h.read(his, 10).await.is_err(), "撤销共享之后要立刻失效");

        terms.close(his);
    }

    /// 自己起的服务被用户关掉之后再读，报错要指路（重新起一个），
    /// 不能只说"关了"—— 模型上一轮的记忆里这个 id 还是活的，只给一句
    /// "关了"它会拿着旧 id 再试，或者从零开始猜。
    #[tokio::test]
    async fn 用户关掉的服务再读要指路重开() {
        let terms = Terminals::default();
        let h = HostTerminal::new(terms.clone(), std::env::temp_dir());
        let id = h.spawn("sleep 30", "测试服务").await.expect("起服务");

        // 用户在面板上点 ×：宿主把条目彻底移除。
        terms.close(id);

        let err = h.read(id, 10).await.expect_err("条目没了就该失败");
        assert!(err.0.contains("重新起"), "要指路而不是只说关了：{}", err.0);
        assert!(
            !err.0.contains("不是你起的"),
            "明明是它起的，语义不能串：{}",
            err.0
        );
    }

    /// 模型这一侧不能给自己开权限 —— 这靠的是 trait 上没有这个方法，
    /// 不是靠提示词劝。这个用例是那条结构性约束的说明书。
    #[test]
    fn 模型接口里没有共享方法() {
        // TerminalAccess 只有 spawn / read / kill / list 四项。
        // 有人往上加 share 方法时，这个断言的注释就是该被读到的东西。
        fn assert_no_share<T: TerminalAccess>() {}
        assert_no_share::<HostTerminal>();
    }

    #[tokio::test]
    async fn 起的服务能读能停且只列自己的() {
        let terms = Terminals::default();
        // 用户自己开的那个：模型不该看见。
        let (ch, _) = crate::term::testing::probe();
        let mine_not = terms.open(None, 80, 24, ch).expect("开终端");

        let h = HostTerminal::new(terms.clone(), std::env::temp_dir());
        // 命令按 shell 方言走:Windows 的默认 shell 是 PowerShell，没有
        // printf；echo 和 sleep（Start-Sleep 的别名）两边语义一致。
        #[cfg(not(windows))]
        const CMD: &str = "printf 'riot-service-up\\n'; sleep 30";
        #[cfg(windows)]
        const CMD: &str = "echo riot-service-up; sleep 30";
        let id = h.spawn(CMD, "测试服务").await.expect("起服务");

        let listed = h.list().await;
        assert_eq!(listed.len(), 1, "只列模型自己起的");
        assert_eq!(listed[0].id, id);
        assert_eq!(listed[0].command.as_deref(), Some(CMD));

        let seen = crate::term::testing::wait_until(|| {
            h.terms
                .read(id, 50)
                .map(|t| t.contains("riot-service-up"))
                .unwrap_or(false)
        });
        assert!(
            seen,
            "模型该读得到自己服务的输出，实际读到：{:?}",
            h.terms.read(id, 50)
        );

        h.kill(id).await.expect("停得掉");
        assert!(h.read(id, 10).await.is_err(), "停掉之后就不归它管了");
        terms.close(mine_not);
    }
}
