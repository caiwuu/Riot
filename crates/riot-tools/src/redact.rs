//! 凭证遮蔽：工具结果发给模型之前，把高置信度的密钥特征换成占位符。
//!
//! # 防的是什么
//!
//! 敏感**路径**在权限层有一张白名单（`riot-permissions::safety`），但它按
//! 文件名认：一个叫 `notes.txt` 的私钥、一段被 `cat` 进日志的 token、
//! 一个网页里泄漏的 key，都从名字上看不出来。而工具结果会进对话历史、
//! 进磁盘上的 transcript、最终发到第三方模型服务 —— 这一层是内容侧的
//! 兜底：**值不出门，事实照说**。模型仍然看得到"第 3 行有一个 AWS key"
//! （占位符标明了种类），只是拿不到值本身 —— 它的任务几乎从不需要值。
//!
//! # 边界（刻意的）
//!
//! - 只遮蔽**模型自主读到的**（工具结果）。用户亲手附上的内容
//!   （`@` 引用、粘贴进输入框）不动 —— 那是他明确的选择，和模型
//!   自己翻出来是两回事。
//! - 只认**有厂商前缀的高置信度特征**。熵检测、通用 base64、JWT 都
//!   不做：开发场景里到处是长随机串和测试 token，误报几次用户就会
//!   想关掉整层 —— 而这层没有开关，宁可窄而准。
//! - 界面照常显示原文（UiPayload 不经过这里）。这是用户自己的机器和
//!   文件，对他遮蔽毫无意义，还会妨碍他核对。
//!
//! # 为什么收口在调度器而不是各工具
//!
//! 在 [`crate::scheduler`] 的结果出口统一做，Read / Bash / Grep / WebFetch
//! / MCP 工具全部覆盖 —— 散到各工具里的话，每接一个新工具都要有人想起
//! 这件事，忘掉的那个不会有任何报错。

use std::sync::LazyLock;

use regex_lite::Regex;

/// 一类凭证特征。`label` 进占位符，让模型知道被遮的是什么。
struct Pattern {
    label: &'static str,
    re: Regex,
}

/// 高置信度特征表。每条都有厂商文档背书的固定前缀 —— 加新条目前先想
/// 清楚误报面：这里错杀一个正常字符串的代价是模型拿着 `[已遮蔽]` 干活。
static PATTERNS: LazyLock<Vec<Pattern>> = LazyLock::new(|| {
    let p = |label, re: &str| Pattern {
        label,
        re: Regex::new(re).expect("特征正则要能编译（有单测守着）"),
    };
    vec![
        // AWS Access Key ID（AKIA=长期，ASIA=临时）。16 位定长大写。
        p("AWS 密钥", r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b"),
        // GitHub：经典 token（ghp_/gho_/ghu_/ghs_/ghr_，36 位）与
        // fine-grained（github_pat_，22+ 位）。
        p("GitHub token", r"\bgh[pousr]_[A-Za-z0-9]{36}\b"),
        p("GitHub token", r"\bgithub_pat_[A-Za-z0-9_]{22,}\b"),
        // OpenAI / Anthropic 风格（sk-、sk-proj-、sk-ant-）。32 位起 ——
        // 短的 sk- 前缀词（sk-learn 之类）够不着这个长度。
        p("API key", r"\bsk-[A-Za-z0-9_\-]{32,}\b"),
        // Stripe。live 和 test 都遮：test key 虽然打不了款，但它一样是
        // 不该进第三方对话的账号凭证。
        p("Stripe key", r"\b[sr]k_(?:live|test)_[A-Za-z0-9]{16,}\b"),
        // Slack（bot/app/user/legacy）。
        p("Slack token", r"\bxox[baprs]-[A-Za-z0-9\-]{10,}\b"),
        // Google API key。AIza + 35 位定长。
        p("Google API key", r"\bAIza[0-9A-Za-z_\-]{35}\b"),
    ]
});

/// PEM 私钥块（RSA/EC/DSA/OPENSSH/PGP/ENCRYPTED …）。整块匹配，
/// 替换时保留 BEGIN/END 行 —— 模型要知道"这是一把什么钥匙"。
static PEM_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"-----BEGIN ([A-Z0-9 ]*)PRIVATE KEY-----[\s\S]*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
    )
    .expect("PEM 正则要能编译")
});

/// 私钥块的头行。配合手动的"后面有没有 END"判断处理**未闭合**的块
/// （文件被截断、Read 限行切掉了 END）—— 只认完整块的话，密钥体就
/// 跟着截断漏出去了。不能用一条 `BEGIN…[\s\S]*$` 正则：regex-lite
/// 没有前瞻，它会把已经遮蔽好的完整块连 END 行一起再吞一遍。
static PEM_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----").expect("PEM 正则要能编译")
});

