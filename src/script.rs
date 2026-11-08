//! 构建脚本 schema：`ns-build.toml` 的解析、校验与规范化。
//!
//! 设计目标：声明式、无隐式状态、字段直白（编译器/源文件/链接库/产物），
//! 一个脚本可声明多个 target，支持分步（`kind = "object"`）与并行构建。

use std::collections::{BTreeMap, BTreeSet};
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
    /// 模块库（`dlopen` 加载，类似动态库但默认不链接）。
    Module,
    /// INTERFACE 头文件库：只传递 include/宏，无产物。
    Interface,
}

impl TargetKind {
    pub fn is_linkable(&self) -> bool {
        matches!(self, Self::Exe | Self::Lib | Self::Static | Self::Module)
    }
    /// 是否产生产物（interface/phony 无产物）。
    pub fn produces_output(&self) -> bool {
        !matches!(self, Self::Phony | Self::Interface)
    }
}

/// 目标编程语言。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// C（默认）：cc/gcc/clang。
    #[default]
    C,
    /// C++：c++/g++/clang++。
    Cpp,
    /// Rust：rustc。
    Rust,
    /// Go：go build。
    Go,
    /// Java：javac + jar。
    Java,
    /// Zig：zig build-exe / build-lib。
    Zig,
    /// Python：生成可执行脚本（shebang）。
    Python,
    /// Shell：生成可执行脚本（shebang）。
    Shell,
}

impl Language {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::C => "c",
            Self::Cpp => "c++",
            Self::Rust => "rust",
            Self::Go => "go",
            Self::Java => "java",
            Self::Zig => "zig",
            Self::Python => "python",
            Self::Shell => "shell",
        }
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

impl Default for Command {
    fn default() -> Self {
        Command::Shell(String::new())
    }
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

    /// MSVC 风格优化标志。
    pub fn msvc_opt_flags(&self) -> &'static [&'static str] {
        match self {
            Self::Release => &["/O2"],
            Self::Debug => &["/Od", "/Zi"],
            Self::RelWithDebInfo => &["/O2", "/Zi"],
            Self::MinSizeRel => &["/O1"],
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
    /// 默认工具链名（`[[toolchains]].name`）。
    pub toolchain: Option<String>,
    pub strip: bool,
    pub warnings: bool,
    pub werror: bool,
    pub cache: String,
    /// 变量表：`${name}` 在脚本字符串中替换。
    pub variables: BTreeMap<String, String>,
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
            unity: "auto".into(),
            toolchain: None,
            strip: false,
            warnings: true,
            werror: false,
            cache: ".ns-cache".into(),
            variables: BTreeMap::new(),
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
    /// 编程语言（c/c++/rust/go/java/zig/python/shell）。
    pub language: Option<Language>,
    /// 指定工具链名（`[[toolchains]].name`）；覆盖 `[defaults].toolchain`。
    pub toolchain: Option<String>,
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
    /// 依赖的其他 target 名（其使用要求向下游传播；CMake PUBLIC/INTERFACE）。
    pub deps: Vec<String>,
    /// 私有依赖：参与编译与链接，但使用要求**不**向下游传播（CMake PRIVATE）。
    pub private_deps: Vec<String>,
    /// pkg-config 包名：链接时展开 `pkg-config --libs`。
    pub pkg_config: Vec<String>,
    /// 产物名（相对 out 目录）；默认按 target 名 + 平台后缀。
    pub output: Option<String>,
    /// 分步构建：将源文件分为 N 组，每组编译为一个 `.o`.
    pub groups: Option<usize>,
    /// 每组源文件数（与 groups 二选一）。
    pub group_size: Option<usize>,
    /// 是否参与并行构建（默认 true）。
    pub parallel: Option<bool>,
    /// 构建钩子。
    pub hooks: Hooks,
    /// 平台过滤：仅当目标 OS 在列时构建，如 `["linux", "macos"]`.
    pub os: Vec<String>,
    /// 架构过滤。
    pub arch: Vec<String>,
    pub profile: Option<Profile>,
    pub strip: Option<bool>,
    /// 动态库的 soversion（如 `"1"`）：Linux `libfoo.so.1` + soname、macOS `libfoo.1.dylib` + install_name。
    /// 并在 out 与 install 前缀下创建 `libfoo.so`/`libfoo.dylib` 开发者符号链接。Windows 忽略。
    pub soversion: Option<String>,
    /// 链接时写入的运行时库搜索路径（`-Wl,-rpath`）；支持 `$ORIGIN`/`${var}` 与相对路径。
    /// Windows 下忽略（无 rpath 概念）。
    pub rpath: Vec<String>,
    /// 警告即错误（-Werror / /WX）；默认取 defaults.werror。
    pub werror: Option<bool>,
    /// 产物是否安装（供 `install` 用）；默认 true。
    pub install: Option<bool>,
    /// 条件构建表达式（`when`）：如 `os == "linux" && arch == "x86_64"`.
    pub when: Option<String>,
    /// C 标准，如 `c11` / `gnu11` / `c17`.
    pub c_std: Option<String>,
    /// C++ 标准，如 `c++17` / `c++20`.
    pub cxx_std: Option<String>,
    /// 预编译头（`-include`，对应 CMake `target_precompile_headers`）。
    /// 支持单个字符串或字符串数组。
    pub pch: Option<PchValue>,
}
/// 预编译头字段的值：单个头文件或头文件列表。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PchValue {
    /// `pch = "include/pre.h"`
    One(String),
    /// `pch = ["a.h", "b.h"]`
    Many(Vec<String>),
}

impl PchValue {
    pub fn into_vec(self) -> Vec<String> {
        match self {
            Self::One(s) => vec![s],
            Self::Many(v) => v,
        }
    }
}

/// 编译器特性检测（CMake `check_cxx_compiler_flag` / `CHECK_INCLUDE_FILE` 等价）：
/// 解析期做一次 try-compile+link，结果注入 `${name}` 变量（1/0）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Check {
    /// 名字（同时用作 `${name}` 变量名）。
    pub name: String,
    /// 内联源码（与 `source_file` 二选一）。
    pub source: Option<String>,
    /// 外部源文件（相对脚本目录；与 `source` 二选一）。
    pub source_file: Option<String>,
    /// `c` | `cxx`（默认 `cxx`）。
    pub kind: String,
    /// 检测时额外的宏定义（-D/-U）。
    pub defines: Vec<String>,
    /// 检测时链接的库（-l）。
    pub libraries: Vec<String>,
    /// 成功时注入所有 target 的 `-D<define>=1`。
    pub define: Option<String>,
    /// true → 检测失败直接报错（默认 false，仅为 0）。
    pub fail_fatal: bool,
}

/// 自定义命令（CMake `add_custom_command(OUTPUT)` 等价）：
/// 生成文件，供 targets 作为源/输入使用；生命周期与缓存接入与普通任务一致。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomCommand {
    /// 名字（日志/错误消息；缺省 `custom_<索引>`）。
    pub name: String,
    /// 生成的文件（相对脚本目录；建议放 out 目录下，如 `build/gen/version.c`）。
    pub outputs: Vec<String>,
    /// 命令：字符串（`sh -c` / `cmd /C`）或 `{ program = "...", args = [...] }`。
    pub command: Command,
    /// 输入文件（相对脚本目录，支持 glob）。
    pub depends: Vec<String>,
    /// 依赖的其他自定义命令名（先于本命令执行）。
    pub deps: Vec<String>,
}

/// 构建预设（CMake `--preset` 等价）：一组可命名的配置覆盖。
/// 命令行显式传入的选项优先于预设中的值。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Preset {
    /// 优化配置覆盖。
    pub profile: Option<Profile>,
    /// 并行任务数覆盖。
    pub jobs: Option<usize>,
    /// 变量覆盖（等价 `--define`，CLI 中显式 --define 优先）。
    pub defines: BTreeMap<String, String>,
    /// unity 模式覆盖（`off` / `on` / `auto`）。
    pub unity: Option<String>,
    /// 默认工具链覆盖（`[[toolchains]].name`）。
    pub toolchain: Option<String>,
    /// 警告标志覆盖。
    pub warnings: Option<bool>,
}

