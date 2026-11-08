//! 安装规则：将构建产物复制到安装前缀。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::export::{ExportConfig, ExportPackage, ExportedTarget};
use crate::script::{ResolvedScript, ResolvedTarget, TargetKind};

/// 安装选项。
#[derive(Debug, Default, Clone, Copy)]
pub struct InstallOptions {
    /// 安装后 strip 安装副本（可执行/静态库/动态库）。
    pub strip: bool,
}

/// 安装卸载（`ns-build uninstall`）结果。
#[derive(Debug, Default, Clone)]
pub struct UninstallReport {
    /// 已删除的文件数。
    pub removed: usize,
    /// 清单中存在但磁盘已缺失的文件数。
    pub missing: usize,
    /// 清理的空目录数。
    pub dirs_removed: usize,
}

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
/// - `phony`/`interface`/`alias` 无产物
/// - `opts.strip`：strip 安装后的副本（保留构建树产物不动）
fn check_within_prefix(prefix: &Path, dest: &Path, what: &str) -> Result<(), String> {
    // dest 是相对于 prefix 的路径片段（可能含 ..）；先规范化再校验。
    let norm = crate::fsutil::normalize(dest);
    let pfx = crate::fsutil::normalize(prefix);
    if norm.starts_with(&pfx) {
        return Ok(());
    }
    Err(format!(
        "安装路径 {dest:?}（{what}）越出前缀 {pfx:?}；请使用相对 prefix 且不含绝对路径/逃逸的 '..'",
        dest = dest.display(),
        pfx = pfx.display()
    ))
}

pub fn install_all(script: &ResolvedScript, prefix: &Path, opts: InstallOptions) -> Result<InstallReport, String> {
    let mut report = InstallReport::default();
    let mut manifest: Vec<PathBuf> = Vec::new();
    let dirs = &script.script.install;
    for t in &script.targets {
        if !t.install {
            continue;
        }
        match t.kind {
            TargetKind::Exe => {
                let dest_dir = prefix.join(&dirs.bindir);
                let dest = dest_dir.join(file_name(&t.output));
                copy_if_changed(&t.output, &dest, &mut report, &mut manifest)?;
                if opts.strip {
                    strip_binary(&dest)?;
                }
            }
            TargetKind::Lib | TargetKind::Static | TargetKind::Module => {
                let dest_dir = prefix.join(&dirs.libdir);
                let dest_name = file_name(&t.output);
                let dest = dest_dir.join(&dest_name);
                copy_if_changed(&t.output, &dest, &mut report, &mut manifest)?;
                if opts.strip {
                    strip_binary(&dest)?;
                }
                // soversion：在 prefix/lib 下创建开发者符号链接（libfoo.so → libfoo.so.1）。
                #[cfg(unix)]
                if t.soversion.is_some() && t.kind != TargetKind::Static {
                    if let Some(dev) = crate::platform::Os::dev_link_name(&dest_name) {
                        if dev != dest_name {
                            let dev_path = dest_dir.join(&dev);
                            let _ = std::fs::remove_file(&dev_path);
                            std::os::unix::fs::symlink(&dest_name, &dev_path).map_err(|e| {
                                format!("创建符号链接 {} 失败: {e}", dev_path.display())
                            })?;
                            manifest.push(dev_path);
                        }
                    }
                }
            }
            TargetKind::Phony | TargetKind::Interface | TargetKind::Alias | TargetKind::Object => {}
        }
    }
    // 头文件安装规则。
    for h in &script.script.install_headers {
        let from_dir = script.dir.join(&h.from);
        let to_rel = if h.to.is_empty() { dirs.includedir.as_str() } else { &h.to };
        let to_dir = prefix.join(to_rel);
        check_within_prefix(prefix, &to_dir, "install_headers.to")?;
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
                copy_if_changed(&src, &dest, &mut report, &mut manifest)?;
            }
        }
    }
    // 安装导出：写入 `<name>-config.toml`（CMake install(EXPORT) 等价）。
    install_exports(script, prefix, &mut report, &mut manifest)?;
    // 通用文件安装规则（`[[install_files]]`）。
    for f in &script.script.install_files {
        let to_base = prefix.join(&f.to);
        check_within_prefix(prefix, &to_base, "install_files.to")?;
        if let Some(dir) = &f.dir {
            // 目录安装：递归复制，保留相对结构。
            let src_dir = script.dir.join(dir);
            if !src_dir.is_dir() {
                return Err(format!("安装目录不存在: {}", src_dir.display()));
            }
            install_directory(&src_dir, &to_base, &f.exclude, f.keep_structure, &mut report, &mut manifest)?;
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
                copy_if_changed(&src, &dest, &mut report, &mut manifest)?;
            }
        }
    }
    write_manifest(&script.script.project.name, prefix, &manifest)?;
    Ok(report)
}

