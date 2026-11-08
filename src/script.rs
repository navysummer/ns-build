//! 构建脚本 schema：`ns-build.toml` 的解析、校验与规范化。
//!
//! 设计目标：声明式、无隐式状态、字段直白（编译器/源文件/链接库/产物），
//! 一个脚本可声明多个 target，支持分步（`kind = "object"`）与并行构建。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::platform::{Arch, Host, Os};

/// 目标种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetKind {
    /// 可执行文件。
    #[default]
    Exe,
    /// 动态库。
    Lib,
    /// 静态库（`ar`）。
    Static,
    /// 只编译成对象文件，不链接（分步构建的中间产物）。
    Object,
    /// 纯聚合节点（依赖其他 target，自身无产物）。
    Phony,
}

impl TargetKind {
    pub fn is_linkable(&self) -> bool {
        matches!(self, Self::Exe | Self::Lib | Self::Static)
    }
}

/// 构建脚本中一个命令调用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Command {
    /// 简单字符串，如 `"echo hello"`。
    Shell(String),
    /// 结构化：程序 + 参数。
    Run { program: String, args: Vec<String> },
}

/// 构建钩子：目标构建前/后运行的自定义命令。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hooks {
    pub before: Vec<Command>,
    pub after: Vec<Command>,
}

/// 顶层 `[project]` 配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Project {
    pub name: String,
    pub version: String,
    /// 源码根目录（相对脚本目录）。
    pub root: String,
    /// 产物输出目录（相对脚本目录）。
    pub out: String,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            name: "project".into(),
            version: "0.1.0".into(),
            root: ".".into(),
            out: "build".into(),
        }
    }
}

/// 可选优化/调试级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    #[default]
    Release,
    Debug,
    /// 不优化，含符号（等价 debug）。
    RelWithDebInfo,
    MinSizeRel,
}

impl Profile {
    pub fn opt_flags(&self) -> &'static [&'static str] {
        match self {
            Self::Release => &["-O2"],
            Self::Debug => &["-O0", "-g"],
            Self::RelWithDebInfo => &["-O2", "-g"],
            Self::MinSizeRel => &["-Os"],
        }
    }
}

/// 默认编译/链接设置（`[defaults]`），target 可覆盖。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Defaults {
    pub cc: String,
    pub cxx: String,
    pub ar: String,
    pub flags: Vec<String>,
    pub cflags: Vec<String>,
    pub cxxflags: Vec<String>,
    pub link_flags: Vec<String>,
    pub include_dirs: Vec<String>,
    pub defines: Vec<String>,
    pub profile: Option<Profile>,
    /// 并行度；`None` = CPU 核数。
    pub parallel: Option<usize>,
    /// unity build：`off` / `on` / `auto`（auto = ≥4 源文件自动开）。
    pub unity: String,
    pub strip: bool,
    pub warnings: bool,
    pub cache: String,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            cc: "cc".into(),
            cxx: "c++".into(),
            ar: "ar".into(),
            flags: vec![],
            cflags: vec![],
            cxxflags: vec![],
            link_flags: vec![],
            include_dirs: vec![],
            defines: vec![],
            profile: None,
            parallel: None,
            unity: "off".into(),
            strip: false,
            warnings: true,
            cache: ".ns-cache".into(),
        }
    }
}

/// 交叉编译工具链（`[[toolchains]]`）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Toolchain {
    pub name: String,
    pub cc: String,
    pub cxx: String,
    pub ar: String,
    pub os: Option<String>,
    pub arch: Option<String>,
    pub flags: Vec<String>,
    pub include_dirs: Vec<String>,
    pub link_flags: Vec<String>,
    pub sysroot: Option<String>,
}