/// 扫一段给模型的文本，遮蔽命中的凭证。
///
/// 没命中返回 `None`（常态路径零分配）；命中返回替换后的文本和一句给
/// 模型的说明 —— 不说明的话，模型看到 `[已遮蔽]` 会以为是文件内容，
/// 可能换个工具再试一遍。
pub fn redact_secrets(text: &str) -> Option<String> {
    // 便宜的预筛：绝大多数结果连一个候选前缀都没有。
    const HINTS: &[&str] = &[
        "AKIA",
        "ASIA",
        "gh",
        "sk-",
        "sk_",
        "rk_",
        "xox",
        "AIza",
        "PRIVATE KEY",
    ];
    if !HINTS.iter().any(|h| text.contains(h)) {
        return None;
    }

    let mut out = text.to_owned();
    let mut kinds: Vec<&'static str> = Vec::new();

    // 完整 PEM 块先遮（保留头尾行），再兜未闭合的块。
    if PEM_BLOCK.is_match(&out) {
        out = PEM_BLOCK
            .replace_all(&out, "-----BEGIN ${1}PRIVATE KEY-----\n[已遮蔽：私钥内容]\n-----END ${1}PRIVATE KEY-----")
            .into_owned();
        kinds.push("私钥");
    }
    // 未闭合块 = 最后一个 BEGIN 头之后再没有 END 行（完整块刚被上面
    // 换掉了，它们的 END 还在，不会误伤）。从那个头遮到文本结尾。
    if let Some(m) = PEM_HEADER.find_iter(&out).last()
        && !out[m.end()..].contains("-----END ")
    {
        out.truncate(m.end());
        out.push_str("\n[已遮蔽：私钥内容（块未闭合，已遮到结尾）]");
        if !kinds.contains(&"私钥") {
            kinds.push("私钥");
        }
    }

    for pat in PATTERNS.iter() {
        if pat.re.is_match(&out) {
            out = pat
                .re
                .replace_all(&out, format!("[已遮蔽：{}]", pat.label))
                .into_owned();
            if !kinds.contains(&pat.label) {
                kinds.push(pat.label);
            }
        }
    }

    if kinds.is_empty() {
        return None;
    }
    out.push_str(&format!(
        "\n\n[安全提示：以上结果中检测到疑似凭证（{}），值已在发给你之前遮蔽，\
         文件本身没有被改动。不要尝试用别的工具把值读出来 —— 会被同样遮蔽；\
         需要用到值本身的操作，请让用户自己完成。]",
        kinds.join("、"),
    ));
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn 没有凭证的文本原样通过() {
        assert_eq!(redact_secrets("fn main() { println!(\"hello\"); }"), None);
        assert_eq!(redact_secrets(""), None);
        // 有提示词但对不上完整特征：不该误伤。
        assert_eq!(redact_secrets("用 sk-learn 训练，github 上有例子"), None);
    }

    #[test]
    fn aws_密钥被遮蔽并附说明() {
        let out = redact_secrets("export AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE").expect("该命中");
        assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"), "值不能出门：{out}");
        assert!(out.contains("[已遮蔽：AWS 密钥]"), "{out}");
        assert!(
            out.contains("安全提示"),
            "要给模型说明，否则它会重试：{out}"
        );
    }

    #[test]
    fn 完整_pem_块保留头尾遮蔽内容() {
        let pem = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\nQyNTUxOQAAACBd\n-----END OPENSSH PRIVATE KEY-----";
        let out = redact_secrets(pem).expect("该命中");
        assert!(
            out.contains("-----BEGIN OPENSSH PRIVATE KEY-----"),
            "头要留着：{out}"
        );
        assert!(
            out.contains("-----END OPENSSH PRIVATE KEY-----"),
            "尾要留着：{out}"
        );
        assert!(
            !out.contains("b3BlbnNzaC1rZXktdjEAAAAA"),
            "密钥体不能出门：{out}"
        );
    }

    /// Read 按行数截断可能正好切掉 END 行 —— 未闭合的块要从 BEGIN 遮到底。
    #[test]
    fn 截断的_pem_块也遮() {
        let cut =
            "前文\n-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA7qF3\nMIIEowIBAAKCAQEA7qF4";
        let out = redact_secrets(cut).expect("该命中");
        assert!(!out.contains("MIIEowIBAAKCAQEA7qF3"), "{out}");
        assert!(out.contains("前文"), "无关内容不动：{out}");
        assert!(out.contains("未闭合"), "要说明遮的是截断块：{out}");
    }

    #[test]
    fn 多种凭证一起遮_说明合并() {
        let text = "key1=AKIAIOSFODNN7EXAMPLE\ntoken=ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let out = redact_secrets(text).expect("该命中");
        assert!(out.contains("[已遮蔽：AWS 密钥]"), "{out}");
        assert!(out.contains("[已遮蔽：GitHub token]"), "{out}");
        assert_eq!(out.matches("安全提示").count(), 1, "说明只附一次：{out}");
    }

    #[test]
    fn openai_风格长_key_被遮_短前缀词不误伤() {
        let hit = "OPENAI_API_KEY=sk-proj-AbCdEfGhIjKlMnOpQrStUvWxYz012345678901";
        let out = redact_secrets(hit).expect("该命中");
        assert!(out.contains("[已遮蔽：API key]"), "{out}");
        // 32 位以下的 sk- 串是正常词汇（sk-learn、sk-1 之类），不遮。
        assert_eq!(redact_secrets("装个 sk-learn 再说"), None);
        assert_eq!(redact_secrets("sk-abc123"), None);
    }

    #[test]
    fn slack_stripe_google_都认() {
        for (text, label) in [
            ("xoxb-1234567890-abcdefghij", "Slack token"),
            ("sk_live_AbCdEfGhIjKlMnOp", "Stripe key"),
            ("AIzaSyA1234567890abcdefghijklmnopqrstuv", "Google API key"),
        ] {
            let out = redact_secrets(text).unwrap_or_else(|| panic!("该命中：{text}"));
            assert!(out.contains(&format!("[已遮蔽：{label}]")), "{out}");
        }
    }

    /// 占位符本身说清了种类 —— 模型仍然知道"这里有一把什么钥匙"，
    /// 能如实告诉用户，只是拿不到值。
    #[test]
    fn 遮蔽后上下文仍可读() {
        let out = redact_secrets("AWS_KEY=AKIAIOSFODNN7EXAMPLE 在 .env 第 3 行").expect("该命中");
        assert!(out.contains("在 .env 第 3 行"), "周围文本要完整保留：{out}");
    }
}
