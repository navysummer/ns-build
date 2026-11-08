//! 安装导出 / 导入：CMake `install(EXPORT)` / `find_package` 的声明式等价。
//!
//! 安装时，`[[exports]]` 生成 `<name>-config.toml` 并复制到
//! `<prefix>/<to>/`（默认 `lib/cmake/<name>/`）。下游脚本用 `[[imports]]`
//! 读回该配置：其 `include_dirs` 并入所有目标，产物以绝对路径参与链接。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 写入配置的包信息。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportPackage {
    pub name: String,
    pub version: String,
    /// 安装库目录名（如 `lib64`）；默认 `lib`（旧配置无此字段时按 `lib` 处理）。
    pub libdir: String,
    pub license: String,
    pub description: String,
}

/// 导出包中的一个目标。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportedTarget {
    pub name: String,
    /// `static` | `lib` | `module` | ...
    pub kind: String,
    /// 相对 prefix 的产物文件名（如 `libcore.a`）。
    pub output: String,
    /// prefix 相对的头文件目录（默认 `["include"]`）。
    pub include_dirs: Vec<String>,
    pub defines: Vec<String>,
    pub link: Vec<String>,
}

/// 导出包配置（`<name>-config.toml`）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportConfig {
    pub package: ExportPackage,
    #[serde(rename = "targets")]
    pub targets: Vec<ExportedTarget>,
}

impl ExportConfig {
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| format!("解析导出配置失败: {e}"))
    }
}

/// 导入加载结果。
#[derive(Debug, Clone)]
pub struct LoadedExport {
    pub config: ExportConfig,
    /// 安装前缀（`include_dirs`/`output` 都相对它解析）。
    pub prefix: PathBuf,
}

