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
/// - `exe`  → `prefix/bin/`
/// - `lib`/`static`/`module` → `prefix/lib/`
/// - `object` 是中间产物（对应 CMake OBJECT 库），不安装
/// - `phony`/`interface` 无产物
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
            TargetKind::Lib | TargetKind::Static | TargetKind::Module => {
                let dest_dir = prefix.join("lib");
                copy_if_changed(&t.output, &dest_dir.join(file_name(&t.output)), &mut report)?;
            }
            TargetKind::Phony | TargetKind::Interface | TargetKind::Object => {}
        }
    }
    // 头文件安装规则。
    for h in &script.script.install_headers {
        let from_dir = script.dir.join(&h.from);
        let to_rel = if h.to.is_empty() { "include" } else { &h.to };
        let to_dir = prefix.join(to_rel);
        for pat in &h.files {
            let pattern = from_dir.join(pat).to_string_lossy().to_string();
            let paths = glob::glob(&pattern).map_err(|e| format!("头文件 glob 错误 {pat}: {e}"))?;
            for entry in paths {
                let src = entry.map_err(|e| format!("头文件遍历错误: {e}"))?;
                if !src.is_file() {
                    continue;
                }
                let rel = src.strip_prefix(&from_dir).unwrap_or(&src);
                let dest = to_dir.join(rel);
                copy_if_changed(&src, &dest, &mut report)?;
            }
        }
    }
    // 通用文件安装规则（`[[install_files]]`）。
    for f in &script.script.install_files {
        let to_base = prefix.join(&f.to);
        if let Some(dir) = &f.dir {
            // 目录安装：递归复制，保留相对结构。
            let src_dir = script.dir.join(dir);
            if !src_dir.is_dir() {
                return Err(format!("安装目录不存在: {}", src_dir.display()));
            }
            install_directory(&src_dir, &to_base, &f.exclude, f.keep_structure, &mut report)?;
        }
        for pat in &f.files {
            let pattern = script.dir.join(pat).to_string_lossy().to_string();
            let paths = glob::glob(&pattern).map_err(|e| format!("文件 glob 错误 {pat}: {e}"))?;
            for entry in paths {
                let src = entry.map_err(|e| format!("文件遍历错误: {e}"))?;
                if !src.is_file() {
                    continue;
                }
                let name = file_name(&src);
                let dest = to_base.join(&name);
                copy_if_changed(&src, &dest, &mut report)?;
            }
        }
    }
    Ok(report)
}

fn file_name(p: &Path) -> String {
    p.file_name().and_then(|s| s.to_str()).unwrap_or("out").to_string()
}

/// 递归复制目录到目标，保留相对结构；`exclude` 为 glob 排除模式。
fn install_directory(
    src_dir: &Path,
    to_base: &Path,
    exclude: &[String],
    keep_structure: bool,
    report: &mut InstallReport,
) -> Result<(), String> {
    let mut stack = vec![src_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .map_err(|e| format!("读取目录 {} 失败: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("遍历 {} 失败: {e}", dir.display()))?;
            let path = entry.path();
            let rel = path.strip_prefix(src_dir).unwrap_or(&path);
            let excluded = exclude.iter().any(|pat| {
                glob::Pattern::new(pat)
                    .map(|p| p.matches_path(rel))
                    .unwrap_or(false)
            });
            if excluded {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let dest_rel = if keep_structure { rel } else { path.file_name().map(Path::new).unwrap_or(rel) };
            let dest = to_base.join(dest_rel);
            copy_if_changed(&path, &dest, report)?;
        }
    }
    Ok(())
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
