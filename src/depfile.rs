//! 解析编译器生成的 Makefile 风格依赖文件（`-MMD` 的 `.d`）。
//!
//! 格式示例：
//! ```text
//! build/obj/foo.o: src/foo.c src/foo.h \
//!   include/bar.h
//! ```

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// 解析 `.d` 文件，返回目标与其依赖（源文件 + 头文件）。
pub fn parse_depfile(path: &Path) -> Result<BTreeSet<PathBuf>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("读取依赖文件 {} 失败: {e}", path.display()))?;
    parse_depfile_str(&text)
}

/// 从字符串解析（跳过目标前缀，`\` 续行）。
pub fn parse_depfile_str(text: &str) -> Result<BTreeSet<PathBuf>, String> {
    // 去掉续行符，规范化空白。
    let joined = text.replace("\\\n", " ").replace("\\\r\n", " ");
    let mut deps = BTreeSet::new();

    for line in joined.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // 找到目标与依赖的分隔符 `:`。
        let Some(colon) = find_colon(line) else {
            continue;
        };
        let rhs = &line[colon + 1..];
        for tok in split_make_tokens(rhs) {
            if !tok.is_empty() {
                deps.insert(PathBuf::from(tok));
            }
        }
    }
    Ok(deps)
}

/// 按 Makefile 规则拆分依赖 token：支持 `\ `（转义空格）与 `$$`（美元符）。
fn split_make_tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // Make 用反斜杠转义空格等字符；其余反斜杠（如 Windows 路径）
            // 原样保留。
            '\\' => match chars.peek().copied() {
                Some(n) if n.is_whitespace() || matches!(n, '#' | ':' | '\\' | '$') => {
                    chars.next();
                    cur.push(n);
                }
                _ => cur.push('\\'),
            },
            '$' if chars.peek() == Some(&'$') => {
                chars.next();
                cur.push('$');
            }
            c if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 查找非 Windows 盘符的冒号（`C:\...` 的冒号不是分隔符）。
fn find_colon(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b':' {
            // 跳过盘符：冒号前是单字母且再前一位是 `\` 或 `/` 或开头。
            let is_drive = i == 1 || (i >= 2 && (bytes[i - 2] == b'\\' || bytes[i - 2] == b'/'));
            if is_drive && bytes[0].is_ascii_alphabetic() {
                continue;
            }
            return Some(i);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_line() {
        let deps = parse_depfile_str("obj/a.o: src/a.c src/a.h\n").unwrap();
        assert!(deps.contains(&PathBuf::from("src/a.c")));
        assert!(deps.contains(&PathBuf::from("src/a.h")));
        assert_eq!(deps.len(), 2);
    }

    #[test]
    fn parses_continuation() {
        let deps = parse_depfile_str("a.o: b.c \\\n  b.h \\\n  c.h\n").unwrap();
        assert_eq!(deps.len(), 3);
    }

    #[test]
    fn parses_windows_drive() {
        let deps = parse_depfile_str("C:\\build\\a.o: C:\\src\\a.c C:\\src\\a.h\n").unwrap();
        assert!(deps.contains(&PathBuf::from("C:\\src\\a.c")));
    }

    #[test]
    fn handles_dollar_escape() {
        let deps = parse_depfile_str("a.o: b.c $$foo.h\n").unwrap();
        assert!(deps.contains(&PathBuf::from("$foo.h")));
    }

    #[test]
    fn handles_escaped_spaces() {
        let deps = parse_depfile_str("a.o: src/a\\ b.c src/x.h\n").unwrap();
        assert!(deps.contains(&PathBuf::from("src/a b.c")));
        assert!(deps.contains(&PathBuf::from("src/x.h")));
        assert_eq!(deps.len(), 2);
    }
}