/// 单个 target。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Target {
    pub name: String,
    pub kind: TargetKind,
    /// 编译器；覆盖 `[defaults].cc`。
    pub cc: Option<String>,
    /// 源文件 / glob，如 `["src/**/*.c"]`。
    pub sources: Vec<String>,
    /// 额外 include 目录。
    pub include_dirs: Vec<String>,
    /// 编译标志。
    pub flags: Vec<String>,
    pub cflags: Vec<String>,
    pub cxxflags: Vec<String>,
    /// 链接的库：库名（`-lfoo`）或路径。
    pub link: Vec<String>,
    /// 链接库搜索目录。
    pub link_dirs: Vec<String>,
    /// 链接标志。
    pub link_flags: Vec<String>,
    /// 宏定义（`-D`）。
    pub defines: Vec<String>,
    /// 依赖的其他 target 名。
    pub deps: Vec<String>,
    /// 产物名（相对 out 目录）；默认按 target 名 + 平台后缀。
    pub output: Option<String>,
    /// 分步构建：将源文件分为 N 组，每组编译为一个 `.o`。
    pub groups: Option<usize>,
    /// 每组源文件数（与 groups 二选一）。
    pub group_size: Option<usize>,
    /// 是否参与并行构建（默认 true）。
    pub parallel: Option<bool>,
    /// 构建钩子。
    pub hooks: Hooks,
    /// 平台过滤：仅当目标 OS 在列时构建，如 `["linux", "macos"]`。
    pub os: Vec<String>,
    /// 架构过滤。
    pub arch: Vec<String>,
    /// 覆盖优化级别。
    pub profile: Option<Profile>,
    pub strip: Option<bool>,
    /// 产物是否安装（供 `install` 用）；默认 true。
    pub install: Option<bool>,
}

/// 完整构建脚本。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildScript {
    pub project: Project,
    pub defaults: Defaults,
    pub toolchains: Vec<Toolchain>,
    pub targets: Vec<Target>,
}

/// 解析后、面向规划器的规范化目标。
#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    pub name: String,
    pub kind: TargetKind,
    pub cc: String,
    pub sources: Vec<PathBuf>,
    pub include_dirs: Vec<PathBuf>,
    pub flags: Vec<String>,
    pub cflags: Vec<String>,
    pub cxxflags: Vec<String>,
    pub link: Vec<String>,
    pub link_dirs: Vec<PathBuf>,
    pub link_flags: Vec<String>,
    pub defines: Vec<String>,
    pub deps: Vec<String>,
    pub output: PathBuf,
    pub groups: Option<usize>,
    pub group_size: Option<usize>,
    pub parallel: bool,
    pub hooks: Hooks,
    pub profile: Profile,
    pub strip: bool,
    pub install: bool,
}

/// 解析后的完整脚本（路径已相对脚本目录解析、glob 已展开、平台过滤已应用）。
#[derive(Debug, Clone)]
pub struct ResolvedScript {
    pub script: BuildScript,
    pub dir: PathBuf,
    pub out_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub targets: Vec<ResolvedTarget>,
    pub parallel: usize,
    pub host: Host,
}

impl BuildScript {
    pub fn parse(toml_text: &str) -> Result<Self, String> {
        toml::from_str::<BuildScript>(toml_text)
            .map_err(|e| format!("ns-build.toml 解析失败: {e}"))
    }

