//! 给人看、但不带具体语言的文本。
//!
//! 界面支持多种语言，而翻译只在前端做（词典在 `src/i18n/messages`）。
//! 宿主和内核发给前端的每一句人话 —— 错误、权限询问的标题、状态说明 ——
//! 都必须走这里的类型：一个词典键加参数，前端按当前语言查词填空。
//!
//! `[约束]` Rust 侧不出现任何界面语言的文案。给模型看的文本（工具结果、
//! 提示词）不在此列：模型不跟界面语言走，那些照旧写英文字符串。
//!
//! `[约束]` 键必须在 `src/i18n/messages/zh-CN/` 里存在。这条由本模块的
//! 测试 [`tests::every_key_used_in_rust_exists_in_dictionary`] 盯着：它扫
//! 整个 workspace 的 `.rs` 文件里 `ui_text!` / `ui_error!` 的字面量键，
//! 逐条去词典里找。漏一条就是测试失败，而不是界面上出现一个裸键名。

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// 一句给人看的话：词典键 + 占位参数。前端 `t(key, args)` 得到译文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UiText {
    /// 词典键，如 `host.session.missing`。
    pub key: String,
    /// 填进译文 `{name}` 占位符的值。全部是字符串 —— 数字由调用方格式化好。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub args: BTreeMap<String, String>,
}

impl UiText {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            args: BTreeMap::new(),
        }
    }

    pub fn arg(mut self, name: impl Into<String>, value: impl ToString) -> Self {
        self.args.insert(name.into(), value.to_string());
        self
    }
}

/// 给人看的错误：一句 [`UiText`]，外加一段可选的技术细节。
///
/// `detail` 是原始原因 —— 操作系统的错误文本、HTTP 状态、内核的原话。
/// 它不翻译（多半是英文，或者本来就是机器话），前端放在次要位置。
/// 给模型看的那一面也用它：模型读 `detail`，不读词典键。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UiError {
    #[serde(flatten)]
    pub text: UiText,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl UiError {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            text: UiText::new(key),
            detail: None,
        }
    }

    pub fn arg(mut self, name: impl Into<String>, value: impl ToString) -> Self {
        self.text = self.text.arg(name, value);
        self
    }

    pub fn detail(mut self, detail: impl ToString) -> Self {
        let d = detail.to_string();
        self.detail = if d.is_empty() { None } else { Some(d) };
        self
    }

    pub fn key(&self) -> &str {
        &self.text.key
    }
}

impl std::fmt::Display for UiError {
    /// 日志和模型用的表示。没有词典，只能给键和细节 —— 足够定位问题。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text.key)?;
        if !self.text.args.is_empty() {
            f.write_str(" {")?;
            let mut first = true;
            for (k, v) in &self.text.args {
                if !first {
                    f.write_str(", ")?;
                }
                first = false;
                write!(f, "{k}={v}")?;
            }
            f.write_str("}")?;
        }
        if let Some(d) = &self.detail {
            write!(f, ": {d}")?;
        }
        Ok(())
    }
}

impl std::error::Error for UiError {}

impl From<UiText> for UiError {
    fn from(text: UiText) -> Self {
        Self { text, detail: None }
    }
}

/// 旧数据里存的是一句话。落盘过的记录（定时任务的运行历史）在字段改成
/// [`UiError`] 之前写的是字符串，读回来不能报错 —— 包成 `legacy` 键，
/// 原话当细节，界面照样能显示。
pub fn deserialize_lenient_ui_error<'de, D>(d: D) -> Result<Option<UiError>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Either {
        Structured(UiError),
        Legacy(String),
    }
    Ok(Option::<Either>::deserialize(d)?.map(|e| match e {
        Either::Structured(e) => e,
        Either::Legacy(s) => UiError::new("host.legacy").detail(s),
    }))
}

/// 造一条 [`UiText`]：`ui_text!("host.foo", path = p, count = n)`。
///
/// 键必须是字面量 —— 词典对齐测试靠扫源码找它。
#[macro_export]
macro_rules! ui_text {
    ($key:literal $(, $name:ident = $val:expr)* $(,)?) => {{
        #[allow(unused_mut)]
        let mut t = $crate::text::UiText::new($key);
        $( t = t.arg(stringify!($name), $val); )*
        t
    }};
}

