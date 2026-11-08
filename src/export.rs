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
            // C. <from>/lib/cmake/<name>/<name>-config.toml（及 share、平铺两种）。
            candidates.push(from_p.join("lib").join("cmake").join(name).join(format!("{name}-config.toml")));
            candidates.push(from_p.join("share").join("cmake").join(name).join(format!("{name}-config.toml")));
            candidates.push(from_p.join("lib").join("cmake").join(format!("{name}-config.toml")));
            candidates.push(from_p.join("share").join("cmake").join(format!("{name}-config.toml")));
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
    let is_layout = |p: &Path| {
        p.file_name().is_some_and(|n| n == "cmake")
            && p.parent()
                .and_then(|gp| gp.file_name())
                .is_some_and(|n| n == "lib" || n == "share")
    };
    // 嵌套布局 <pfx>/lib/cmake/<name>/<file>：parent=<pfx>/lib/cmake/<name>
    if let Some(p) = parent.parent() {
        if is_layout(p) {
            if let Some(pfx) = p.parent().and_then(|gp| gp.parent()) {
                return pfx.to_path_buf();
            }
        }
    }
    // 平铺布局 <pfx>/lib/cmake/<file>：parent == lib/cmake
    if is_layout(parent) {
        if let Some(pfx) = parent.parent().and_then(|gp| gp.parent()) {
            return pfx.to_path_buf();
        }
    }
    // 其他：直接用父目录作为前缀。
    parent.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

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