    /// 解析并规范化，应用默认值、glob 展开、平台过滤。
    pub fn resolve(&self, dir: &Path) -> Result<ResolvedScript, String> {
        let dir = dir
            .canonicalize()
            .unwrap_or_else(|_| dir.to_path_buf());
        let host = Host::current();
        let out_dir = dir.join(&self.project.out);
        let cache_dir = dir.join(&self.defaults.cache);
        let parallel = self
            .defaults
            .parallel
            .unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4));

        let mut targets = Vec::new();
        for t in &self.targets {
            if let Some(r) = self.resolve_target(t, &dir, &out_dir, &host)? {
                targets.push(r);
            }
        }
        Ok(ResolvedScript {
            script: self.clone(),
            dir,
            out_dir,
            cache_dir,
            targets,
            parallel,
            host,
        })
    }

    fn resolve_target(
        &self,
        t: &Target,
        dir: &Path,
        out_dir: &Path,
        host: &Host,
    ) -> Result<Option<ResolvedTarget>, String> {
        // 平台过滤。
        if !t.os.is_empty() {
            let os = host.os.to_string();
            if !t.os.iter().any(|x| x == &os || x == "all") {
                return Ok(None);
            }
        }
        if !t.arch.is_empty() {
            let arch = host.arch.to_string();
            if !t.arch.iter().any(|x| x == &arch || x == "all") {
                return Ok(None);
            }
        }
        if t.name.is_empty() {
            return Err("target 缺少 name".into());
        }
        // 工具链：若指定 os/arch 且匹配当前主机，用对应 cc 覆盖。
        let mut cc = if t.cc.is_some() {
            t.cc.clone().unwrap()
        } else {
            self.defaults.cc.clone()
        };
        let mut toolchain_flags: Vec<String> = vec![];
        let mut toolchain_incs: Vec<String> = vec![];
        let mut toolchain_link: Vec<String> = vec![];
        let mut sysroot: Option<&str> = None;
        for tc in &self.toolchains {
            let tc_os = tc.os.as_deref().map(Os::from_str);
            let tc_arch = tc.arch.as_deref().map(Arch::from_str);
            let match_os = tc_os.map(|o| o == host.os).unwrap_or(true);
            let match_arch = tc_arch.map(|a| a == host.arch).unwrap_or(true);
            if match_os && match_arch {
                if t.cc.is_none() && !tc.cc.is_empty() {
                    cc = tc.cc.clone();
                }
                toolchain_flags.extend(tc.flags.clone());
                toolchain_incs.extend(tc.include_dirs.clone());
                toolchain_link.extend(tc.link_flags.clone());
                if tc.sysroot.is_some() {
                    sysroot = tc.sysroot.as_deref();
                }
            }
        }

        // 展开源文件 glob。
        let mut sources = Vec::new();
        let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
        for pat in &t.sources {
            let p = dir.join(pat);
            if p.to_string_lossy().contains('*') || p.to_string_lossy().contains('?') {
                let mut matched = false;
                let pattern = p.to_string_lossy().to_string();
                let paths = glob::glob(&pattern).map_err(|e| format!("glob 错误 {pat}: {e}"))?;
                for entry in paths {
                    let path = entry.map_err(|e| format!("glob 遍历错误 {pat}: {e}"))?;
                    if path.is_file() && seen.insert(path.clone()) {
                        sources.push(path);
                        matched = true;
                    }
                }
                if !matched {
                    return Err(format!("glob '{pat}' 无匹配文件"));
                }
            } else if p.is_file() {
                if seen.insert(p.clone()) {
                    sources.push(p);
                }
            } else {
                return Err(format!("源文件不存在: {}", p.display()));
            }
        }
        sources.sort();

        // 校验：可链接目标需要源文件或依赖。
        if t.kind.is_linkable() && sources.is_empty() && t.deps.is_empty() && t.kind != TargetKind::Phony {
            return Err(format!(
                "target '{}': {} 目标需要 sources 或 deps",
                t.name, "链接"
            ));
        }

        let mut flags = vec![];
        flags.extend(self.defaults.flags.clone());
        flags.extend(toolchain_flags);
        if let Some(sr) = sysroot {
            flags.push(format!("--sysroot={sr}"));
        }
        flags.extend(t.flags.clone());

        let mut cflags = vec![];
        cflags.extend(self.defaults.cflags.clone());
        cflags.extend(t.cflags.clone());

        let mut cxxflags = vec![];
        cxxflags.extend(self.defaults.cxxflags.clone());
        cxxflags.extend(t.cxxflags.clone());

        let mut defines = vec![];
        defines.extend(self.defaults.defines.clone());
        defines.extend(t.defines.clone());

        let profile = t.profile.or(self.defaults.profile).unwrap_or_default();
        let strip = t.strip.unwrap_or(self.defaults.strip);
        let output = t.output.clone().unwrap_or_else(|| t.name.clone());

        let mut incs: Vec<PathBuf> = self
            .defaults
            .include_dirs
            .iter()
            .map(|s| dir.join(s))
            .collect();
        incs.extend(t.include_dirs.iter().map(|s| dir.join(s)));
        incs.extend(toolchain_incs.iter().map(|s| dir.join(s)));
        let link_dirs: Vec<PathBuf> = t.link_dirs.iter().map(|s| dir.join(s)).collect();

        Ok(Some(ResolvedTarget {
            name: t.name.clone(),
            kind: t.kind,
            cc,
            sources,
            include_dirs: incs,
            flags,
            cflags,
            cxxflags,
            link: t.link.clone(),
            link_dirs,
            link_flags: {
                let mut lf = toolchain_link;
                lf.extend(t.link_flags.clone());
                lf
            },
            defines,
            deps: t.deps.clone(),
            output: out_dir.join(output),
            groups: t.groups,
            group_size: t.group_size,
            parallel: t.parallel.unwrap_or(true),
            hooks: t.hooks.clone(),
            profile,
            strip,
            install: t.install.unwrap_or(true),
        }))
    }
}