/// 外部依赖拉取（CMake FetchContent / ExternalProject 等价）。
/// 由 `ns-build fetch` 执行；构建本身不需要网络。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Fetch {
    /// 包名（拉取到 `.ns-fetch/<name>/`）；仅字母数字/`-`/`_`。
    pub name: String,
    /// git 仓库地址（支持本地 `file://`，便于离线/测试）。
    pub git: Option<String>,
    /// 版本、分支或提交（仅 git；默认远程 HEAD）。
    pub rev: Option<String>,
    /// 压缩包地址（`.tar.gz`/`.tgz`/`.tar`/`.zip`；支持本地 `file://`）。
    pub url: Option<String>,
    /// 解压时去掉的目录层数（仅 tar；默认 0）。
    pub strip_components: usize,
}

/// 安装包导出（CMake `install(EXPORT)` 等价）：生成 `<name>-config.toml`。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Export {
    /// 导出包名（配置文件名 `<name>-config.toml`）。
    pub name: String,
    /// 导出的 target；空 = 所有 `install = true` 的 `static`/`lib`/`module` 目标。
    pub targets: Vec<String>,
    /// 安装目录（相对 prefix）；默认 `lib/cmake/<name>`。
    pub to: String,
    /// 包版本；默认 `project.version`。
    pub version: String,
    /// true：同时生成 `lib/pkgconfig/<name>.pc`（pkg-config，prefix 可 relocatable）。
    pub pc: bool,
}

/// 导入已安装的包（CMake `find_package` 等价）：其使用要求并入所有目标。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Import {
    /// 前缀目录、配置文件路径或包含 `<name>-config.toml` 的目录。
    pub from: String,
    /// 包名（`from` 是目录时必填；`from` 是文件时可从文件名推断）。
    pub name: String,
}

/// 头文件安装规则。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct InstallHeader {
    /// 源目录（相对脚本）。
    pub from: String,
    /// 安装到 prefix 下的相对路径（默认 `include`）。
    pub to: String,
    /// 文件/glob 模式，如 `["**/*.h"]`。
    pub files: Vec<String>,
}

/// 通用文件安装规则（对应 CMake `install(FILES/DIRECTORY)`）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct InstallFile {
    /// 源：文件或 glob 模式（相对脚本），或目录（`dir` 时）。
    pub files: Vec<String>,
    /// 源目录（相对脚本），安装整个目录（对应 `install(DIRECTORY)`）。
    pub dir: Option<String>,
    /// 安装到 prefix 下的相对路径。
    pub to: String,
    /// 复制目录时排除的 glob（对应 `PATTERN ... EXCLUDE`）。
    pub exclude: Vec<String>,
    /// 是否保留目录结构（默认 true）。
    #[serde(default = "default_true")]
    pub keep_structure: bool,
}

fn default_true() -> bool {
    true
}

/// 完整构建脚本。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildScript {
    pub project: Project,
    pub defaults: Defaults,
    pub toolchains: Vec<Toolchain>,
    pub targets: Vec<Target>,
    /// 子脚本（`include = ["sub/ns-build.toml"]`）。
    pub includes: Vec<String>,
    /// 自定义命令（`[[custom_commands]]`）。
    pub custom_commands: Vec<CustomCommand>,
    /// 编译器特性检测（`[[checks]]`），解析期执行一次，结果缓存。
    pub checks: Vec<Check>,
    /// 安装导出（`[[exports]]`）。
    pub exports: Vec<Export>,
    /// 导入已安装的包（`[[imports]]`）。
    pub imports: Vec<Import>,
    /// 外部依赖拉取（`[[fetch]]`）。
    pub fetch: Vec<Fetch>,
    /// 构建预设（`[presets]` 表，CMake `--preset` 等价）。
    pub presets: BTreeMap<String, Preset>,
    /// 头文件安装规则（`[[install_headers]]`）。
    pub install_headers: Vec<InstallHeader>,
    /// 通用文件安装规则（`[[install_files]]`）。
    pub install_files: Vec<InstallFile>,
    /// 显式测试注册（`[[tests]]`）。
    pub tests: Vec<TestSpec>,

}

/// 显式测试用例。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TestSpec {
    /// 测试名。
    pub name: String,
    /// 要运行的目标（可执行文件）。
    pub target: String,
    /// 传入参数。
    pub args: Vec<String>,
    /// 工作目录（相对脚本目录）。
    pub workdir: Option<String>,
    /// 超时秒数（对应 CMake `add_test(... TIMEOUT)`）。
    pub timeout: Option<u64>,
    /// 测试标签（对应 `LABELS`）。
    pub labels: Vec<String>,
    /// 仅当进程成功退出才算通过（默认 true）。
    pub require_success: bool,

}

/// 解析后、面向规划器的规范化目标。
#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    pub name: String,
    pub kind: TargetKind,
    /// 编程语言。
    pub language: Language,
    pub cc: String,
    /// C++ 编译器（链接 c++ 目标时用）。
    pub cxx: String,
    /// 静态库归档器。
    pub ar: String,
    /// 目标平台（交叉编译时来自工具链，否则为主机）。
    pub host: Host,
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
    pub private_deps: Vec<String>,
    pub output: PathBuf,
    pub groups: Option<usize>,
    pub group_size: Option<usize>,
    pub parallel: bool,
    pub hooks: Hooks,
    pub profile: Profile,
    pub strip: bool,
    pub warnings: bool,
    pub werror: bool,
    /// 已规范化的 rpath 目录（绝对路径或 $ORIGIN 模板，原样传递）。
    pub rpath: Vec<String>,
    /// 动态库 soversion（unix 有效；决定输出文件名与 soname/install_name）。
    pub soversion: Option<String>,
    pub install: bool,
    pub c_std: Option<String>,
    pub cxx_std: Option<String>,
    pub pkg_config: Vec<String>,
    pub when: Option<String>,
    pub pch: Vec<String>,
}

/// 自定义命令解析结果（路径已绝对化、glob 已展开、命令已做变量替换）。
#[derive(Debug, Clone)]
pub struct ResolvedCustomCommand {
    pub name: String,
    pub outputs: Vec<PathBuf>,
    pub command: Command,
    pub depends: Vec<PathBuf>,
    pub deps: Vec<String>,
}

/// 解析后的完整脚本（路径已相对脚本目录解析、glob 已展开、平台过滤已应用）。
#[derive(Debug, Clone)]
pub struct ResolvedScript {
    pub script: BuildScript,
    pub dir: PathBuf,
    pub out_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub targets: Vec<ResolvedTarget>,
    /// 自定义命令（按声明顺序；产物路径供 targets 的 sources 引用）。
    pub custom_commands: Vec<ResolvedCustomCommand>,
    pub parallel: usize,
    pub host: Host,
}

/// `resolve_target` 的共享上下文（单次 `resolve` 调用内不变）。
struct ResolveContext<'a> {
    dir: &'a Path,
    out_dir: &'a Path,
    host: &'a Host,
    vars: &'a BTreeMap<String, String>,
    cli_profile: Option<Profile>,
    /// 所有自定义命令声明的产物路径（target 的 sources 可引用，存在性不查）。
    custom_outputs: &'a BTreeSet<PathBuf>,
    /// 来自 imports 的使用要求（全局并入）。
    import_includes: &'a [PathBuf],
    import_defines: &'a [String],
    import_link: &'a [String],
}

impl BuildScript {
    pub fn parse(toml_text: &str) -> Result<Self, String> {
        toml::from_str::<BuildScript>(toml_text)
            .map_err(|e| format!("ns-build.toml 解析失败: {e}"))
    }
    /// 便捷版：无 CLI 覆盖。
    pub fn resolve_default(&self, dir: &Path) -> Result<ResolvedScript, String> {
        self.resolve(dir, &BTreeMap::new(), None)
    }

