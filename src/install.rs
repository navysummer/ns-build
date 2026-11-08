//! 安装规则：将构建产物复制到安装前缀。

use std::path::{Path, PathBuf};

use crate::export::{ExportConfig, ExportPackage, ExportedTarget};
use crate::script::{ResolvedScript, ResolvedTarget, TargetKind};

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
                let dest_name = file_name(&t.output);
                copy_if_changed(&t.output, &dest_dir.join(&dest_name), &mut report)?;
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
                        }
                    }
                }
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
    // 安装导出：写入 `<name>-config.toml`（CMake install(EXPORT) 等价）。
    install_exports(script, prefix, &mut report)?;
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

/// 写入安装导出的配置文件并汇报到用户可见输出。
fn install_exports(
    script: &ResolvedScript,
    prefix: &Path,
    report: &mut InstallReport,
) -> Result<(), String> {
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
        };
        let config = ExportConfig {
            package: ExportPackage {
                name: exp.name.clone(),
                version: if exp.version.is_empty() {
                    script.script.project.version.clone()
                } else {
                    exp.version.clone()
                },
            },
            targets: selected
                .iter()
                .map(|t| ExportedTarget {
                    name: t.name.clone(),
                    kind: kind_name(t.kind).to_string(),
                    output: file_name(&t.output),
                    include_dirs: vec!["include".to_string()],
                    defines: t.defines.clone(),
                    link: t.link.clone(),
                })
                .collect(),
        };
        let to = if exp.to.is_empty() {
            format!("lib/cmake/{}", exp.name)
        } else {
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
            let pc_dir = prefix.join("lib").join("pkgconfig");
            std::fs::create_dir_all(&pc_dir)
                .map_err(|e| format!("创建 pkgconfig 目录 {} 失败: {e}", pc_dir.display()))?;
            let pc = crate::export::render_pc_file(
                &exp.name,
                &script.script.project.name,
                &pkg_ver,
                &libs,
                &privates,
            );
            let pc_path = pc_dir.join(format!("{}.pc", exp.name));
            std::fs::write(&pc_path, pc)
                .map_err(|e| format!("写入 pkg-config 文件 {} 失败: {e}", pc_path.display()))?;
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