/// 从目录加载并解析 `ns-build.toml`。
pub fn load(dir: &Path) -> Result<(BuildScript, PathBuf), String> {
    let path = dir.join("ns-build.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    let script = BuildScript::parse(&text)?;
    Ok((script, dir.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("nsbuild-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn parse_minimal() {
        let s = BuildScript::parse(
            r#"
[project]
name = "hello"

[[targets]]
name = "app"
kind = "exe"
sources = ["src/main.c"]
"#,
        )
        .unwrap();
        assert_eq!(s.project.name, "hello");
        assert_eq!(s.targets[0].kind, TargetKind::Exe);
        assert_eq!(s.defaults.cc, "cc");
    }

    #[test]
    fn parse_glob_and_group() {
        let s = BuildScript::parse(
            r#"
[[targets]]
name = "obj"
kind = "object"
sources = ["a.c", "b.c", "c.c"]
groups = 2
"#,
        )
        .unwrap();
        assert_eq!(s.targets[0].groups, Some(2));
        assert_eq!(s.targets[0].sources.len(), 3);
    }

    #[test]
    fn resolve_expands_glob() {
        let d = tmpdir();
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join("src/a.c"), "int a;\n").unwrap();
        std::fs::write(d.join("src/b.c"), "int b;\n").unwrap();
        let s = BuildScript::parse(
            r#"
[[targets]]
name = "app"
kind = "exe"
sources = ["src/*.c"]
"#,
        )
        .unwrap();
        let r = s.resolve(&d).unwrap();
        assert_eq!(r.targets.len(), 1);
        assert_eq!(r.targets[0].sources.len(), 2);
    }

    #[test]
    fn platform_filter_skips() {
        let s = BuildScript::parse(
            r#"
[[targets]]
name = "win"
kind = "exe"
sources = ["x.c"]
os = ["windows"]

[[targets]]
name = "any"
kind = "exe"
sources = ["y.c"]
"#,
        )
        .unwrap();
        let d = tmpdir();
        std::fs::write(d.join("x.c"), "").unwrap();
        std::fs::write(d.join("y.c"), "").unwrap();
        let r = s.resolve(&d).unwrap();
        let host_os = Host::current().os.to_string();
        if host_os == "windows" {
            assert_eq!(r.targets.len(), 2);
        } else {
            assert_eq!(r.targets.len(), 1);
            assert_eq!(r.targets[0].name, "any");
        }
    }

    #[test]
    fn parse_error_reported() {
        assert!(BuildScript::parse("not toml [").is_err());
    }
}
