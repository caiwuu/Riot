//! Windows spawn 的命令行与环境块拼接。
//!
//! `CreateProcessW` 系接的是**一整条命令行字符串**（不是 argv 数组）和
//! 一个 `\0` 分隔的环境块。把 `ProcessSpec` 的 argv / env 拼成它们是纯
//! 字符串逻辑，而这里有 Windows 著名的坑（引号里的反斜杠反解规则），
//! 值得独立成跨平台可测的一块 —— 拼错的表现是子进程收到的参数和我们
//! 以为的不一样，隔离 check 看不出来，真机上也未必立刻炸。
//!
//! FFI 主体（`CreateProcessAsUserW`）在 [`crate::sandbox_win`]，用这里的
//! 产物。

#![allow(dead_code)] // 只被 Windows 的 spawn 用；非 Windows 平台是死代码。

/// 按 Windows 的 `CommandLineToArgvW` 反解规则给单个参数加引号。
///
/// 规则出自微软 "Everyone quotes command line arguments the wrong way"：
/// - 不含空格 / tab / 引号 / 换行的参数原样输出；
/// - 否则用双引号包起来，内部按下面两条处理反斜杠与引号。
///
/// 反斜杠只有在**紧贴引号**时才有转义含义，所以：
/// - 引号（含闭合引号）前的一串 n 个反斜杠要变成 2n 个（闭合引号前）
///   或 2n+1 个（后面跟字面引号）；
/// - 不贴引号的反斜杠原样。
///
/// 拼错的经典后果：`C:\path\` 结尾的反斜杠把闭合引号转义掉，参数吞掉
/// 后面的内容。这个函数就是为了不出那个错。
pub fn quote_arg(arg: &str) -> String {
    let needs_quote = arg.is_empty()
        || arg
            .chars()
            .any(|c| c == ' ' || c == '\t' || c == '"' || c == '\n' || c == '\u{0b}');
    if !needs_quote {
        return arg.to_owned();
    }

    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => {
                backslashes += 1;
            }
            '"' => {
                // 贴着引号的反斜杠 double，再加一个转义的引号。
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                // 不贴引号的反斜杠原样吐出。
                out.extend(std::iter::repeat_n('\\', backslashes));
                backslashes = 0;
                out.push(c);
            }
        }
    }
    // 结尾的反斜杠在闭合引号前，全部 double（否则会把闭合引号转义掉）。
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

/// 把 program + args 拼成 `CreateProcessW` 的命令行字符串。
///
/// program 本身也要 quote —— 路径里有空格（`C:\Program Files\...`）是
/// Windows 上的常态。
pub fn build_command_line(program: &str, args: &[String]) -> String {
    let mut parts = Vec::with_capacity(args.len() + 1);
    parts.push(quote_arg(program));
    parts.extend(args.iter().map(|a| quote_arg(a)));
    parts.join(" ")
}

