//! 编译/链接/归档命令生成。
//!
//! 只负责把目标描述转成具体的外部命令（gcc/clang/ar），执行交由 executor。

use std::path::{Path, PathBuf};

use crate::platform::Host;
use crate::script::{Profile, ResolvedTarget};

/// 一个可执行的任务单元（编译一个对象 / 链接 / 归档）。
#[derive(Debug, Clone)]
pub struct CompileTask {
    pub key: String,
    pub kind: CompileKind,
    /// 输入文件。
    pub inputs: Vec<PathBuf>,
    /// 依赖文件（仅编译任务）。
    pub depfile: Option<PathBuf>,
    /// 产物。
    pub output: PathBuf,
    /// 完整命令。
    pub command: Vec<String>,
    /// 是否允许并行。
    pub parallel: bool,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileKind {
    /// 单个源文件编译为 .o。
    Obj,
    /// unity build：多个源文件合并编译为一个 .o。
    UnityObj,
    /// 链接可执行/动态库。
    Link,
    /// 归档静态库。
    Archive,
    /// 纯命令（钩子）。
    Command,
}

/// 判断路径是否为动态库（含 macOS `.dylib`、Linux `.so`/`.so.N`、Windows `.dll`）。
pub fn is_shared_lib(path: &Path) -> bool {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    name.ends_with(".dylib")
        || name.ends_with(".so")
        || name.ends_with(".dll")
        || name.contains(".so.")
}

/// 判断编译器/链接器/归档器是否 MSVC 风格工具链
///（cl / clang-cl 编译器，link 链接器，lib 归档器；含 .exe 后缀、大小写、路径前缀）。
pub fn is_msvc_compiler(cc: &str) -> bool {
    let stem = cc.rsplit(['/', '\\']).next().unwrap_or(cc).to_lowercase();
    let stem = stem.strip_suffix(".exe").unwrap_or(&stem);
    matches!(stem, "cl" | "clang-cl" | "link" | "lib")
}

/// 判断文件是否 C++ 源。
pub fn is_cxx(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("cc") | Some("cpp") | Some("cxx") | Some("C") | Some("mm") | Some("mxx")
    )
}

/// 判断是否 C 源。
pub fn is_c_source(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("c") | Some("m")
    )
}

/// 一次编译（单文件或 unity 单元）的上下文。
pub struct CompileContext<'a> {
    pub cc: &'a str,
    pub target: &'a ResolvedTarget,
    pub profile: Profile,
    pub is_cxx: bool,
}

impl<'a> CompileContext<'a> {
    /// 追加通用编译选项（优化/标准/警告/宏/头文件路径/标志/depfile/PIC）。
    fn push_common_flags(&self, cmd: &mut Vec<String>) {
        for f in self.profile.opt_flags() {
            cmd.push(f.to_string());
        }
        let t = self.target;
        if self.is_cxx {
            let std = t.cxx_std.as_deref().unwrap_or("c++17");
            cmd.push(format!("-std={std}"));
        } else {
            let std = t.c_std.as_deref().unwrap_or("c11");
            cmd.push(format!("-std={std}"));
        }
        // 警告。
        if t.warnings {
            cmd.push("-Wall".into());
            if self.is_cxx {
                cmd.push("-Wextra".into());
            }
        }
        if t.werror {
            cmd.push("-Werror".into());
        }
        // 宏定义。
        for d in &t.defines {
            cmd.push(format!("-D{d}"));
        }
        // include 目录。
        for inc in &t.include_dirs {
            cmd.push(format!("-I{}", inc.display()));
        }
        // 预编译头（`-include <path>`：必须是两个独立参数）。
        for pch in &t.pch {
            cmd.push("-include".into());
            cmd.push(pch.clone());
        }
        // 编译标志（全局/target 的 flags + 语言专属标志）。
        for f in &t.flags {
            cmd.push(f.clone());
        }
        let lang_flags = if self.is_cxx { &t.cxxflags } else { &t.cflags };
        for f in lang_flags {
            cmd.push(f.clone());
        }
        // 生成依赖文件 + 位置无关代码。
        cmd.push("-MMD".into());
        cmd.push("-MF".into());
    }
}