/// 从 `from`（前缀目录 / 配置文件 / 包含配置的目录）加载导出配置。
/// `name` 可以为空：此时只接受显式文件路径。
pub fn load_import(base_dir: &Path, from: &str, name: &str) -> Result<LoadedExport, String> {
    let from_p = absolutize(base_dir, from);
    // A. 显式配置文件。
    if from_p.is_file() {
        let config = ExportConfig::parse(&std::fs::read_to_string(&from_p).map_err(|e| {
            format!("读取导出配置 {} 失败: {e}", from_p.display())
        })?)?;
        return Ok(LoadedExport {
            config,
            prefix: prefix_for_config_path(&from_p),
        });
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if from_p.is_dir() {
        // B. from 目录直接含 <name>-config.toml。
        if !name.is_empty() {
            candidates.push(from_p.join(format!("{name}-config.toml")));
            // C. 任意一级子目录下都可能是 <libdir>/cmake/<name>/[...]-config.toml（lib/lib64/share 等）。
            if let Ok(entries) = std::fs::read_dir(&from_p) {
                for entry in entries.flatten() {
                    let sub = entry.path();
                    if !sub.is_dir() {
                        continue;
                    }
                    candidates.push(sub.join("cmake").join(name).join(format!("{name}-config.toml")));
                    candidates.push(sub.join("cmake").join(format!("{name}-config.toml")));
                }
            }
            // 兜底常见位置（read_dir 失败时仍能找到）。
            candidates.push(from_p.join("lib/cmake").join(name).join(format!("{name}-config.toml")));
            candidates.push(from_p.join("lib64/cmake").join(name).join(format!("{name}-config.toml")));
            candidates.push(from_p.join("share/cmake").join(name).join(format!("{name}-config.toml")));
            candidates.push(from_p.join("lib/cmake").join(format!("{name}-config.toml")));
            candidates.push(from_p.join("lib64/cmake").join(format!("{name}-config.toml")));
            candidates.push(from_p.join("share/cmake").join(format!("{name}-config.toml")));
        }
    }
    for f in candidates {
        if f.is_file() {
            let config = ExportConfig::parse(&std::fs::read_to_string(&f).map_err(|e| {
                format!("读取导出配置 {} 失败: {e}", f.display())
            })?)?;
            return Ok(LoadedExport {
                config,
                prefix: prefix_for_config_path(&f),
            });
        }
    }
    if name.is_empty() {
        Err(format!(
            "导入包：from = '{from}' 是目录，但未给出包名（需 <name>-config.toml）"
        ))
    } else {
        Err(format!(
            "找不到导入包 '{name}' 的配置（from = '{from}'）；已尝试 lib/cmake、share/cmake 等位置"
        ))
    }
}

fn absolutize(base: &Path, p: &str) -> PathBuf {
    let p = Path::new(p);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

/// 推断配置文件的安装前缀：`<pfx>/lib/cmake/<name>/<name>-config.toml` → `<pfx>`。
fn prefix_for_config_path(f: &Path) -> PathBuf {
    let parent = f.parent().unwrap_or(Path::new("/"));
    // 平铺：<pfx>/lib/cmake/<name>-config.toml → parent(lib/cmake) → 上溯 2 级 = pfx? 不对：
    // parent = lib/cmake，grandparent(lib) → 上溯 2 = pfx 的上级。故平铺按 parent-parent.parent? 平铺时直接取<two levels up> lib→pfx。
    fn is_layout(p: &Path) -> bool {
        p.file_name().is_some_and(|n| n == "cmake")
            && p.parent().and_then(|gp| gp.file_name()).is_some()
    }
    // 嵌套布局 <pfx>/lib/cmake/<name>/<file>：parent=<pfx>/lib/cmake/<name>
    if let Some(p) = parent.parent() {
        if (is_layout)(p) {
            if let Some(pfx) = p.parent().and_then(|gp| gp.parent()) {
                return pfx.to_path_buf();
            }
        }
    }
    // 平铺布局 <pfx>/lib/cmake/<file>：parent == lib/cmake
    if (is_layout)(parent) {
        if let Some(pfx) = parent.parent().and_then(|gp| gp.parent()) {
            return pfx.to_path_buf();
        }
    }
    // 其他：直接用父目录作为前缀。
    parent.to_path_buf()
}

/// 从目标产物文件名推出 pkg-config 的 `-l` 名：`libfoo.a` / `libfoo.so(.N)` / `libfoo.dylib` → `foo`。
pub fn pc_lib_name(output: &str) -> Option<String> {
    let stem = output.strip_prefix("lib").unwrap_or(output);
    for suf in [".so", ".a", ".dylib", ".lib"] {
        if let Some(i) = stem.find(suf) {
            let name = &stem[..i];
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// 生成 relocatable 的 pkg-config `.pc` 内容（prefix = ${pcfiledir}/../..）。
/// `libdir`/`includedir` 为安装目录名（如 `lib64`、`include64`）。
pub fn render_pc_file(
    name: &str,
    description: &str,
    version: &str,
    libs: &[String],
    libs_private: &[String],
    libdir: &str,
    includedir: &str,
) -> String {
    let libs_flag = libs
        .iter()
        .map(|l| format!("-l{l}"))
        .collect::<Vec<_>>()
        .join(" ");
    let private_flag = libs_private.join(" ");
    format!(
        "prefix=${{pcfiledir}}/../..\n\
         exec_prefix=${{prefix}}\n\
         libdir=${{exec_prefix}}/{libdir}\n\
         includedir=${{prefix}}/{includedir}\n\
         \n\
         Name: {name}\n\
         Description: {description}\n\
         Version: {version}\n\
         Libs: -L${{libdir}} {libs_flag}\n\
         Libs.private: {private_flag}\n\
         Cflags: -I${{includedir}}\n"
    )
}

/// pkg-config 的 Libs.private 行：裸库名 → `-lname`，其余原样保留。
pub fn pc_private_lib(l: &str) -> String {
    if l.contains('/')
        || l.contains('\\')
        || l.starts_with('-')
        || [".so", ".a", ".dylib", ".lib", ".dll"]
            .iter()
            .any(|s| l.to_lowercase().ends_with(s))
    {
        l.to_string()
    } else {
        format!("-l{l}")
    }
}

/// 比较 `a` 与 `b` 的半语义版本（按 `.` 分段的数字比较，缺段按 0）。
/// 返回 std::cmp::Ordering 风格的 -1/0/1。
fn cmp_version(a: &str, b: &str) -> Result<i64, String> {
    fn parse(v: &str) -> Result<Vec<i64>, String> {
        let mut out = Vec::new();
        for seg in v.trim().split('.') {
            let n: i64 = seg
                .parse()
                .map_err(|_| format!("版本 '{v}' 含非数字段 '{seg}'"))?;
            out.push(n);
        }
        if out.is_empty() {
            return Err(format!("版本 '{v}' 为空"));
        }
        Ok(out)
    }
    let (a, b) = (parse(a)?, parse(b)?);
    let n = a.len().max(b.len());
    for i in 0..n {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return Ok(if x < y { -1 } else { 1 });
        }
    }
    Ok(0)
}

/// 检查 `actual` 是否满足约束 `constraint`：
/// `>=` / `<=` / `>` / `<` / `=` / `!=`，不带操作符等价 `=`。
pub(crate) fn version_matches(constraint: &str, actual: &str) -> Result<bool, String> {
    let c = constraint.trim();
    // 提取操作符前缀。
    let (op, rhs) = [">=", "<=", "!=", ">", "<", "="]
        .iter()
        .find_map(|p| c.strip_prefix(p).map(|r| (*p, r.trim())))
        .unwrap_or(("=", c));
    if rhs.is_empty() {
        return Err(format!("版本约束 '{constraint}' 缺少版本号"));
    }
    let ord = cmp_version(actual, rhs)?;
    Ok(match op {
        ">=" => ord >= 0,
        "<=" => ord <= 0,
        ">" => ord > 0,
        "<" => ord < 0,
        "=" => ord == 0,
        "!=" => ord != 0,
        _ => return Err(format!("未知操作符 '{op}'")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_constraint_matches() {
        assert!(version_matches(">=1.2.0", "1.2.3").unwrap());
        assert!(version_matches(">=1.2.0", "1.2.0").unwrap());
        assert!(!version_matches(">=2.0.0", "1.2.3").unwrap());
        assert!(!version_matches(">1.0.0", "1.0.0").unwrap());
        assert!(version_matches("<=1.2.3", "1.2.3").unwrap());
        assert!(version_matches("<2.0.0", "1.2.3").unwrap());
        assert!(version_matches("=1.2.3", "1.2.3").unwrap());
        assert!(version_matches("1.2.3", "1.2.3").unwrap());
        assert!(!version_matches("!=1.0.0", "1.0.0").unwrap());
        assert!(version_matches("!=2.0.0", "1.0.0").unwrap());
        // 段数不齐也按 0 补齐比较。
        assert!(version_matches(">=1.2", "1.2.0").unwrap());
        assert!(!version_matches(">=1.2.1", "1.2").unwrap());
        assert!(version_matches(">=abc", "1.0.0").is_err());
        assert!(version_matches(">=", "1.0.0").is_err());
    }

    #[test]
    fn prefix_nested_layout() {
        let p = prefix_for_config_path(Path::new("/opt/sdk/lib/cmake/mylib/mylib-config.toml"));
        assert_eq!(p, PathBuf::from("/opt/sdk"));
    }

    #[test]
    fn prefix_share_layout() {
        let p = prefix_for_config_path(Path::new("/opt/sdk/share/cmake/mylib/mylib-config.toml"));
        assert_eq!(p, PathBuf::from("/opt/sdk"));
    }

    #[test]
    fn prefix_flat_layout() {
        let p = prefix_for_config_path(Path::new("/opt/sdk/lib/cmake/mylib-config.toml"));
        assert_eq!(p, PathBuf::from("/opt/sdk"));
    }

    #[test]
    fn prefix_plain_dir() {
        let p = prefix_for_config_path(Path::new("/opt/sdk/mylib-config.toml"));
        assert_eq!(p, PathBuf::from("/opt/sdk"));
    }

    #[test]
    fn config_roundtrips() {
        let cfg = ExportConfig {
            package: ExportPackage {
                name: "libx".into(),
                version: "1.2.3".into(),
                libdir: "lib".into(),
                license: String::new(),
                description: String::new(),
            },
            targets: vec![ExportedTarget {
                name: "core".into(),
                kind: "static".into(),
                output: "libcore.a".into(),
                include_dirs: vec!["include".into()],
                defines: vec!["X=1".into()],
                link: vec!["m".into()],
            }],
        };
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back = ExportConfig::parse(&text).unwrap();
        assert_eq!(back.package.name, "libx");
        assert_eq!(back.targets[0].output, "libcore.a");
        assert_eq!(back.targets[0].defines, vec!["X=1".to_string()]);
    }
}