    /// 解析并规范化，应用默认值、glob 展开、平台过滤、变量替换。
    /// `overrides`：CLI `--define name=value` 覆盖（优先于 `[defaults].variables`）。
    pub fn resolve(
        &self,
        dir: &Path,
        overrides: &BTreeMap<String, String>,
        cli_profile: Option<Profile>,
    ) -> Result<ResolvedScript, String> {
        self.resolve_impl(dir, overrides, cli_profile, &BTreeSet::new())
    }

    /// `inherited_customs`：父脚本中自定义命令声明的产物（供 include 子脚本
    /// 在 source 存在性检查时识别跨文件生成源）。
    fn resolve_impl(
        &self,
        dir: &Path,
        overrides: &BTreeMap<String, String>,
        cli_profile: Option<Profile>,
        inherited_customs: &BTreeSet<PathBuf>,
    ) -> Result<ResolvedScript, String> {
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

        // 变量表：defaults.variables + CLI 覆盖。
        let mut vars = self.defaults.variables.clone();
        for (k, v) in overrides {
            vars.insert(k.clone(), v.clone());
        }
        // 内置变量。
        vars.entry("os".into()).or_insert_with(|| host.os.to_string());
        vars.entry("arch".into()).or_insert_with(|| host.arch.to_string());
        vars.entry("project".into()).or_insert_with(|| self.project.name.clone());
        vars.entry("out".into()).or_insert_with(|| self.project.out.clone());

        // imports：已安装的包（find_package 等价），使用要求全局并入所有目标。
        let mut import_includes: Vec<PathBuf> = Vec::new();
        let mut import_defines: Vec<String> = Vec::new();
        let mut import_link: Vec<String> = Vec::new();
        for imp in &self.imports {
            if imp.from.trim().is_empty() {
                return Err("import：from 不能为空".into());
            }
            let loaded = crate::export::load_import(&dir, &imp.from, imp.name.trim())?;
            let pfx = &loaded.prefix;
            for t in &loaded.config.targets {
                for i in &t.include_dirs {
                    let p = Path::new(i);
                    let p = if p.is_absolute() {
                        p.to_path_buf()
                    } else {
                        pfx.join(p)
                    };
                    if !import_includes.contains(&p) {
                        import_includes.push(p);
                    }
                }
                for d in &t.defines {
                    if !import_defines.contains(d) {
                        import_defines.push(d.clone());
                    }
                }
                // 产物：以绝对路径参与链接（如同 install 后的静态库路径）。
                if !t.output.is_empty() {
                    let artifact = pfx.join("lib").join(&t.output);
                    let s = artifact.to_string_lossy().into_owned();
                    if artifact.is_file() && !import_link.contains(&s) {
                        import_link.push(s);
                    }
                }
                // 记录的 -l 名字 / 路径。
                for l in &t.link {
                    if looks_like_lib_name(l) {
                        if !import_link.contains(l) {
                            import_link.push(l.clone());
                        }
                        continue;
                    }
                    let p = Path::new(l);
                    let s = if p.is_absolute() {
                        l.clone()
                    } else {
                        pfx.join(l).to_string_lossy().into_owned()
                    };
                    if !import_link.contains(&s) {
                        import_link.push(s);
                    }
                }
            }
        }

        // [[checks]]：编译器特性检测（try-compile+link）。结果缓存 in <cache_dir>/checks.json；
        // 成功时把 `${name}` 置 "1"（否则 "0"），并按 `define` 注入全局 -D<define>=1。
        run_checks(
            &self.checks,
            &self.defaults,
            &dir,
            &out_dir,
            &cache_dir,
            &mut vars,
            &mut import_defines,
        )?;

        // 自定义命令：第一遍只收集 outputs（含父脚本继承的），
        // 第二遍解析 depends/命令（这样 depends 可以引用任意顺序声明的产物）。
        let mut custom_outputs: BTreeSet<PathBuf> = inherited_customs.clone();
        let mut seen_custom: BTreeSet<String> = BTreeSet::new();
        let mut resolved_names: Vec<String> = Vec::with_capacity(self.custom_commands.len());
        for (i, cc) in self.custom_commands.iter().enumerate() {
            let name = if cc.name.is_empty() {
                format!("custom_{i}")
            } else {
                cc.name.clone()
            };
            if !seen_custom.insert(name.clone()) {
                return Err(format!("custom_commands 名重复: '{name}'"));
            }
            if cc.outputs.is_empty() {
                return Err(format!("custom_command '{name}': outputs 不能为空"));
            }
            let mut outputs: Vec<PathBuf> = Vec::new();
            for o in &cc.outputs {
                let o = substitute_vars(o, &vars);
                if o.trim().is_empty() {
                    return Err(format!("custom_command '{name}': outputs 含空路径"));
                }
                let p = if Path::new(&o).is_absolute() {
                    PathBuf::from(&o)
                } else {
                    dir.join(&o)
                };
                if !outputs.contains(&p) && !custom_outputs.insert(p.clone()) {
                    return Err(format!(
                        "custom_command '{name}': 产物重复声明: {}",
                        p.display()
                    ));
                }
                if !outputs.contains(&p) {
                    outputs.push(p);
                }
            }
            resolved_names.push(name);
        }

        let mut custom_commands: Vec<ResolvedCustomCommand> = Vec::new();
        for (i, cc) in self.custom_commands.iter().enumerate() {
            let name = resolved_names[i].clone();
            // outputs 重新绝对化（第二遍）。
            let mut outputs: Vec<PathBuf> = Vec::new();
            for o in &cc.outputs {
                let o = substitute_vars(o, &vars);
                let p = if Path::new(&o).is_absolute() {
                    PathBuf::from(&o)
                } else {
                    dir.join(&o)
                };
                if !outputs.contains(&p) {
                    outputs.push(p);
                }
            }
            // depends：字面量路径必须存在；支持 glob。
            let mut depends: Vec<PathBuf> = Vec::new();
            for d in &cc.depends {
                let d = substitute_vars(d, &vars);
                let p = if Path::new(&d).is_absolute() {
                    PathBuf::from(&d)
                } else {
                    dir.join(&d)
                };
                let s = p.to_string_lossy().to_string();
                if s.contains('*') || s.contains('?') {
                    let mut matched = false;
                    for entry in glob::glob(&s)
                        .map_err(|e| format!("custom_command '{name}': depends glob 错误 {d}: {e}"))?
                    {
                        let path = entry
                            .map_err(|e| format!("custom_command '{name}': depends 遍历错误: {e}"))?;
                        if path.is_file() {
                            depends.push(path);
                            matched = true;
                        }
                    }
                    if !matched {
                        return Err(format!(
                            "custom_command '{name}': depends '{d}' 无匹配文件"
                        ));
                    }
                } else if p.is_file() {
                    depends.push(p);
                } else if custom_outputs.contains(&p) {
                    // depends 的是另一个自定义命令将生成的文件：不做存在性检查，
                    // 构建时会按 deps 顺序先生成它。
                    depends.push(p);
                } else {
                    return Err(format!(
                        "custom_command '{name}': depends 文件不存在: {}",
                        p.display()
                    ));
                }
            }
            depends.sort();
            depends.dedup();
            // 命令变量替换。
            let command = match &cc.command {
                Command::Shell(s) => {
                    let s = substitute_vars(s, &vars);
                    if s.trim().is_empty() {
                        return Err(format!("custom_command '{name}': command 不能为空"));
                    }
                    Command::Shell(s)
                }
                Command::Run { program, args } => Command::Run {
                    program: substitute_vars(program, &vars),
                    args: args.iter().map(|a| substitute_vars(a, &vars)).collect(),
                },
            };
            custom_commands.push(ResolvedCustomCommand {
                name,
                outputs,
                command,
                depends,
                deps: cc.deps.clone(),
            });
        }
        // 校验 deps 引用存在。
        for cc in &custom_commands {
            for d in &cc.deps {
                if !seen_custom.contains(d) {
                    return Err(format!(
                        "custom_command '{}' 依赖不存在的自定义命令 '{}'",
                        cc.name, d
                    ));
                }
            }
        }

        // include 子脚本：先于本文件的 targets 合并（路径相对子脚本目录），
        // 子脚本的自定义命令产物也并入存在性检查集合。
        let mut targets: Vec<ResolvedTarget> = Vec::new();
        for inc in &self.includes {
            let inc_path = dir.join(inc);
            let (sub_script, sub_dir) = load(&inc_path)?;
            let sub_resolved =
                sub_script.resolve_impl(&sub_dir, overrides, cli_profile, &custom_outputs)?;
            for cc in sub_resolved.custom_commands {
                if !seen_custom.insert(cc.name.clone()) {
                    return Err(format!("custom_commands 名重复: '{}'", cc.name));
                }
                custom_outputs.extend(cc.outputs.iter().cloned());
                custom_commands.push(cc);
            }
            for t in sub_resolved.targets {
                targets.push(t);
            }
        }

        // INTERFACE 头文件库：依赖者使用要求由 propagate_usage 全局传递。
        let ctx = ResolveContext {
            dir: &dir,
            out_dir: &out_dir,
            host: &host,
            vars: &vars,
            cli_profile,
            custom_outputs: &custom_outputs,
            import_includes: &import_includes,
            import_defines: &import_defines,
            import_link: &import_link,
        };
        for t in &self.targets {
            if let Some(r) = self.resolve_target(t, &ctx)? {
                targets.push(r);
            }
        }
        // 依赖目标的公开使用要求（include / defines / link）向下游传递。
        propagate_usage(&mut targets)?;
        Ok(ResolvedScript {
            script: self.clone(),
            dir,
            out_dir,
            cache_dir,
            targets,
            custom_commands,
            parallel,
            host,
        })
    }