/// 构建编译单源文件的命令。
pub fn compile_command(
    ctx: &CompileContext<'_>,
    src: &Path,
    out: &Path,
    depfile: &Path,
) -> Vec<String> {
    if is_msvc_compiler(ctx.cc) {
        return compile_command_msvc(ctx, src, out);
    }
    let mut cmd = vec![ctx.cc.to_string()];
    ctx.push_common_flags(&mut cmd);
    cmd.push(depfile.to_string_lossy().into_owned());
    cmd.push("-fPIC".into());
    cmd.push("-c".into());
    cmd.push(src.to_string_lossy().into_owned());
    cmd.push("-o".into());
    cmd.push(out.to_string_lossy().into_owned());
    cmd
}

/// MSVC（cl / clang-cl）编译命令：/c /O2 等。不产出 depfile——
/// MSVC 没有 `-MMD` 等价物（`/sourceDependencies` 是 JSON 格式），
/// MSVC 风格下增量检测仅基于源文件与命令指纹。
fn compile_command_msvc(ctx: &CompileContext<'_>, src: &Path, out: &Path) -> Vec<String> {
    let t = ctx.target;
    let mut cmd = vec![ctx.cc.to_string(), "/nologo".into(), "/c".into()];
    for f in ctx.profile.msvc_opt_flags() {
        cmd.push(f.to_string());
    }
    // 编译标准。
    if ctx.is_cxx {
        let std = t.cxx_std.as_deref().unwrap_or("c++17");
        let mapped = match std {
            "c++14" | "gnu++14" => "/std:c++14".to_string(),
            "c++17" | "gnu++17" => "/std:c++17".to_string(),
            "c++20" | "gnu++20" => "/std:c++20".to_string(),
            "c++23" | "gnu++23" => "/std:c++latest".to_string(),
            _ => "/std:c++17".to_string(),
        };
        cmd.push(mapped);
    } else {
        let std = t.c_std.as_deref().unwrap_or("c11");
        let mapped = match std {
            "c11" => "/std:c11",
            "c17" | "c23" | "gnu11" | "gnu17" | "gnu23" => "/std:clatest",
            _ => "/std:c11",
        };
        cmd.push(mapped.to_string());
    }
    // 警告。
    cmd.push(if t.warnings { "/W3".into() } else { "/w".into() });
    if t.werror {
        cmd.push("/WX".into());
    }
    // 宏定义。
    for d in &t.defines {
        cmd.push(format!("/D{d}"));
    }
    // include 目录。
    for inc in &t.include_dirs {
        cmd.push(format!("/I{}", inc.display()));
    }
    // 预编译头（/FI：强制包含，与 -include 等价语义）。
    for pch in &t.pch {
        cmd.push(format!("/FI{pch}"));
    }
    // 编译标志（原样透传）。
    for f in &t.flags {
        cmd.push(f.clone());
    }
    let lang_flags = if ctx.is_cxx { &t.cxxflags } else { &t.cflags };
    for f in lang_flags {
        cmd.push(f.clone());
    }
    cmd.push(format!("/Fo{}", out.display()));
    cmd.push(src.to_string_lossy().into_owned());
    cmd
}

/// unity build：将多个源文件合成一个合并单元（`#include` 全部源），
/// 编译为单个 `.o`，大幅减少编译启动开销。
pub fn unity_compile_command(
    ctx: &CompileContext<'_>,
    unity_src: &Path,
    out: &Path,
    depfile: &Path,
) -> Vec<String> {
    if is_msvc_compiler(ctx.cc) {
        return compile_command_msvc(ctx, unity_src, out);
    }
    let mut cmd = vec![ctx.cc.to_string()];
    ctx.push_common_flags(&mut cmd);
    cmd.push(depfile.to_string_lossy().into_owned());
    cmd.push("-fPIC".into());
    cmd.push("-c".into());
    cmd.push(unity_src.to_string_lossy().into_owned());
    cmd.push("-o".into());
    cmd.push(out.to_string_lossy().into_owned());
    cmd
}

