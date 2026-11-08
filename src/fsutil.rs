//! 文件系统工具：内容哈希、路径规范化、原子写、目录清理。

use std::path::{Path, PathBuf};

/// 计算文件内容哈希（blake3）。
pub fn hash_file(path: &Path) -> Result<String, String> {
    let data = std::fs::read(path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    Ok(blake3::hash(&data).to_hex().to_string())
}

/// 将路径规范化为相对 cwd 的可显示字符串。
pub fn display_path(path: &Path, cwd: &Path) -> String {
    let p = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let norm = normalize(&p);
    if let Ok(rel) = norm.strip_prefix(cwd) {
        rel.to_string_lossy().to_string()
    } else {
        norm.to_string_lossy().to_string()
    }
}

/// 简单路径规范化（移除 `.`，解析 `..` 不触碰文件系统）。
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        use std::path::Component::*;
        match comp {
            CurDir => {}
            ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// 原子写：先写临时文件再 rename。
pub fn atomic_write(path: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目录 {} 失败: {e}", parent.display()))?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|s| s.to_str()).unwrap_or("")
    ));
    std::fs::write(&tmp, data).map_err(|e| format!("写入 {} 失败: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("原子替换 {} 失败: {e}", path.display()))?;
    Ok(())
}

/// 删除目录（若存在）。
pub fn remove_dir_all_if_exists(path: &Path) -> Result<(), String> {
    if path.exists() {
        std::fs::remove_dir_all(path)
            .map_err(|e| format!("删除 {} 失败: {e}", path.display()))?;
    }
    Ok(())
}

/// 删除文件（若存在）。
pub fn remove_file_if_exists(path: &Path) -> Result<(), String> {
    if path.exists() {
        std::fs::remove_file(path)
            .map_err(|e| format!("删除 {} 失败: {e}", path.display()))?;
    }
    Ok(())
}

/// 文件修改时间（epoch 秒）。
pub fn mtime(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
}

/// 判断文件是否需要重编译：不存在目标，或任一依赖比目标新。
pub fn is_stale(target: &Path, deps: &[PathBuf]) -> bool {
    let Some(tm) = mtime(target) else {
        return true;
    };
    deps.iter().any(|d| match mtime(d) {
        Some(dm) => dm > tm,
        None => true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_removes_dots() {
        assert_eq!(
            normalize(Path::new("a/./b/../c")),
            PathBuf::from("a/c")
        );
    }
}