    /// 选择目标使用的工具链：显式名字优先，否则按主机 OS/架构匹配。
    fn select_toolchain<'a>(
        &'a self,
        t: &Target,
        host: &Host,
    ) -> Result<Option<&'a Toolchain>, String> {
        if let Some(name) = t.toolchain.as_deref().or(self.defaults.toolchain.as_deref()) {
            return self
                .toolchains
                .iter()
                .find(|tc| tc.name == name)
                .map(Some)
                .ok_or_else(|| format!("target '{}': 找不到工具链 '{}'", t.name, name));
        }
        Ok(self.toolchains.iter().find(|tc| {
            let os_ok = tc
                .os
                .as_deref()
                .map(|o| Os::parse(o) == host.os)
                .unwrap_or(true);
            let arch_ok = tc
                .arch
                .as_deref()
                .map(|a| Arch::parse(a) == host.arch)
                .unwrap_or(true);
            os_ok && arch_ok
        }))
    }

    fn resolve_target(
        &self,
        t: &Target,
        ctx: &ResolveContext<'_>,
    ) -> Result<Option<ResolvedTarget>, String> {
        let ResolveContext {
            dir,
            out_dir,
            host,
            vars,
            cli_profile,
            custom_outputs,
            import_includes,
            import_defines,
            import_link,
        } = *ctx;
        if t.name.is_empty() {
            return Err("target 缺少 name".into());
        }

        // 工具链决定目标平台：交叉编译时 os/arch 来自工具链。
        let tc = self.select_toolchain(t, host)?;
        let mut target_host = *host;
        if let Some(tc) = tc {
            if let Some(os) = tc.os.as_deref() {
                target_host.os = Os::parse(os);
            }
            if let Some(arch) = tc.arch.as_deref() {
                target_host.arch = Arch::parse(arch);
            }
        }

        // 平台过滤基于目标平台。
        if !t.os.is_empty() {
            let os = target_host.os.to_string();
            if !t.os.iter().any(|x| x == &os || x == "all") {
                return Ok(None);
            }
        }
        if !t.arch.is_empty() {
            let arch = target_host.arch.to_string();
            if !t.arch.iter().any(|x| x == &arch || x == "all") {
                return Ok(None);
            }
        }
        // when 与变量替换使用目标平台的 os/arch。
        let mut tvars = vars.clone();
        tvars.insert("os".into(), target_host.os.to_string());
        tvars.insert("arch".into(), target_host.arch.to_string());
        if let Some(when) = &t.when {
            if !eval_when(when, &tvars) {
                return Ok(None);
            }
        }

        // 变量替换辅助。
        let sub = |s: &str| -> String { substitute_vars(s, &tvars) };

        // 编译器 / 归档器：target 覆盖 > 工具链 > defaults。
        let mut cc = self.defaults.cc.clone();
        let mut cxx = self.defaults.cxx.clone();
        let mut ar = self.defaults.ar.clone();
        let mut toolchain_flags: Vec<String> = vec![];
        let mut toolchain_incs: Vec<String> = vec![];
        let mut toolchain_link: Vec<String> = vec![];
        let mut sysroot: Option<&str> = None;
        if let Some(tc) = tc {
            if !tc.cc.is_empty() {
                cc = sub(&tc.cc);
            }
            if !tc.cxx.is_empty() {
                cxx = sub(&tc.cxx);
            }
            if !tc.ar.is_empty() {
                ar = sub(&tc.ar);
            }
            toolchain_flags.extend(tc.flags.iter().map(|f| sub(f)));
            toolchain_incs.extend(tc.include_dirs.iter().map(|f| sub(f)));
            toolchain_link.extend(tc.link_flags.iter().map(|f| sub(f)));
            sysroot = tc.sysroot.as_deref();
        }
        if let Some(v) = &t.cc {
            cc = sub(v);
        }

        // 展开源文件 glob（相对 project.root，默认脚本目录）。
        let src_root = if self.project.root.is_empty() {
            dir.to_path_buf()
        } else {
            dir.join(sub(&self.project.root))
        };
        let mut sources = Vec::new();
        let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
        for pat in &t.sources {
            let pat = sub(pat);
            let p = if Path::new(&pat).is_absolute() {
                PathBuf::from(&pat)
            } else {
                src_root.join(&pat)
            };
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
                    return Err(format!("glob '{pat}' 无匹配文件（根 {}）", src_root.display()));
                }
            } else if p.is_file() {
                if seen.insert(p.clone()) {
                    sources.push(p);
                }
            } else if custom_outputs.contains(&p) {
                // 自定义命令将生成该文件（构建时先于编译执行）。
                if seen.insert(p.clone()) {
                    sources.push(p);
                }
            } else {
                return Err(format!("源文件不存在: {}", p.display()));
            }
        }
        sources.sort();

        // 校验：可链接目标需要源文件或依赖。
        if t.kind.is_linkable() && sources.is_empty() && t.deps.is_empty() {
            return Err(format!(
                "target '{}': {} 目标需要 sources 或 deps",
                t.name, "链接"
            ));
        }

        // 语言：未显式指定时按源文件后缀推断（.cpp/.cc/... → C++）。
        let language = t.language.unwrap_or_else(|| {
            if sources.iter().any(|s| is_cxx_path(s)) {
                Language::Cpp
            } else {
                Language::C
            }
        });

        let mut flags = vec![];
        flags.extend(self.defaults.flags.iter().map(|f| sub(f)));
        flags.extend(toolchain_flags);
        if let Some(sr) = sysroot {
            flags.push(format!("--sysroot={sr}"));
        }
        flags.extend(t.flags.iter().map(|f| sub(f)));
        // pkg-config 的编译参数（-I/-D/-pthread 等）。
        let pkg_config: Vec<String> = t.pkg_config.iter().map(|p| sub(p)).collect();
        for pkg in &pkg_config {
            flags.extend(pkg_config_flags(pkg, "--cflags"));
        }

        let mut cflags = vec![];
        cflags.extend(self.defaults.cflags.iter().map(|f| sub(f)));
        cflags.extend(t.cflags.iter().map(|f| sub(f)));

        let mut cxxflags = vec![];
        cxxflags.extend(self.defaults.cxxflags.iter().map(|f| sub(f)));
        cxxflags.extend(t.cxxflags.iter().map(|f| sub(f)));

        let mut defines = vec![];
        defines.extend(self.defaults.defines.iter().map(|f| sub(f)));
        for d in import_defines {
            if !defines.contains(d) {
                defines.push(d.clone());
            }
        }
        defines.extend(t.defines.iter().map(|f| sub(f)));

        let mut link = vec![];
        link.extend(t.link.iter().map(|f| sub(f)));
        for l in import_link {
            if !link.contains(l) {
                link.push(l.clone());
            }
        }
        let mut link_flags = vec![];
        link_flags.extend(toolchain_link);
        link_flags.extend(t.link_flags.iter().map(|f| sub(f)));
        let link_dirs: Vec<PathBuf> = t
            .link_dirs
            .iter()
            .map(|s| dir.join(sub(s)))
            .collect();

        let profile = t
            .profile
            .or(cli_profile)
            .or(self.defaults.profile)
            .unwrap_or_default();
        let strip = t.strip.unwrap_or(self.defaults.strip);
        // rpath：$ORIGIN 模板/绝对路径原样保留；其余相对路径相对脚本目录解析。
        let rpath: Vec<String> = t
            .rpath
            .iter()
            .map(|s| sub(s))
            .map(|s| {
                let p = Path::new(&s);
                if p.is_absolute() || s.contains("$ORIGIN") || s.contains("${ORIGIN}") {
                    s
                } else {
                    dir.join(&s).to_string_lossy().into_owned()
                }
            })
            .collect();
        let base_output = sub(t.output.as_deref().unwrap_or(&t.name));
        let output = if t.output.is_some() {
            // 显式 output 完全按用户写法处理。
            base_output
        } else {
            // soversion（非 Windows）改变动态库文件名：libfoo.so.N / libfoo.N.dylib。
            let sv = t
                .soversion
                .as_deref()
                .filter(|v| !v.trim().is_empty())
                .filter(|_| !target_host.os.is_windows_family());
            match t.kind {
                TargetKind::Exe
                    if !matches!(language, Language::Python | Language::Shell | Language::Java) =>
                {
                    format!("{}{}", base_output, target_host.os.exe_suffix())
                }
                TargetKind::Lib | TargetKind::Module => match sv {
                    Some(v) => target_host.os.dylib_name_versioned(&base_output, v),
                    None => target_host.os.dylib_name(&base_output),
                },
                TargetKind::Static => target_host.os.static_name(&base_output),
                _ => base_output,
            }
        };

        let mut incs: Vec<PathBuf> = self
            .defaults
            .include_dirs
            .iter()
            .map(|s| dir.join(sub(s)))
            .collect();
        for i in import_includes {
            if !incs.contains(i) {
                incs.push(i.clone());
            }
        }
        incs.extend(t.include_dirs.iter().map(|s| dir.join(sub(s))));
        incs.extend(toolchain_incs.iter().map(|s| dir.join(sub(s))));
        // INTERFACE 使用要求（include_dirs/defines）由 propagate_usage 全局传递。

        Ok(Some(ResolvedTarget {
            name: t.name.clone(),
            kind: t.kind,
            language,
            cc,
            cxx,
            ar,
            host: target_host,
            sources,
            include_dirs: incs,
            flags,
            cflags,
            cxxflags,
            link,
            link_dirs,
            link_flags,
            defines,
            deps: t.deps.clone(),
            private_deps: t.private_deps.clone(),
            output: out_dir.join(output),
            groups: t.groups,
            group_size: t.group_size,
            parallel: t.parallel.unwrap_or(true),
            hooks: resolve_hooks(&t.hooks, &sub),
            profile,
            strip,
            rpath,
            soversion: t
                .soversion
                .as_deref()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty()),
            warnings: self.defaults.warnings,
            werror: t.werror.unwrap_or(self.defaults.werror),
            install: t.install.unwrap_or(true),
            c_std: t.c_std.clone(),
            cxx_std: t.cxx_std.clone(),
            pkg_config,
            when: t.when.clone(),
            pch: t
                .pch
                .clone()
                .map(PchValue::into_vec)
                .unwrap_or_default()
                .iter()
                .map(|p| sub(p))
                .collect(),
        }))
    }
}