/// 生成 unity 合并源文件内容。
pub fn unity_source_content(srcs: &[PathBuf]) -> String {
    let mut s = String::from("/* unity build generated by ns-build */\n");
    for src in srcs {
        let path = src.to_string_lossy().replace('\\', "/");
        s.push_str(&format!("#include \"{path}\"\n"));
    }
    s
}

/// 链接命令（可执行 / 动态库）。
pub fn link_command(
    target: &ResolvedTarget,
    objs: &[PathBuf],
    out: &Path,
    host: Host,
    profile: Profile,
) -> Vec<String> {
    let is_cpp = matches!(target.language, crate::script::Language::Cpp);
    let linker = if is_cpp { target.cxx.clone() } else { target.cc.clone() };
    if is_msvc_compiler(&linker) {
        return link_command_msvc(target, objs, out, profile);
    }
    let mut cmd = vec![linker];

    for f in profile.opt_flags() {
        cmd.push(f.to_string());
    }
    // 库搜索目录。
    for d in &target.link_dirs {
        cmd.push(format!("-L{}", d.display()));
    }
    for f in &target.flags {
        cmd.push(f.clone());
    }
    for f in &target.link_flags {
        cmd.push(f.clone());
    }
    for o in objs {
        cmd.push(o.to_string_lossy().into_owned());
    }
    // 直接链接动态库时写入构建树 rpath，保证未安装前也能运行。
    let mut rpaths: Vec<PathBuf> = Vec::new();
    if !host.os.is_windows_family() {
        for o in objs {
            if is_shared_lib(o) {
                if let Some(parent) = o.parent() {
                    if !parent.as_os_str().is_empty() && !rpaths.contains(&parent.to_path_buf()) {
                        rpaths.push(parent.to_path_buf());
                    }
                }
            }
        }
        for p in rpaths {
            cmd.push(format!("-Wl,-rpath,{}", p.display()));
        }
        // 用户声明的 rpath（如 $ORIGIN/../lib、/usr/local/lib）。
        for rp in &target.rpath {
            cmd.push(format!("-Wl,-rpath,{rp}"));
        }
        // soversion → soname / install_name（unix；MSVC 没有该概念）。
        if target.soversion.is_some() {
            let fname = out
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("lib.so")
                .to_string();
            match host.os {
                crate::platform::Os::Macos | crate::platform::Os::Ios => {
                    cmd.push(format!("-Wl,-install_name,@rpath/{fname}"))
                }
                _ => cmd.push(format!("-Wl,-soname,{fname}")),
            }
        }
    }
    for l in &target.link {
        // 已带路径或绝对路径直接用，否则 `-l`。
        if l.starts_with('/') || l.starts_with('.') || l.contains('/') {
            cmd.push(l.clone());
        } else {
            cmd.push(format!("-l{l}"));
        }
    }
    // pkg-config 包：展开 `pkg-config --libs <pkg>`。
    for pkg in &target.pkg_config {
        let out = std::process::Command::new("pkg-config")
            .args(["--libs", pkg])
            .output();
        match out {
            Ok(o) if o.status.success() => {
                let s = String::from_utf8_lossy(&o.stdout);
                cmd.extend(s.split_whitespace().map(|s| s.to_string()));
            }
            // 找不到包或运行失败：回退为 `-l<pkg>`，不阻塞构建。
            _ => cmd.push(format!("-l{pkg}")),
        }
    }
    if target.strip {
        cmd.push("-s".into());
    }
    match target.kind {
        crate::script::TargetKind::Lib | crate::script::TargetKind::Module => {
            cmd.push("-shared".into());
            cmd.push("-o".into());
            cmd.push(out.to_string_lossy().into_owned());
            if host.os.is_windows_family() {
                // 导出全部符号（简化处理）。
                cmd.push("-Wl,--export-all-symbols".into());
            }
        }
        crate::script::TargetKind::Exe => {
            cmd.push("-o".into());
            cmd.push(out.to_string_lossy().into_owned());
        }
        _ => {
            cmd.push("-o".into());
            cmd.push(out.to_string_lossy().into_owned());
        }
    }
    cmd
}