/// 造一条 [`UiError`]：`ui_error!("host.foo", path = p; detail)`。
/// 分号后面的表达式是技术细节，可省。键必须是字面量（理由同 [`ui_text!`]）。
#[macro_export]
macro_rules! ui_error {
    ($key:literal $(, $name:ident = $val:expr)* ; $detail:expr $(,)?) => {{
        #[allow(unused_mut)]
        let mut e = $crate::text::UiError::new($key);
        $( e = e.arg(stringify!($name), $val); )*
        e.detail($detail)
    }};
    ($key:literal $(, $name:ident = $val:expr)* $(,)?) => {{
        #[allow(unused_mut)]
        let mut e = $crate::text::UiError::new($key);
        $( e = e.arg(stringify!($name), $val); )*
        e
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    #[test]
    fn text_serializes_flat_with_optional_args() {
        let t = ui_text!("host.session.missing");
        assert_eq!(
            serde_json::to_value(&t).unwrap(),
            serde_json::json!({ "key": "host.session.missing" })
        );
        let t = ui_text!("host.file.tooLarge", size = 12, max = 10);
        assert_eq!(
            serde_json::to_value(&t).unwrap(),
            serde_json::json!({
                "key": "host.file.tooLarge",
                "args": { "size": "12", "max": "10" }
            })
        );
    }

    #[test]
    fn error_flattens_text_and_keeps_detail() {
        let e = ui_error!("host.update.failed"; "HTTP 503");
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            serde_json::json!({ "key": "host.update.failed", "detail": "HTTP 503" })
        );
        let e = ui_error!("host.update.failed"; "");
        assert_eq!(e.detail, None, "空细节不该占一个字段");
        assert_eq!(e.to_string(), "host.update.failed");
    }

    #[test]
    fn lenient_error_accepts_legacy_string() {
        #[derive(Deserialize)]
        struct Rec {
            #[serde(default, deserialize_with = "deserialize_lenient_ui_error")]
            error: Option<UiError>,
        }
        let old: Rec = serde_json::from_str(r#"{"error":"建不了会话"}"#).unwrap();
        let e = old.error.unwrap();
        assert_eq!(e.key(), "host.legacy");
        assert_eq!(e.detail.as_deref(), Some("建不了会话"));
        let new: Rec = serde_json::from_str(r#"{"error":{"key":"k","detail":"d"}}"#).unwrap();
        assert_eq!(new.error.unwrap().key(), "k");
        let none: Rec = serde_json::from_str(r#"{}"#).unwrap();
        assert!(none.error.is_none());
    }

    #[test]
    fn error_roundtrips() {
        let e = ui_error!("kernel.turn.busy", session = "s1"; "turn 7 running");
        let json = serde_json::to_string(&e).unwrap();
        let back: UiError = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e);
    }

    /// 词典对齐：Rust 里用过的每个键都得在 `zh-CN` 词典里。
    ///
    /// 词典是 TS 源码，这里不解析它 —— 只认 `"key":` 这一形态的属性名，
    /// 词典文件全是这个写法（见 `src/i18n/messages/zh-CN/*.ts`）。
    ///
    /// 豁免理由：这是读仓库源码的构建期契约检查，不是内核代码；它读的
    /// 就是磁盘上的真文件，注入 FileSystem 没有意义，也不参与黄金回放。
    #[test]
    #[allow(clippy::disallowed_methods)]
    fn every_key_used_in_rust_exists_in_dictionary() {
        let root = workspace_root();
        let dict_dir = root.join("src/i18n/messages/zh-CN");
        let mut dict = BTreeSet::new();
        for path in list_files(&dict_dir, "ts") {
            let src = std::fs::read_to_string(&path).unwrap();
            for line in src.lines() {
                let line = line.trim_start();
                if let Some(rest) = line.strip_prefix('"')
                    && let Some(end) = rest.find('"')
                    && rest[end + 1..].trim_start().starts_with(':')
                {
                    dict.insert(rest[..end].to_owned());
                }
            }
        }
        assert!(!dict.is_empty(), "没读到词典：{}", dict_dir.display());

        let mut used: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let this_file = Path::new(file!()).file_name().unwrap();
        for dir in ["crates", "src-tauri/src"] {
            for path in list_files(&root.join(dir), "rs") {
                // 本文件的文档和单测里有示例键，不算真用。
                if path.file_name() == Some(this_file) {
                    continue;
                }
                let src = std::fs::read_to_string(&path).unwrap();
                for key in macro_keys(&src) {
                    used.entry(key)
                        .or_default()
                        .push(path.strip_prefix(root).unwrap().display().to_string());
                }
            }
        }

        let missing: Vec<String> = used
            .iter()
            .filter(|(k, _)| !dict.contains(*k))
            .map(|(k, files)| format!("  {k}  ← {}", files.join(", ")))
            .collect();
        assert!(
            missing.is_empty(),
            "这些键在 Rust 里用了，但 src/i18n/messages/zh-CN 里没有：\n{}",
            missing.join("\n")
        );
    }

    /// 找出源码里 `ui_text!("…"` / `ui_error!("…"` 的字面量键。
    ///
    /// 注释行先剔掉，再对整段文本扫 —— rustfmt 会把长调用拆行，键落在
    /// `ui_error!(` 的下一行，逐行扫会漏。
    fn macro_keys(src: &str) -> Vec<String> {
        let code: String = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut out = Vec::new();
        for needle in ["ui_text!(", "ui_error!("] {
            let mut rest = code.as_str();
            while let Some(i) = rest.find(needle) {
                rest = &rest[i + needle.len()..];
                let rest_trim = rest.trim_start();
                if let Some(after) = rest_trim.strip_prefix('"')
                    && let Some(end) = after.find('"')
                {
                    out.push(after[..end].to_owned());
                }
            }
        }
        out
    }

    fn list_files(dir: &Path, ext: &str) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(dir) else {
            return out;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                out.extend(list_files(&p, ext));
            } else if p.extension().is_some_and(|e| e == ext) {
                out.push(p);
            }
        }
        out
    }

    fn workspace_root() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root")
    }
}