/// 写入安装卸载清单：`<prefix>/share/ns-build/<name>.manifest`，每行一个相对 prefix 的路径。
fn write_manifest(
    pkg_name: &str,
    prefix: &Path,
    manifest: &[PathBuf],
) -> Result<(), String> {
    if manifest.is_empty() {
        return Ok(());
    }
    let dir = prefix.join("share").join("ns-build");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("创建清单目录 {} 失败: {e}", dir.display()))?;
    let name = sanitize_package_name(pkg_name);
    let path = dir.join(format!("{name}.manifest"));
    // 与旧清单合并：既有安装条目仍记录在案，确保卸载能清理到老版本遗留文件。
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut merged: Vec<String> = Vec::new();
    if let Ok(old_text) = std::fs::read_to_string(&path) {
        for line in old_text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            merged.push(line.to_string());
            seen.insert(line.to_string());
        }
    }
    for item in manifest {
        if let Ok(rel) = item.strip_prefix(prefix) {
            if !rel.as_os_str().is_empty() {
                let s = rel.to_string_lossy().to_string();
                if seen.insert(s.clone()) {
                    merged.push(s);
                }
            }
        }
    }
    let mut text = String::from("# ns-build install manifest (package: ");
    text.push_str(&name);
    text.push_str(")\n");
    for line in &merged {
        text.push_str(line);
        text.push('\n');
    }
    let _ = std::fs::write(&path, text);
    Ok(())
}

/// 卸载：读取 `<prefix>/share/ns-build/<name>.manifest`，删除其中记录的文件并清理空目录。
pub fn uninstall_all(prefix: &Path, name: &str) -> Result<UninstallReport, String> {
    let safe = sanitize_package_name(name);
    let manifest_path = prefix.join("share").join("ns-build").join(format!("{safe}.manifest"));
    let text = std::fs::read_to_string(&manifest_path).map_err(|_| {
        format!(
            "未找到安装清单 {}（是否先执行 `ns-build install --prefix {}`？）",
            manifest_path.display(),
            prefix.display()
        )
    })?;
    let mut report = UninstallReport::default();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let full = prefix.join(line);
        let is_file_like = full.is_file() || full.is_symlink();
        if is_file_like {
            std::fs::remove_file(&full)
                .map_err(|e| format!("删除 {} 失败: {e}", full.display()))?;
            report.removed += 1;
            dirs.push(full.parent().unwrap_or(prefix).to_path_buf());
        } else {
            report.missing += 1;
        }
    }
    // 删除清单自身。
    let _ = std::fs::remove_file(&manifest_path);
    report.removed += 1;
    // 清理空目录（最深的先删；不触碰 prefix 根本身）。
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    let mut seen: Vec<PathBuf> = Vec::new();
    for d in dirs {
        let mut cur = d;
        while cur != prefix && cur.starts_with(prefix) {
            if seen.contains(&cur) {
                break;
            }
            seen.push(cur.clone());
            match std::fs::read_dir(&cur) {
                Ok(mut entries) => {
                    if entries.next().is_none() && std::fs::remove_dir(&cur).is_ok() {
                        report.dirs_removed += 1;
                    } else {
                        break;
                    }
                }
                Err(_) => break,
            }
            cur = cur.parent().unwrap_or(prefix).to_path_buf();
        }
    }
    Ok(report)
}

/// 句柄名清理：与 package 相同的规则（alnum . - _ 保留，其余 _）。
pub fn sanitize_package_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