/// MSVC 库名映射：裸名 `foo` → `foo.lib`；带路径或已带库后缀的原样保留。
pub fn msvc_lib_name(lib: &str) -> String {
    if lib.contains('/')
        || lib.contains('\\')
        || lib.starts_with('-')
        || [".lib", ".obj", ".a", ".dll", ".dylib", ".so"]
            .iter()
            .any(|s| lib.to_lowercase().ends_with(s))
    {
        return lib.to_string();
    }
    format!("{lib}.lib")
}

/// MSVC 链接命令（link.exe / lld-link）：/nologo /OUT: /DLL /DEBUG 等。
/// 注意：MSVC 无 rpath（运行时 DLL 依赖通过 PATH/SxS 解决）。
fn link_command_msvc(
    target: &ResolvedTarget,
    objs: &[PathBuf],
    out: &Path,
    profile: Profile,
) -> Vec<String> {
    let mut cmd = vec!["link".to_string(), "/nologo".into()];
    for o in objs {
        cmd.push(o.to_string_lossy().into_owned());
    }
    match target.kind {
        crate::script::TargetKind::Lib | crate::script::TargetKind::Module => {
            cmd.push("/DLL".into());
        }
        _ => {}
    }
    // 调试配置 → /DEBUG（生成 .pdb）。
    if matches!(profile, Profile::Debug | Profile::RelWithDebInfo) {
        cmd.push("/DEBUG".into());
    }
    for d in &target.link_dirs {
        cmd.push(format!("/LIBPATH:{}", d.display()));
    }
    for l in &target.link {
        cmd.push(msvc_lib_name(l));
    }
    // link_flags 原样透传。
    for f in &target.link_flags {
        cmd.push(f.clone());
    }
    // pkg-config 是 POSIX 工具，MSVC 风格下忽略。
    cmd.push(format!("/OUT:{}", out.display()));
    cmd
}

/// 归档命令（静态库）：MSVC 下用 lib.exe，链接器/归档器名 `lib` 或工具链声明为 MSVC 时启用。
pub fn archive_command(ar: &str, objs: &[PathBuf], out: &Path, msvc: bool) -> Vec<String> {
    if msvc || is_msvc_compiler(ar) {
        let mut cmd = vec![ar.to_string(), "/nologo".into()];
        cmd.push(format!("/OUT:{}", out.display()));
        for o in objs {
            cmd.push(o.to_string_lossy().into_owned());
        }
        return cmd;
    }
    let mut cmd = vec![ar.to_string()];
    cmd.push("rcs".into());
    cmd.push(out.to_string_lossy().into_owned());
    for o in objs {
        cmd.push(o.to_string_lossy().into_owned());
    }
    cmd
}

/// 从目标 + 输入构建链接/打包任务（按语言分派）。
pub fn make_link_task(
    target: &ResolvedTarget,
    inputs: Vec<PathBuf>,
    host: Host,
    profile: Profile,
    ar: &str,
) -> CompileTask {
    use crate::script::{Language, TargetKind};

    let cmd = match target.language {
        Language::Rust => rust_build_command(target, &inputs),
        Language::Go => go_build_command(target, &inputs),
        Language::Java => java_build_command(target, &inputs),
        Language::Zig => zig_build_command(target, &inputs),
        Language::Python => script_package_command(target, &inputs, host),
        Language::Shell => script_package_command(target, &inputs, host),
        // C / C++：原有编译链接。
        _ => match target.kind {
            TargetKind::Static => {
                let is_cpp_target = matches!(target.language, crate::script::Language::Cpp);
                let probe = if is_cpp_target { &target.cxx } else { &target.cc };
                archive_command(ar, &inputs, &target.output, is_msvc_compiler(probe))
            }
            _ => link_command(target, &inputs, &target.output, host, profile),
        },
    };
    CompileTask {
        key: format!("link:{}", target.name),
        kind: match target.kind {
            TargetKind::Static => CompileKind::Archive,
            _ => CompileKind::Link,
        },
        inputs,
        depfile: None,
        output: target.output.clone(),
        command: cmd,
        parallel: target.parallel,
        label: format!("LINK {}", target.name),
    }
}

