//! 安装规则：将构建产物复制到安装前缀。

use std::path::{Path, PathBuf};

use crate::script::{ResolvedScript, TargetKind};

/// 安装结果。
#[derive(Debug, Default, Clone)]
pub struct InstallReport {
    pub installed: usize,
    pub skipped: usize,
}

/// 将脚本中所有 install 目标复制到 prefix（默认 `/usr/local`）。
///
/// 布局：
/// - `exe`  → `prefix/bin/`
/// - `lib`/`static` → `prefix/lib/`
/// - `object` → `prefix/lib/`
pub fn install_all(script: &ResolvedScript, prefix: &Path) -> Result<InstallReport, String> {
    let mut report = InstallReport::default();
    for t in &script.targets {
        if !t.install {
            continue;
        }
        match t.kind {
            TargetKind::Exe => {
                let dest_dir = prefix.join("bin");
                copy_if_changed(&t.output, &dest_dir.join(file_name(&t.output)), &mut report)?;
            }
            TargetKind::Lib | TargetKind::Static | TargetKind::Object => {
                let dest_dir = prefix.join("lib");
                copy_if_changed(&t.output, &dest_dir.join(file_name(&t.output)), &mut report)?;
            }
            TargetKind::Phony => {}
        }
    }
    Ok(report)
}

fn file_name(p: &Path) -> String {
    p.file_name().and_then(|s| s.to_str()).unwrap_or("out").to_string()
}

fn copy_if_changed(src: &Path, dest: &Path, report: &mut InstallReport) -> Result<(), String> {
    if !src.exists() {
        return Err(format!("安装产物不存在: {}", src.display()));
    }
    // 内容相同则跳过。
    let same = dest.exists() && same_content(src, dest);
    if same {
        report.skipped += 1;
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目录 {} 失败: {e}", parent.display()))?;
    }
    std::fs::copy(src, dest).map_err(|e| format!("复制 {} → {} 失败: {e}", src.display(), dest.display()))?;
    report.installed += 1;
    Ok(())
}

fn same_content(a: &Path, b: &Path) -> bool {
    let Ok(ma) = std::fs::metadata(a) else { return false };
    let Ok(mb) = std::fs::metadata(b) else { return false };
    if ma.len() != mb.len() {
        return false;
    }
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// 默认安装前缀。
pub fn default_prefix() -> PathBuf {
    if cfg!(target_os = "windows") {
        PathBuf::from("C:/Program Files/ns-build")
    } else {
        PathBuf::from("/usr/local")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_works() {
        assert_eq!(file_name(Path::new("/a/b/hello")), "hello");
    }
}