/// 判断路径是否 C++ 源文件。
fn is_cxx_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("cc") | Some("cpp") | Some("cxx") | Some("C") | Some("mm") | Some("mxx")
    )
}

/// 对钩子命令做变量替换。
fn resolve_hooks(hooks: &Hooks, sub: &impl Fn(&str) -> String) -> Hooks {
    let map_cmd = |c: &Command| match c {
        Command::Shell(s) => Command::Shell(sub(s)),
        Command::Run { program, args } => Command::Run {
            program: sub(program),
            args: args.iter().map(|a| sub(a)).collect(),
        },
    };
    Hooks {
        before: hooks.before.iter().map(map_cmd).collect(),
        after: hooks.after.iter().map(map_cmd).collect(),
    }
}

/// 调用 pkg-config 获取编译/链接参数；失败时返回空（不阻塞构建）。
fn pkg_config_flags(pkg: &str, flag: &str) -> Vec<String> {
    match std::process::Command::new("pkg-config").args([flag, pkg]).output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .split_whitespace()
            .map(|s| s.to_string())
            .collect(),
        _ => vec![],
    }
}

/// 目标的公开导出集：向下游（包括经由 public deps）重导出的使用要求。
/// 目标本身声明的是 public 的；private_deps 的导出集只并入**本级编译**，
/// 不再向下游重导出（CMake PRIVATE 语义）。
#[derive(Clone, PartialEq)]
struct ExportSet {
    include_dirs: Vec<PathBuf>,
    defines: Vec<String>,
    link: Vec<String>,
    link_dirs: Vec<PathBuf>,
    link_flags: Vec<String>,
    pkg_config: Vec<String>,
}

impl ExportSet {
    fn of(t: &ResolvedTarget) -> Self {
        Self {
            include_dirs: t.include_dirs.clone(),
            defines: t.defines.clone(),
            link: t.link.clone(),
            link_dirs: t.link_dirs.clone(),
            link_flags: t.link_flags.clone(),
            pkg_config: t.pkg_config.clone(),
        }
    }

    /// 并入 `src`，返回是否有变化。
    fn merge(&mut self, src: &ExportSet) -> bool {
        let mut changed = false;
        changed |= merge_unique(&mut self.include_dirs, &src.include_dirs);
        changed |= merge_unique(&mut self.defines, &src.defines);
        changed |= merge_unique(&mut self.link, &src.link);
        changed |= merge_unique(&mut self.link_dirs, &src.link_dirs);
        changed |= merge_unique(&mut self.link_flags, &src.link_flags);
        changed |= merge_unique(&mut self.pkg_config, &src.pkg_config);
        changed
    }
}