/// Rust：`rustc <主源> -o <out>`。
fn rust_build_command(target: &ResolvedTarget, srcs: &[PathBuf]) -> Vec<String> {
    let mut cmd = vec!["rustc".to_string()];
    cmd.push("--edition".into());
    cmd.push("2021".into());
    match target.kind {
        crate::script::TargetKind::Lib | crate::script::TargetKind::Module => {
            cmd.push("--crate-type".into());
            cmd.push("cdylib".into());
        }
        crate::script::TargetKind::Static => {
            cmd.push("--crate-type".into());
            cmd.push("staticlib".into());
        }
        _ => {}
    }
    for d in &target.link_dirs {
        cmd.push("-L".into());
        cmd.push(d.to_string_lossy().into_owned());
    }
    for f in &target.flags {
        cmd.push(f.clone());
    }
    if let Some(main) = srcs.first() {
        cmd.push(main.to_string_lossy().into_owned());
    }
    for l in &target.link {
        if l.starts_with('/') || l.starts_with('.') || l.contains('/') {
            cmd.push(l.clone());
        } else {
            cmd.push(format!("-l{l}"));
        }
    }
    cmd.push("-o".into());
    cmd.push(target.output.to_string_lossy().into_owned());
    cmd
}

/// Go：`go build -o <out> <源...>`。
fn go_build_command(target: &ResolvedTarget, srcs: &[PathBuf]) -> Vec<String> {
    let mut cmd = vec!["go".to_string(), "build".into()];
    for f in &target.flags {
        cmd.push(f.clone());
    }
    cmd.push("-o".into());
    cmd.push(target.output.to_string_lossy().into_owned());
    for s in srcs {
        cmd.push(s.to_string_lossy().into_owned());
    }
    cmd
}

/// Java：`javac -d <out>/classes <源...>`。
fn java_build_command(target: &ResolvedTarget, srcs: &[PathBuf]) -> Vec<String> {
    let mut cmd = vec!["javac".to_string()];
    for f in &target.flags {
        cmd.push(f.clone());
    }
    let classes = target
        .output
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("classes");
    cmd.push("-d".into());
    cmd.push(classes.to_string_lossy().into_owned());
    for s in srcs {
        cmd.push(s.to_string_lossy().into_owned());
    }
    cmd
}

/// Zig：`zig build-exe` / `build-lib`。
fn zig_build_command(target: &ResolvedTarget, srcs: &[PathBuf]) -> Vec<String> {
    let mut cmd = vec!["zig".to_string()];
    if target.kind == crate::script::TargetKind::Lib {
        cmd.push("build-lib".into());
    } else {
        cmd.push("build-exe".into());
    }
    for f in &target.flags {
        cmd.push(f.clone());
    }
    for s in srcs {
        cmd.push(s.to_string_lossy().into_owned());
    }
    cmd.push(format!("-femit-bin={}", target.output.to_string_lossy()));
    cmd
}

/// Python/Shell：拷贝源为可执行脚本。
fn script_package_command(target: &ResolvedTarget, srcs: &[PathBuf], host: Host) -> Vec<String> {
    let src = srcs
        .first()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let out = target.output.to_string_lossy().into_owned();
    if host.os.is_windows_family() {
        // Windows 无 chmod；copy /Y 保留原文件内容。
        vec![
            "cmd".into(),
            "/C".into(),
            format!("copy /Y \"{src}\" \"{out}\""),
        ]
    } else {
        vec![
            "sh".into(),
            "-c".into(),
            format!("cp -- '{}' '{}' && chmod +x '{}'", shell_quote(&src), shell_quote(&out), shell_quote(&out)),
        ]
    }
}