/// 写入安装导出的配置文件并汇报到用户可见输出。
fn install_exports(
    script: &ResolvedScript,
    prefix: &Path,
    report: &mut InstallReport,
    manifest: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let dirs = &script.script.install;
    let export_sets = &script.export_sets;
    for exp in &script.script.exports {
        if exp.name.trim().is_empty() {
            return Err("exports：name 不能为空".into());
        }
        // 选择导出目标：显式列表（校验存在）；否则所有 install 的库类目标。
        let selected: Vec<&ResolvedTarget> = if exp.targets.is_empty() {
            script
                .targets
                .iter()
                .filter(|t| {
                    t.install
                        && matches!(
                            t.kind,
                            TargetKind::Static | TargetKind::Lib | TargetKind::Module
                        )
                })
                .collect()
        } else {
            let mut v = Vec::with_capacity(exp.targets.len());
            for name in &exp.targets {
                let t = script
                    .targets
                    .iter()
                    .find(|t| &t.name == name)
                    .ok_or_else(|| format!("导出包 '{}'：target '{name}' 不存在", exp.name))?;
                if matches!(t.kind, TargetKind::Exe | TargetKind::Object) {
                    return Err(format!(
                        "导出包 '{}'：target '{name}' 是 {}，只能导出 lib/static/module",
                        exp.name,
                        match t.kind {
                            TargetKind::Exe => "可执行文件",
                            _ => "object 库",
                        }
                    ));
                }
                v.push(t);
            }
            v
        };
        let kind_name = |k: TargetKind| match k {
            TargetKind::Static => "static",
            TargetKind::Lib => "lib",
            TargetKind::Module => "module",
            TargetKind::Object => "object",
            TargetKind::Exe => "exe",
            TargetKind::Interface => "interface",
            TargetKind::Phony => "phony",
            TargetKind::Alias => "alias",
        };
        let config = ExportConfig {
            package: ExportPackage {
                name: exp.name.clone(),
                version: if exp.version.is_empty() {
                    script.script.project.version.clone()
                } else {
                    exp.version.clone()
                },
                libdir: dirs.libdir.clone(),
                license: script.script.project.license.clone(),
                description: script.script.project.description.clone(),
            },
            targets: selected
                .iter()
                .map(|t| {
                    let idx = script
                        .targets
                        .iter()
                        .position(|x| x.name == t.name)
                        .unwrap_or(0);
                    let export = &export_sets[idx];
                    ExportedTarget {
                        name: t.name.clone(),
                        kind: kind_name(t.kind).to_string(),
                        output: file_name(&t.output),
                        include_dirs: vec![dirs.includedir.clone()],
                        defines: export.defines.clone(),
                        link: export.link.clone(),
                    }
                })
                .collect(),
        };
        let to = if exp.to.is_empty() {
            format!("{}/cmake/{}", dirs.libdir, exp.name)
        } else {
            let checked = crate::fsutil::normalize(Path::new(&exp.to));
            if checked.is_absolute() || checked.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
                return Err(format!(
                    "exports '{}' 的 to='{}'：必须为相对 prefix、不含 '..' 的相对路径",
                    exp.name, exp.to
                ));
            }
            exp.to.clone()
        };
        let pkg_ver = if exp.version.is_empty() {
            script.script.project.version.clone()
        } else {
            exp.version.clone()
        };
        let to_dir = prefix.join(&to);
        std::fs::create_dir_all(&to_dir)
            .map_err(|e| format!("创建导出目录 {} 失败: {e}", to_dir.display()))?;
        let path = to_dir.join(format!("{}-config.toml", exp.name));
        let text = toml::to_string_pretty(&config)
            .map_err(|e| format!("序列化导出配置 '{}' 失败: {e}", exp.name))?;
        std::fs::write(&path, &text)
            .map_err(|e| format!("写入导出配置 {} 失败: {e}", path.display()))?;
        manifest.push(path.clone());
        println!("[ns-build] 导出包 {} -> {}", exp.name, path.display());

        // 可选：同时生成 lib/pkgconfig/<name>.pc（relocatable）。
        if exp.pc {
            let libs: Vec<String> = config
                .targets
                .iter()
                .filter_map(|t| crate::export::pc_lib_name(&t.output))
                .collect();
            let privates: Vec<String> = config
                .targets
                .iter()
                .flat_map(|t| t.link.iter())
                .map(|l| crate::export::pc_private_lib(l))
                .collect();
            let pc_dir = prefix.join(&dirs.libdir).join("pkgconfig");
            std::fs::create_dir_all(&pc_dir)
                .map_err(|e| format!("创建 pkgconfig 目录 {} 失败: {e}", pc_dir.display()))?;
            let description = if script.script.project.description.is_empty() {
                &script.script.project.name
            } else {
                &script.script.project.description
            };
            let pc = crate::export::render_pc_file(
                &exp.name,
                description,
                &pkg_ver,
                &libs,
                &privates,
                &dirs.libdir,
                &dirs.includedir,
            );
            let pc_path = pc_dir.join(format!("{}.pc", exp.name));
            std::fs::write(&pc_path, pc)
                .map_err(|e| format!("写入 pkg-config 文件 {} 失败: {e}", pc_path.display()))?;
            manifest.push(pc_path.clone());
            println!("[ns-build] 导出 pkg-config {} -> {}", exp.name, pc_path.display());
        }
        report.installed += selected.len();
    }
    Ok(())
}