/// 传递依赖目标的公开使用要求（include_dirs / defines / link / link_dirs / link_flags / pkg_config）。
/// `deps` 重导出（PUBLIC），`private_deps` 仅提供给本级编译与链接，不外传。
fn propagate_usage(targets: &mut [ResolvedTarget]) -> Result<(), String> {
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for (i, t) in targets.iter().enumerate() {
        if index.insert(t.name.clone(), i).is_some() {
            return Err(format!("target 名重复: '{}'", t.name));
        }
    }
    let kinds: Vec<TargetKind> = targets.iter().map(|t| t.kind).collect();
    // 依赖存在性/自指校验（public 与 private 都检查）。
    for t in targets.iter() {
        for dep in t.deps.iter().chain(t.private_deps.iter()) {
            if !index.contains_key(dep) {
                return Err(format!("target '{}' 依赖不存在的 target '{}'", t.name, dep));
            }
            if dep == &t.name {
                return Err(format!("target '{}' 依赖自身", t.name));
            }
        }
    }
    // 编译集 = 目标（原地累积，初始为目标自身声明集之外的 defaults/工具链合并结果）。
    // 导出集 = 自身声明集 + public 依赖的导出集（private 不参与重导出）。
    let self_sets: Vec<ExportSet> = targets.iter().map(ExportSet::of).collect();
    let mut exports: Vec<ExportSet> = self_sets.clone();
    // 最多迭代 N 轮，覆盖任意深度的依赖链。
    for _ in 0..targets.len().max(1) {
        let snapshot = exports.clone();
        let mut changed = false;
        for (i, target) in targets.iter_mut().enumerate() {
            let mut exp_new = self_sets[i].clone();
            // public 依赖（deps）：
            // - 其导出集并入本级编译集；
            // - 其导出集也并入本级导出集（向下游重导出）。
            for dep in &target.deps {
                let &j = index.get(dep).expect("dep 存在性已校验");
                if j == i {
                    continue;
                }
                if matches!(kinds[j], TargetKind::Exe | TargetKind::Phony) {
                    continue;
                }
                changed |= merge_unique(&mut target.include_dirs, &snapshot[j].include_dirs);
                changed |= merge_unique(&mut target.defines, &snapshot[j].defines);
                changed |= merge_unique(&mut target.link, &snapshot[j].link);
                changed |= merge_unique(&mut target.link_dirs, &snapshot[j].link_dirs);
                changed |= merge_unique(&mut target.link_flags, &snapshot[j].link_flags);
                changed |= merge_unique(&mut target.pkg_config, &snapshot[j].pkg_config);
                changed |= exp_new.merge(&snapshot[j]);
            }
            // private 依赖（private_deps）：只并入本级编译集，不进导出集。
            for dep in &target.private_deps {
                let &j = index.get(dep).expect("private_dep 存在性已校验");
                if j == i {
                    continue;
                }
                if matches!(kinds[j], TargetKind::Exe | TargetKind::Phony) {
                    continue;
                }
                changed |= merge_unique(&mut target.include_dirs, &snapshot[j].include_dirs);
                changed |= merge_unique(&mut target.defines, &snapshot[j].defines);
                changed |= merge_unique(&mut target.link, &snapshot[j].link);
                changed |= merge_unique(&mut target.link_dirs, &snapshot[j].link_dirs);
                changed |= merge_unique(&mut target.link_flags, &snapshot[j].link_flags);
                changed |= merge_unique(&mut target.pkg_config, &snapshot[j].pkg_config);
            }
            if exp_new != exports[i] {
                exports[i] = exp_new;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    Ok(())
}

fn merge_unique<T: PartialEq + Clone>(dst: &mut Vec<T>, src: &[T]) -> bool {
    let mut changed = false;
    for item in src {
        if !dst.contains(item) {
            dst.push(item.clone());
            changed = true;
        }
    }
    changed
}

/// `[[checks]]` 执行器：try-compile+link，带磁盘缓存（`<cache_dir>/checks.json`）。
/// 检测成功 → `vars[name] = "1"`；失败 → `"0"`（fail_fatal 则整体报错）。
fn run_checks(
    checks: &[Check],
    defaults: &Defaults,
    dir: &Path,
    out_dir: &Path,
    cache_dir: &Path,
    vars: &mut BTreeMap<String, String>,
    import_defines: &mut Vec<String>,
) -> Result<(), String> {
    if checks.is_empty() {
        return Ok(());
    }
    let check_dir = out_dir.join("checks");
    std::fs::create_dir_all(&check_dir)
        .map_err(|e| format!("创建检测目录 {} 失败: {e}", check_dir.display()))?;

    // 磁盘缓存：hash -> bool。
    let cache_path = cache_dir.join("checks.json");
    let mut cache: BTreeMap<String, bool> = std::fs::read_to_string(&cache_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let mut cache_dirty = false;

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for chk in checks {
        let name = chk.name.trim();
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
        {
            return Err(format!("check：name '{name}' 非法（仅字母数字 / '-' / '_' / '.'）"));
        }
        if !seen.insert(name.to_string()) {
            return Err(format!("check：name '{name}' 重复"));
        }
        let sub = |s: &str| substitute_vars(s, vars);
        let source = match (&chk.source, &chk.source_file) {
            (Some(s), None) => sub(s),
            (None, Some(f)) => {
                let f = sub(f);
                let p = if Path::new(&f).is_absolute() {
                    PathBuf::from(&f)
                } else {
                    dir.join(&f)
                };
                std::fs::read_to_string(&p)
                    .map_err(|e| format!("check '{name}'：读取 source_file {} 失败: {e}", p.display()))?
            }
            _ => {
                return Err(format!(
                    "check '{name}'：source 与 source_file 必须二选一（不多不少）"
                ))
            }
        };
        let kind_raw = if chk.kind.trim().is_empty() {
            "cxx".to_string()
        } else {
            chk.kind.trim().to_lowercase()
        };
        let is_c = matches!(kind_raw.as_str(), "c");
        let is_cpp = matches!(kind_raw.as_str(), "cxx" | "cpp" | "c++");
        if !is_c && !is_cpp {
            return Err(format!(
                "check '{name}'：kind 必须是 c 或 cxx（得到 '{kind_raw}'）"
            ));
        }
        let cc = if is_c { &defaults.cc } else { &defaults.cxx };
        let cc = sub(cc);
        let defines: Vec<String> = chk.defines.iter().map(|d| sub(d)).collect();
        let libraries: Vec<String> = chk.libraries.iter().map(|d| sub(d)).collect();

        // 缓存键：编译器 + 源 + 宏 + 库。
        let key_src = format!(
            "v1\n{cc}\n{kind_raw}\n{source}\n{}\n{}",
            defines.join(","),
            libraries.join(",")
        );
        let key = blake3::hash(key_src.as_bytes()).to_hex().to_string();

        let ok = match cache.get(&key) {
            Some(&v) => v,
            None => {
                let ext = if is_c { "c" } else { "cpp" };
                let src_path = check_dir.join(format!("{name}.{ext}"));
                let exe_path = check_dir.join(format!("{name}.bin"));
                std::fs::write(&src_path, &source)
                    .map_err(|e| format!("check '{name}'：写入检测源 {} 失败: {e}", src_path.display()))?;
                let msvc = crate::compiler::is_msvc_compiler(&cc);
                let mut cmd = std::process::Command::new(&cc);
                if msvc {
                    cmd.arg("/nologo");
                    for d in &defines {
                        cmd.arg(format!("/D{d}"));
                    }
                    for l in &libraries {
                        cmd.arg(crate::compiler::msvc_lib_name(l));
                    }
                    cmd.arg(format!("/Fe{}", exe_path.display()));
                    cmd.arg(&src_path);
                } else {
                    for d in &defines {
                        cmd.arg(format!("-D{d}"));
                    }
                    cmd.arg(&src_path);
                    cmd.arg("-o");
                    cmd.arg(&exe_path);
                    for l in &libraries {
                        cmd.arg(format!("-l{l}"));
                    }
                }
                let out = cmd
                    .output()
                    .map_err(|e| format!("check '{name}'：无法启动编译器 '{cc}': {e}"))?;
                let v = out.status.success();
                cache.insert(key, v);
                cache_dirty = true;
                v
            }
        };
        vars.insert(name.to_string(), if ok { "1".into() } else { "0".into() });
        if ok {
            if let Some(d) = &chk.define {
                let d = substitute_vars(d, vars);
                if !d.trim().is_empty() && !import_defines.contains(&d) {
                    import_defines.push(format!("{d}=1"));
                }
            }
        } else if chk.fail_fatal {
            return Err(format!(
                "check '{name}'：try-compile 未通过（fail_fatal = true）"
            ));
        }
    }
    if cache_dirty {
        let _ = std::fs::create_dir_all(cache_dir);
        if let Ok(text) = serde_json::to_string_pretty(&cache) {
            let _ = crate::fsutil::atomic_write(&cache_path, text.as_bytes());
        }
    }
    Ok(())
}

/// 判断字符串是否像库名（`-l` 风格：无分隔符、无常见库文件后缀）。
fn looks_like_lib_name(l: &str) -> bool {
    !l.contains('/')
        && !l.contains('\\')
        && !l.starts_with('-')
        && !l.ends_with(".a")
        && !l.ends_with(".so")
        && !l.ends_with(".dylib")
        && !l.ends_with(".lib")
        && !l.ends_with(".dll")
}

/// 替换字符串中的 `${name}` 变量（保留未定义变量，按 UTF-8 字符安全处理）。
pub fn substitute_vars(s: &str, vars: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while let Some(rel) = s[i..].find("${") {
        let start = i + rel;
        out.push_str(&s[i..start]);
        match s[start + 2..].find('}') {
            Some(end_rel) => {
                let name = &s[start + 2..start + 2 + end_rel];
                if let Some(v) = vars.get(name) {
                    out.push_str(v);
                } else {
                    // 未定义变量：原样保留。
                    out.push_str(&s[start..start + 2 + end_rel + 1]);
                }
                i = start + 2 + end_rel + 1;
            }
            None => {
                // 没有闭合 `}`：剩余部分原样保留。
                out.push_str(&s[start..]);
                return out;
            }
        }
    }
    out.push_str(&s[i..]);
    out
}

/// 求值 `when` 表达式（简单比较与逻辑组合）。
/// 支持：`os == "linux"`、`arch == "x86_64"`、`var == "value"`、`&&`、`||`、`!`、`true`/`false`。
pub fn eval_when(expr: &str, vars: &BTreeMap<String, String>) -> bool {
    let expr = expr.trim();
    // 逻辑或 / 与 的简单分割（不支持括号内嵌套，先处理最外层）。
    for part in split_top(expr, "||") {
        if eval_when_and(&part, vars) {
            return true;
        }
    }
    false
}

fn eval_when_and(expr: &str, vars: &BTreeMap<String, String>) -> bool {
    for part in split_top(expr, "&&") {
        if !eval_when_atom(part.trim(), vars) {
            return false;
        }
    }
    true
}

fn eval_when_atom(atom: &str, vars: &BTreeMap<String, String>) -> bool {
    let atom = atom.trim();
    // 括号分组：`(a || b) && c`。
    if atom.starts_with('(') && atom.ends_with(')') {
        return eval_when(&atom[1..atom.len() - 1], vars);
    }
    if let Some(rest) = atom.strip_prefix('!') {
        return !eval_when_atom(rest, vars);
    }
    if atom == "true" {
        return true;
    }
    if atom == "false" {
        return false;
    }
    // `os == "linux"` / `var != "value"`。
    if let Some(eq) = find_top(atom, "==") {
        let lhs = atom[..eq].trim();
        let rhs = atom[eq + 2..].trim().trim_matches('"');
        return vars.get(lhs).map(|v| v == rhs).unwrap_or(false);
    }
    if let Some(ne) = find_top(atom, "!=") {
        let lhs = atom[..ne].trim();
        let rhs = atom[ne + 2..].trim().trim_matches('"');
        return vars.get(lhs).map(|v| v != rhs).unwrap_or(true);
    }
    // 裸变量：非空且非 "false"。
    if let Some(v) = vars.get(atom) {
        return v != "false" && !v.is_empty();
    }
    false
}

fn split_top(s: &str, op: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {}
        }
        if depth == 0 && s[i..].starts_with(op) {
            out.push(s[start..i].to_string());
            i += op.len();
            start = i;
            continue;
        }
        i += 1;
    }
    out.push(s[start..].to_string());
    out
}

fn find_top(s: &str, op: &str) -> Option<usize> {
    let mut depth = 0;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {}
        }
        if depth == 0 && s[i..].starts_with(op) {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// 从目录或文件加载并解析构建脚本。
/// 若 `path` 是目录，则查找其中的 `ns-build.toml`；若直接是文件则用该文件。
pub fn load(path: &Path) -> Result<(BuildScript, PathBuf), String> {
    let (script_path, dir) = if path.is_file() {
        (path.to_path_buf(), path.parent().unwrap_or(Path::new(".")).to_path_buf())
    } else {
        (path.join("ns-build.toml"), path.to_path_buf())
    };
    let text = std::fs::read_to_string(&script_path)
        .map_err(|e| format!("读取 {} 失败: {e}", script_path.display()))?;
    let script = BuildScript::parse(&text)?;
    Ok((script, dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!(
            "nsbuild-test-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
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
        let r = s.resolve_default(&d).unwrap();
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
        let r = s.resolve_default(&d).unwrap();
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

    #[test]
    fn substitute_vars_replaces() {
        let mut vars = BTreeMap::new();
        vars.insert("name".into(), "world".into());
        assert_eq!(substitute_vars("hello ${name}", &vars), "hello world");
        assert_eq!(substitute_vars("${missing}", &vars), "${missing}");
        assert_eq!(substitute_vars("plain", &vars), "plain");
        assert_eq!(
            substitute_vars("构建-${name}-🚀", &vars),
            "构建-world-🚀"
        );
        assert_eq!(substitute_vars("未闭合 ${name", &vars), "未闭合 ${name");
    }

    #[test]
    fn eval_when_expression() {
        let mut vars = BTreeMap::new();
        vars.insert("os".into(), "macos".into());
        vars.insert("arch".into(), "x86_64".into());
        assert!(eval_when(r#"os == "macos""#, &vars));
        assert!(eval_when(r#"os == "macos" && arch == "x86_64""#, &vars));
        assert!(eval_when(r#"os == "linux" || arch == "x86_64""#, &vars));
        assert!(!eval_when(r#"os == "linux""#, &vars));
        assert!(!eval_when(r#"os == "macos" && arch == "arm64""#, &vars));
        assert!(eval_when("true", &vars));
        assert!(!eval_when("false", &vars));
        assert!(eval_when(
            r#"(os == "linux" || os == "macos") && arch == "x86_64""#,
            &vars
        ));
        assert!(!eval_when(
            r#"!(os == "macos" || arch == "arm64")"#,
            &vars
        ));
    }

    #[test]
    fn when_filters_target() {
        let s = BuildScript::parse(
            r#"
[[targets]]
name = "a"
kind = "exe"
sources = ["x.c"]
when = 'os == "linux"'

[[targets]]
name = "b"
kind = "exe"
sources = ["y.c"]
"#,
        )
        .unwrap();
        let d = tmpdir();
        std::fs::write(d.join("x.c"), "").unwrap();
        std::fs::write(d.join("y.c"), "").unwrap();
        let r = s.resolve_default(&d).unwrap();
        let host_os = Host::current().os.to_string();
        if host_os == "linux" {
            assert_eq!(r.targets.len(), 2);
        } else {
            assert_eq!(r.targets.len(), 1);
            assert_eq!(r.targets[0].name, "b");
        }
    }

    #[test]
    fn include_sub_script_merges_targets() {
        let d = tmpdir();
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("sub/main.c"), "int sub(void){return 1;}\n").unwrap();
        std::fs::write(
            d.join("sub/ns-build.toml"),
            r#"
[[targets]]
name = "sub"
kind = "exe"
sources = ["main.c"]
"#,
        )
        .unwrap();
        std::fs::write(d.join("main.c"), "int main(void){return 0;}\n").unwrap();
        let s = BuildScript::parse(
            r#"
includes = ["sub"]

[[targets]]
name = "top"
kind = "exe"
sources = ["main.c"]
"#,
        )
        .unwrap();
        let r = s.resolve_default(&d).unwrap();
        let names: Vec<&str> = r.targets.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"top"), "应含 top: {names:?}");
        assert!(names.contains(&"sub"), "应含子脚本 sub: {names:?}");
    }

    #[test]
    fn interface_propagates_incs_defines() {
        let d = tmpdir();
        std::fs::write(d.join("main.c"), "int main(void){return 0;}\n").unwrap();
        let s = BuildScript::parse(
            r#"
[[targets]]
name = "hdrs"
kind = "interface"
include_dirs = ["include"]
defines = ["API=1"]

[[targets]]
name = "app"
kind = "exe"
sources = ["main.c"]
deps = ["hdrs"]
"#,
        )
        .unwrap();
        let r = s.resolve_default(&d).unwrap();
        let app = r.targets.iter().find(|t| t.name == "app").unwrap();
        assert!(
            app.include_dirs.iter().any(|p| p.ends_with("include")),
            "应合并 interface include: {:?}",
            app.include_dirs
        );
        assert!(app.defines.contains(&"API=1".to_string()), "应合并 interface define");
    }

    #[test]
    fn parses_module_and_interface_kinds() {
        let s = BuildScript::parse(
            r#"
[[targets]]
name = "plug"
kind = "module"
sources = ["p.c"]

[[targets]]
name = "hdrs"
kind = "interface"
"#,
        )
        .unwrap();
        assert_eq!(s.targets[0].kind, TargetKind::Module);
        assert_eq!(s.targets[1].kind, TargetKind::Interface);
    }

    #[test]
    fn cli_profile_overrides_defaults() {
        let d = tmpdir();
        std::fs::write(d.join("main.c"), "int main(void){return 0;}\n").unwrap();
        let s = BuildScript::parse(
            r#"
[defaults]
profile = "release"

[[targets]]
name = "app"
kind = "exe"
sources = ["main.c"]
"#,
        )
        .unwrap();
        let r = s.resolve(&d, &BTreeMap::new(), Some(Profile::Debug)).unwrap();
        assert_eq!(r.targets[0].profile, Profile::Debug);
        let r2 = s.resolve_default(&d).unwrap();
        assert_eq!(r2.targets[0].profile, Profile::Release);
    }

    #[test]
    fn project_root_prefixes_sources() {
        let d = tmpdir();
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join("src/main.c"), "int main(void){return 0;}\n").unwrap();
        let s = BuildScript::parse(
            r#"
[project]
root = "src"

[[targets]]
name = "app"
kind = "exe"
sources = ["*.c"]
"#,
        )
        .unwrap();
        let r = s.resolve_default(&d).unwrap();
        assert_eq!(r.targets[0].sources.len(), 1);
        assert!(r.targets[0].sources[0].ends_with("src/main.c"));
    }

    #[test]
    fn default_output_names_follow_platform() {
        let d = tmpdir();
        std::fs::write(d.join("main.c"), "int main(void){return 0;}\n").unwrap();
        std::fs::write(d.join("lib.c"), "int f(void){return 0;}\n").unwrap();
        let s = BuildScript::parse(
            r#"
[[targets]]
name = "app"
kind = "exe"
sources = ["main.c"]

[[targets]]
name = "util"
kind = "static"
sources = ["lib.c"]

[[targets]]
name = "plug"
kind = "lib"
sources = ["lib.c"]
"#,
        )
        .unwrap();
        let r = s.resolve_default(&d).unwrap();
        let host = Host::current();
        let find = |n: &str| r.targets.iter().find(|t| t.name == n).unwrap();
        assert_eq!(
            find("app").output.file_name().unwrap().to_string_lossy(),
            format!("app{}", host.os.exe_suffix())
        );
        assert_eq!(
            find("util").output.file_name().unwrap().to_string_lossy(),
            host.os.static_name("util")
        );
        assert_eq!(
            find("plug").output.file_name().unwrap().to_string_lossy(),
            host.os.dylib_name("plug")
        );
    }

    #[test]
    fn cross_toolchain_sets_target_platform() {
        let d = tmpdir();
        std::fs::write(d.join("main.c"), "int main(void){return 0;}\n").unwrap();
        let s = BuildScript::parse(
            r#"
[[toolchains]]
name = "mingw"
cc = "x86_64-w64-mingw32-gcc"
os = "windows"
arch = "x86_64"

[[targets]]
name = "app"
kind = "exe"
toolchain = "mingw"
sources = ["main.c"]
"#,
        )
        .unwrap();
        let r = s.resolve_default(&d).unwrap();
        let t = &r.targets[0];
        assert_eq!(t.cc, "x86_64-w64-mingw32-gcc");
        assert_eq!(t.host.os, Os::Windows);
        assert!(t.output.to_string_lossy().ends_with("app.exe"));
    }

    #[test]
    fn missing_toolchain_is_error() {
        let d = tmpdir();
        std::fs::write(d.join("main.c"), "int main(void){return 0;}\n").unwrap();
        let s = BuildScript::parse(
            r#"
[[targets]]
name = "app"
kind = "exe"
toolchain = "nope"
sources = ["main.c"]
"#,
        )
        .unwrap();
        assert!(s.resolve_default(&d).is_err());
    }

    #[test]
    fn propagates_usage_from_dependencies() {
        let d = tmpdir();
        std::fs::create_dir_all(d.join("include")).unwrap();
        std::fs::write(d.join("main.c"), "int main(void){return 0;}\n").unwrap();
        std::fs::write(d.join("lib.c"), "int f(void){return 0;}\n").unwrap();
        let s = BuildScript::parse(
            r#"
[[targets]]
name = "util"
kind = "static"
sources = ["lib.c"]
include_dirs = ["include"]
defines = ["UTIL=1"]
link = ["m", "pthread"]

[[targets]]
name = "app"
kind = "exe"
sources = ["main.c"]
deps = ["util"]
"#,
        )
        .unwrap();
        let r = s.resolve_default(&d).unwrap();
        let app = r.targets.iter().find(|t| t.name == "app").unwrap();
        assert!(app.include_dirs.iter().any(|p| p.ends_with("include")));
        assert!(app.defines.contains(&"UTIL=1".to_string()));
        assert!(app.link.contains(&"m".to_string()));
        assert!(app.link.contains(&"pthread".to_string()));
    }

    #[test]
    fn private_deps_not_exported() {
        let d = tmpdir();
        std::fs::create_dir_all(d.join("a-inc")).unwrap();
        std::fs::write(d.join("a.c"), "int a(void){return 0;}\n").unwrap();
        std::fs::write(d.join("b.c"), "int b(void){return 0;}\n").unwrap();
        std::fs::write(d.join("main.c"), "int b(void); int main(void){return b();}\n").unwrap();
        let s = BuildScript::parse(
            r#"
[[targets]]
name = "a"
kind = "static"
sources = ["a.c"]
include_dirs = ["a-inc"]
defines = ["A_FLAG=1"]
link = ["m"]

[[targets]]
name = "b"
kind = "static"
sources = ["b.c"]
private_deps = ["a"]

[[targets]]
name = "app"
kind = "exe"
sources = ["main.c"]
deps = ["b"]
"#,
        )
        .unwrap();
        let r = s.resolve_default(&d).unwrap();
        let b = r.targets.iter().find(|t| t.name == "b").unwrap();
        let app = r.targets.iter().find(|t| t.name == "app").unwrap();
        // b 自己编译能看到 a 的使用要求。
        assert!(b.include_dirs.iter().any(|p| p.ends_with("a-inc")));
        assert!(b.defines.contains(&"A_FLAG=1".to_string()));
        assert!(b.link.contains(&"m".to_string()));
        // app 不能经由 b 看到 a 的私有使用要求（include/define/-l）。
        assert!(!app.include_dirs.iter().any(|p| p.ends_with("a-inc")));
        assert!(!app.defines.contains(&"A_FLAG=1".to_string()));
        // 链接层面 a 仍会出现在 app 的链接链路中（collect_link_libs 覆盖 private），
        // 这里只校验传播语义。
        assert!(!app.link.contains(&"m".to_string()));
    }

    #[test]
    fn private_dep_missing_is_error() {
        let d = tmpdir();
        std::fs::write(d.join("main.c"), "int main(void){return 0;}\n").unwrap();
        let s = BuildScript::parse(
            r#"
[[targets]]
name = "app"
kind = "exe"
sources = ["main.c"]
private_deps = ["nonexistent"]
"#,
        )
        .unwrap();
        assert!(s.resolve_default(&d).is_err());
    }

    #[test]
    fn language_inferred_from_cpp_source() {
        let d = tmpdir();
        std::fs::write(d.join("main.cpp"), "int main(){return 0;}\n").unwrap();
        let s = BuildScript::parse(
            r#"
[[targets]]
name = "app"
kind = "exe"
sources = ["main.cpp"]
"#,
        )
        .unwrap();
        let r = s.resolve_default(&d).unwrap();
        assert_eq!(r.targets[0].language, Language::Cpp);
    }

    #[test]
    fn duplicate_target_names_rejected() {
        let d = tmpdir();
        std::fs::write(d.join("a.c"), "int a;\n").unwrap();
        let s = BuildScript::parse(
            r#"
[[targets]]
name = "same"
kind = "object"
sources = ["a.c"]

[[targets]]
name = "same"
kind = "object"
sources = ["a.c"]
"#,
        )
        .unwrap();
        assert!(s.resolve_default(&d).is_err());
    }
}