/// 单引号 shell 转义。
fn shell_quote(s: &str) -> String {
    s.replace('\'', "'\\''")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{Language, TargetKind};

    fn target(name: &str, kind: TargetKind) -> ResolvedTarget {
        ResolvedTarget {
            name: name.into(),
            kind,
            cc: "cc".into(),
            cxx: "c++".into(),
            ar: "ar".into(),
            host: Host::current(),
            sources: vec![],
            include_dirs: vec![],
            flags: vec![],
            cflags: vec![],
            cxxflags: vec![],
            link: vec![],
            link_dirs: vec![],
            link_flags: vec![],
            defines: vec![],
            deps: vec![],
            private_deps: vec![],
            output: PathBuf::from(format!("out/{name}")),
            groups: None,
            group_size: None,
            parallel: true,
            hooks: Default::default(),
            profile: Profile::Release,
            strip: false,
            warnings: true,
            werror: false,
            rpath: vec![],
            soversion: None,
            install: true,
            language: Language::C,
            c_std: None,
            cxx_std: None,
            pkg_config: vec![],
            when: None,
            pch: vec![],
        }
    }

    #[test]
    fn compile_has_depfile_and_pic() {
        let t = target("a", TargetKind::Exe);
        let cmd = compile_command(
            &CompileContext {
                cc: "gcc",
                target: &t,
                profile: Profile::Release,
                is_cxx: false,
            },
            Path::new("a.c"),
            Path::new("a.o"),
            Path::new("a.d"),
        );
        assert!(cmd.iter().any(|c| c == "-c"));
        assert!(cmd.iter().any(|c| c == "-MMD"));
        assert!(cmd.iter().any(|c| c.starts_with("-MF")));
        assert!(cmd.iter().any(|c| c == "-fPIC"));
    }

    #[test]
    fn link_exe_uses_o_flag() {
        let t = target("a", TargetKind::Exe);
        let cmd = link_command(&t, &[PathBuf::from("a.o")], &t.output, Host::current(), Profile::Release);
        assert_eq!(cmd[0], "cc");
        assert!(cmd.contains(&"a.o".to_string()));
        assert!(cmd.contains(&"-o".to_string()));
    }

    #[test]
    fn archive_uses_rcs() {
        let t = target("a", TargetKind::Static);
        let cmd = archive_command("ar", &[PathBuf::from("a.o")], &t.output, false);
        assert_eq!(cmd[0], "ar");
        assert_eq!(cmd[1], "rcs");
    }

    #[test]
    fn is_cxx_detects() {
        assert!(is_cxx(Path::new("a.cpp")));
        assert!(!is_cxx(Path::new("a.c")));
    }

    #[test]
    fn rpath_flags_emitted() {
        let mut t = target("a", TargetKind::Exe);
        t.rpath = vec!["/opt/lib".into(), "$ORIGIN/../lib".into()];
        let cmd = link_command(
            &t,
            &[PathBuf::from("a.o")],
            &PathBuf::from("a"),
            Host::current(),
            Profile::Release,
        );
        assert!(cmd.contains(&"-Wl,-rpath,/opt/lib".to_string()));
        assert!(cmd.contains(&"-Wl,-rpath,$ORIGIN/../lib".to_string()));
    }

    #[test]
    fn werror_adds_flag() {
        let mut t = target("a", TargetKind::Exe);
        t.werror = true;
        let cmd = compile_command(
            &CompileContext {
                cc: "cc",
                target: &t,
                profile: Profile::Release,
                is_cxx: false,
            },
            Path::new("a.c"),
            Path::new("a.o"),
            Path::new("a.d"),
        );
        assert!(cmd.contains(&"-Werror".to_string()));
        // MSVC → /WX
        let mut t2 = target("a", TargetKind::Exe);
        t2.cc = "cl".into();
        t2.werror = true;
        let cmd2 = compile_command(
            &CompileContext {
                cc: "cl",
                target: &t2,
                profile: Profile::Release,
                is_cxx: true,
            },
            Path::new("a.cpp"),
            Path::new("a.obj"),
            Path::new("a.d"),
        );
        assert!(cmd2.contains(&"/WX".to_string()));
    }

    #[test]
    fn pch_multiple_headers() {
        let mut t = target("a", TargetKind::Exe);
        t.pch = vec!["a.h".into(), "b.h".into()];
        let cmd = compile_command(
            &CompileContext {
                cc: "cc",
                target: &t,
                profile: Profile::Release,
                is_cxx: false,
            },
            Path::new("a.c"),
            Path::new("a.o"),
            Path::new("a.d"),
        );
        let includes: Vec<usize> = cmd
            .iter()
            .enumerate()
            .filter(|(_, c)| *c == "-include")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(includes.len(), 2);
        assert_eq!(cmd[includes[0] + 1], "a.h");
        assert_eq!(cmd[includes[1] + 1], "b.h");
    }

    #[test]
    fn pch_uses_two_arguments() {
        let mut t = target("a", TargetKind::Exe);
        t.pch = vec!["include/pre.h".into()];
        let cmd = compile_command(
            &CompileContext {
                cc: "cc",
                target: &t,
                profile: Profile::Release,
                is_cxx: false,
            },
            Path::new("a.c"),
            Path::new("a.o"),
            Path::new("a.d"),
        );
        let pos = cmd.iter().position(|c| c == "-include").unwrap();
        assert_eq!(cmd[pos + 1], "include/pre.h");
    }

    #[test]
    fn cxx_uses_cxxflags_and_compiler() {
        let mut t = target("a", TargetKind::Exe);
        t.cflags = vec!["-DC_ONLY".into()];
        t.cxxflags = vec!["-DCXX_ONLY".into()];
        let cmd = compile_command(
            &CompileContext {
                cc: "c++",
                target: &t,
                profile: Profile::Release,
                is_cxx: true,
            },
            Path::new("a.cpp"),
            Path::new("a.o"),
            Path::new("a.d"),
        );
        assert!(cmd.contains(&"-DCXX_ONLY".to_string()));
        assert!(!cmd.contains(&"-DC_ONLY".to_string()));
        assert!(cmd.contains(&"-std=c++17".to_string()));
    }

    #[test]
    fn warnings_can_be_disabled() {
        let mut t = target("a", TargetKind::Exe);
        t.warnings = false;
        let cmd = compile_command(
            &CompileContext {
                cc: "cc",
                target: &t,
                profile: Profile::Release,
                is_cxx: false,
            },
            Path::new("a.c"),
            Path::new("a.o"),
            Path::new("a.d"),
        );
        assert!(!cmd.contains(&"-Wall".to_string()));
    }

    #[test]
    fn msvc_detection() {
        for cc in ["cl", "cl.exe", "CL", "clang-cl", "clang-cl.exe", "/opt/x/clang-cl", "lib", "link"] {
            assert!(is_msvc_compiler(cc), "{cc} 应识别为 MSVC 风格");
        }
        for cc in ["cc", "gcc", "clang", "c++", "ar"] {
            assert!(!is_msvc_compiler(cc), "{cc} 不应识别为 MSVC 风格");
        }
    }

    #[test]
    fn msvc_compile_command_shape() {
        let mut t = target("a", TargetKind::Exe);
        t.defines = vec!["FOO=1".into(), "BAR".into()];
        t.include_dirs = vec![PathBuf::from("/inc")];
        t.pch = vec!["pch.h".into()];
        let cmd = compile_command(
            &CompileContext {
                cc: "cl",
                target: &t,
                profile: Profile::Release,
                is_cxx: false,
            },
            Path::new("a.c"),
            Path::new("build/a.obj"),
            Path::new("unused.d"),
        );
        assert_eq!(cmd[0], "cl");
        assert!(cmd.contains(&"/nologo".to_string()));
        assert!(cmd.contains(&"/c".to_string()));
        assert!(cmd.contains(&"/O2".to_string()));
        assert!(cmd.contains(&"/std:c11".to_string()));
        assert!(cmd.contains(&"/W3".to_string()));
        assert!(cmd.contains(&"/DFOO=1".to_string()));
        assert!(cmd.contains(&"/DBAR".to_string()));
        assert!(cmd.contains(&"/I/inc".to_string()));
        assert!(cmd.contains(&"/FIpch.h".to_string()));
        assert!(cmd.contains(&"/Fobuild/a.obj".to_string()));
        // MSVC 没有 -MMD/-c/-o/-fPIC 这些 GNU 风格参数。
        assert!(!cmd.iter().any(|c| c == "-c" || c == "-MMD" || c == "-fPIC" || c == "-o"));
        // 警告关闭 → /w
        t.warnings = false;
        let cmd2 = compile_command(
            &CompileContext {
                cc: "cl",
                target: &t,
                profile: Profile::Debug,
                is_cxx: true,
            },
            Path::new("a.cpp"),
            Path::new("a.obj"),
            Path::new("unused.d"),
        );
        assert!(cmd2.contains(&"/w".to_string()));
        assert!(cmd2.contains(&"/Od".to_string()));
        assert!(cmd2.contains(&"/Zi".to_string()));
        assert!(cmd2.contains(&"/std:c++17".to_string()));
    }

    #[test]
    fn msvc_link_command_shape() {
        let mut t = target("a", TargetKind::Exe);
        t.cc = "cl".into();
        t.cxx = "cl".into();
        t.link = vec!["m".into(), "shell32".into(), "/opt/lib/foo.lib".into()];
        t.link_dirs = vec![PathBuf::from("/opt/lib")];
        let cmd = link_command(
            &t,
            &[PathBuf::from("a.obj")],
            &PathBuf::from("app.exe"),
            Host::current(),
            Profile::Debug,
        );
        assert_eq!(cmd[0], "link");
        assert!(cmd.contains(&"/nologo".to_string()));
        assert!(cmd.contains(&"/DEBUG".to_string()));
        assert!(cmd.contains(&"/LIBPATH:/opt/lib".to_string()));
        assert!(cmd.contains(&"m.lib".to_string()));
        assert!(cmd.contains(&"shell32.lib".to_string()));
        assert!(cmd.contains(&"/opt/lib/foo.lib".to_string()));
        assert!(cmd.contains(&"/OUT:app.exe".to_string()));
        // 动态库 → /DLL
        t.kind = TargetKind::Lib;
        let dll = link_command(
            &t,
            &[PathBuf::from("a.obj")],
            &PathBuf::from("a.dll"),
            Host::current(),
            Profile::Release,
        );
        assert!(dll.contains(&"/DLL".to_string()));
    }

    #[test]
    fn msvc_archive_command_shape() {
        let cmd = archive_command("lib", &[PathBuf::from("a.obj")], Path::new("foo.lib"), true);
        assert_eq!(cmd[0], "lib");
        assert!(cmd.contains(&"/nologo".to_string()));
        assert!(cmd.contains(&"/OUT:foo.lib".to_string()));
        assert!(cmd.contains(&"a.obj".to_string()));
    }

    #[test]
    fn msvc_lib_name_mapping() {
        assert_eq!(msvc_lib_name("m"), "m.lib");
        assert_eq!(msvc_lib_name("shell32"), "shell32.lib");
        assert_eq!(msvc_lib_name("C:/sdk/foo.lib"), "C:/sdk/foo.lib");
        assert_eq!(msvc_lib_name("/abs/bar.a"), "/abs/bar.a");
        assert_eq!(msvc_lib_name("./rel.obj"), "./rel.obj");
    }
}