fn file_name(p: &Path) -> String {
    p.file_name().and_then(|s| s.to_str()).unwrap_or("out").to_string()
}

/// 递归复制目录到目标，保留相对结构；`exclude` 为 glob 排除模式。
/// 安装后 strip 二进制/库（只影响安装副本，不动构建树产物）。
fn strip_binary(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        let out = std::process::Command::new("strip")
            .arg(path)
            .output()
            .map_err(|e| format!("运行 strip {} 失败: {e}", path.display()))?;
        if !out.status.success() {
            return Err(format!(
                "strip {} 失败: {}",
                path.display(),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn install_directory(
    src_dir: &Path,
    to_base: &Path,
    exclude: &[String],
    keep_structure: bool,
    report: &mut InstallReport,
    manifest: &mut Vec<PathBuf>,
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
            // 用 symlink_metadata：符号链接一律当作文件（复制链接本体），
            // 避免跟进目录链接导致环。
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(e) => {
                    return Err(format!("stat {} 失败: {e}", path.display()));
                }
            };
            if meta.file_type().is_dir() && !meta.file_type().is_symlink() {
                stack.push(path);
                continue;
            }
            // 符号链接：保留为链接本体直接复制（不能跟进，避免环）。
            if meta.file_type().is_symlink() {
                let dest_rel = if keep_structure {
                    rel
                } else {
                    path.file_name().map(Path::new).unwrap_or(rel)
                };
                let dest = to_base.join(dest_rel);
                #[cfg(unix)]
                {
                    let target = std::fs::read_link(&path)
                        .map_err(|e| format!("read_link {} 失败: {e}", path.display()))?;
                    let _ = std::fs::remove_file(&dest);
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent)
                            .map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
                    }
                    std::os::unix::fs::symlink(&target, &dest).map_err(|e| {
                        format!(
                            "symlink {} -> {} 失败: {e}",
                            target.display(),
                            dest.display()
                        )
                    })?;
                }
                #[cfg(not(unix))]
                {
                    // 非 unix 退化为普通复制（链接环场景主要在 unix 文件系统）。
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent)
                            .map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
                    }
                    std::fs::copy(&path, &dest).map_err(|e| {
                        format!("复制符号链接 {} -> {} 失败: {e}", path.display(), dest.display())
                    })?;
                }
                manifest.push(dest.clone());
                report.installed += 1;
                continue;
            }
            let dest_rel = if keep_structure { rel } else { path.file_name().map(Path::new).unwrap_or(rel) };
            let dest = to_base.join(dest_rel);
            copy_if_changed(&path, &dest, report, manifest)?;
        }
    }
    Ok(())
}

fn copy_if_changed(
    src: &Path,
    dest: &Path,
    report: &mut InstallReport,
    manifest: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if !src.exists() {
        return Err(format!("安装产物不存在: {}", src.display()));
    }
    manifest.push(dest.to_path_buf());
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