/// 拼 `CreateProcessW` 的环境块：`KEY=VALUE\0KEY=VALUE\0...\0`（UTF-16，
/// 末尾双 `\0`）。
///
/// `base` 是继承来的环境（通常当前进程的），`overrides` 是本次要覆盖 /
/// 新增的（`ProcessSpec::env`，含 Bash 工具注入的 `GIT_EDITOR` 等）。
///
/// `[约束]` 环境变量名在 Windows 上**大小写不敏感**：`Path` 和 `PATH`
/// 是同一个。合并时按大写 key 去重，否则会出现两条 `PATH`，
/// `CreateProcess` 的行为在有重复 key 时未定义。
///
/// `[约束]` 结果按 key 排序。Windows 要求环境块按名字排序（历史上
/// 大小写不敏感序），不排序某些程序读环境会出错。排序也让输出确定、可测。
pub fn build_env_block(base: &[(String, String)], overrides: &[(String, String)]) -> Vec<u16> {
    use std::collections::BTreeMap;

    // key 的大写形态做去重键，value 连同原始 key 一起存 —— 保留原始
    // 大小写给子进程看（`Path` vs `PATH` 对读取方无所谓，但没必要改写它）。
    let mut merged: BTreeMap<String, (String, String)> = BTreeMap::new();
    for (k, v) in base.iter().chain(overrides.iter()) {
        // 跳过空 key（`=C:` 这类驱动器当前目录的隐藏变量另说，这里不透传）。
        if k.is_empty() {
            continue;
        }
        merged.insert(k.to_uppercase(), (k.clone(), v.clone()));
    }

    let mut block: Vec<u16> = Vec::new();
    for (_, (k, v)) in merged {
        block.extend(format!("{k}={v}").encode_utf16());
        block.push(0);
    }
    block.push(0); // 收尾的空串 = 块结束
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 简单参数不加引号() {
        assert_eq!(quote_arg("hello"), "hello");
        assert_eq!(quote_arg("--flag=value"), "--flag=value");
    }

    #[test]
    fn 含空格的参数加引号() {
        assert_eq!(quote_arg("a b"), r#""a b""#);
        assert_eq!(quote_arg("C:\\Program Files\\x"), r#""C:\Program Files\x""#);
    }

    #[test]
    fn 不含空格就不引用_结尾反斜杠无害() {
        // 不需引用时结尾反斜杠原样（没有闭合引号可转义）。
        assert_eq!(quote_arg(r"C:\path\"), r"C:\path\");
    }

    #[test]
    fn 需引用时结尾反斜杠要_double() {
        // 含空格 → 要引用；此时结尾的 \ 不 double 会转义掉闭合引号（经典坑）。
        assert_eq!(quote_arg(r"a b\"), r#""a b\\""#);
        assert_eq!(quote_arg(r"a b\\"), r#""a b\\\\""#);
    }

    #[test]
    fn 内嵌引号被转义() {
        assert_eq!(quote_arg(r#"say "hi""#), r#""say \"hi\"""#);
        // 引号前的反斜杠：n 个 → 2n+1 个 + 转义引号。
        assert_eq!(quote_arg(r#"a\"b"#), r#""a\\\"b""#);
    }

    #[test]
    fn 空参数是一对空引号() {
        assert_eq!(quote_arg(""), r#""""#);
    }

    #[test]
    fn 命令行拼接_program_也要引用() {
        let cl = build_command_line("C:\\Program Files\\app.exe", &["--x".into(), "a b".into()]);
        assert_eq!(cl, r#""C:\Program Files\app.exe" --x "a b""#);
    }

    /// 环境块：`\0` 分隔、双 `\0` 收尾、按 key 排序、大小写去重。
    #[test]
    fn 环境块格式与去重() {
        let base = vec![("PATH".into(), "/usr/bin".into()), ("HOME".into(), "/h".into())];
        // 小写 path 覆盖大写 PATH（Windows 大小写不敏感）。
        let over = vec![("path".into(), "/override".into())];
        let block = build_env_block(&base, &over);

        let s = String::from_utf16(&block).expect("utf16");
        let entries: Vec<&str> = s.split('\0').filter(|e| !e.is_empty()).collect();
        // HOME 和一条 path（覆盖后），不该有两条 path。
        assert_eq!(entries.len(), 2, "大小写不敏感去重：{entries:?}");
        assert!(entries.contains(&"HOME=/h"));
        assert!(
            entries.iter().any(|e| e.eq_ignore_ascii_case("path=/override")),
            "覆盖要生效：{entries:?}"
        );
        // 收尾双 \0：最后两个 u16 都是 0。
        assert_eq!(&block[block.len() - 1..], &[0]);
        assert_eq!(block[block.len() - 2], 0, "环境块要以双 \\0 结束");
    }

    #[test]
    fn 环境块按_key_排序() {
        let base = vec![("ZED".into(), "1".into()), ("ALPHA".into(), "2".into())];
        let block = build_env_block(&base, &[]);
        let s = String::from_utf16(&block).expect("utf16");
        let first = s.split('\0').next().expect("有第一条");
        assert_eq!(first, "ALPHA=2", "要按 key 排序");
    }
}
