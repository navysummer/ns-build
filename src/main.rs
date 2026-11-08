//! ns-build CLI：build / clean / test / install / run / show / init / graph。

use std::path::PathBuf;
use std::process::ExitCode;

use ns_build::cache::Cache;
use ns_build::executor::{ExecOptions, execute};
use ns_build::install::{InstallReport, install_all};
use ns_build::planner::plan;
use ns_build::runner::{find_exe, run_one, run_tests};
use ns_build::script::load;

const USAGE: &str = r#"ns-build — 现代化跨平台构建系统

用法:
  ns-build build [--release|--debug|--profile <配置>] [-j N] [--verbose] [--quiet] [目标...]
  ns-build clean
  ns-build test [--all] [--label <标签>] [-j N]
  ns-build install [--prefix <路径>]
  ns-build run <目标> [-- 参数...]
  ns-build show
  ns-build graph
  ns-build init [--name <名>]

选项:
  -j, --jobs <N>      并行任务数 (默认: CPU 核数; 支持 -j4/-j8/-j100)
  --release           优化构建 (默认)
  --debug             调试构建
  --profile <配置>    release | debug | relwithdebinfo | minsizerel
  --verbose           打印完整命令
  --quiet             只输出错误
  -h, --help          帮助
  -V, --version       版本
"#;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match run(&args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("[ns-build] 错误: {e}");
            1
        }
    };
    ExitCode::from(code as u8)
}

fn run(args: &[String]) -> Result<(), String> {
    let Some(cmd) = args.first() else {
        print!("{USAGE}");
        return Ok(());
    };
    match cmd.as_str() {
        "build" | "b" => cmd_build(&args[1..]),
        "clean" => cmd_clean(),
        "test" | "t" => cmd_test(&args[1..]),
        "install" => cmd_install(&args[1..]),
        "run" => cmd_run(&args[1..]),
        "show" => cmd_show(),
        "graph" => cmd_graph(),
        "init" => cmd_init(&args[1..]),
        "help" | "-h" | "--help" => {
            print!("{USAGE}");
            Ok(())
        }
        "version" | "-V" | "--version" => {
            println!("ns-build {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => Err(format!("未知命令 '{other}'\n{USAGE}")),
    }
}

/// 解析 -j / --release / --debug / --quiet / --verbose / 目标列表。
struct BuildArgs {
    jobs: Option<usize>,
    /// None = 使用脚本中的 `profile` / 默认 release。
    profile: Option<ns_build::script::Profile>,
    quiet: bool,
    verbose: bool,
    targets: Vec<String>,
    /// CLI 变量覆盖：`--define name=value`。
    defines: std::collections::BTreeMap<String, String>,
}

fn parse_build_args(args: &[String]) -> Result<BuildArgs, String> {
    use ns_build::script::Profile;
    let mut out = BuildArgs {
        jobs: None,
        profile: None,
        quiet: false,
        verbose: false,
        targets: vec![],
        defines: std::collections::BTreeMap::new(),
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-j" | "--jobs" | "--parallel" => {
                i += 1;
                let v = args.get(i).ok_or("选项后需要并行数参数")?;
                out.jobs = Some(v.parse().map_err(|_| format!("无效并行数 '{v}'"))?);
            }
            t if parse_jobs_arg(t).is_some() => {
                out.jobs = parse_jobs_arg(t);
            }
            "--release" => out.profile = Some(Profile::Release),
            "--debug" => out.profile = Some(Profile::Debug),
            "--profile" => {
                i += 1;
                let v = args.get(i).ok_or("--profile 需要参数 (release|debug|relwithdebinfo|minsizerel)")?;
                out.profile = Some(match v.as_str() {
                    "release" => Profile::Release,
                    "debug" => Profile::Debug,
                    "relwithdebinfo" | "relwithdeb" => Profile::RelWithDebInfo,
                    "minsizerel" | "minsize" => Profile::MinSizeRel,
                    other => return Err(format!("未知配置 '{other}'")),
                });
            }
            "--quiet" | "-q" => out.quiet = true,
            "--verbose" | "-v" => out.verbose = true,
            t if t.starts_with("--define") => {
                let v = t.strip_prefix("--define=")
                    .or_else(|| t.strip_prefix("--define"))
                    .unwrap_or("");
                if let Some((name, value)) = v.split_once('=') {
                    out.defines.insert(name.to_string(), value.to_string());
                } else if !v.is_empty() {
                    out.defines.insert(v.to_string(), "1".to_string());
                } else {
                    return Err("--define 需要 name=value".into());
                }
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            t if t.starts_with('-') => return Err(format!("未知选项 '{t}'")),
            t => out.targets.push(t.to_string()),
        }
        i += 1;
    }
    Ok(out)
}

/// 正则识别 `-j4`、`-j8`、`-j100`、`--jobs=8`、`--parallel=4`。
/// 返回 Some(并行数)；None 表示不是 jobs 参数。
fn parse_jobs_arg(t: &str) -> Option<usize> {
    if let Some(rest) = t.strip_prefix("-j") {
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
            return rest.parse().ok();
        }
    }
    for prefix in ["--jobs=", "--parallel="] {
        if let Some(rest) = t.strip_prefix(prefix) {
            if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
                return rest.parse().ok();
            }
        }
    }
    None
}

/// 展开目标列表：加入递归 `deps`，返回可用于任务过滤的目标名集合。
fn expand_targets(
    script: &ns_build::script::ResolvedScript,
    requested: &[String],
) -> Result<Vec<String>, String> {
    use std::collections::{BTreeMap, BTreeSet};
    let by_name: BTreeMap<&str, &ns_build::script::ResolvedTarget> =
        script.targets.iter().map(|t| (t.name.as_str(), t)).collect();
    let mut allowed: BTreeSet<String> = BTreeSet::new();
    let mut stack: Vec<String> = Vec::new();
    for name in requested {
        let t = by_name
            .get(name.as_str())
            .ok_or_else(|| format!("未知目标 '{name}'"))?;
        stack.push(t.name.clone());
    }
    while let Some(name) = stack.pop() {
        if !allowed.insert(name.clone()) {
            continue;
        }
        if let Some(t) = by_name.get(name.as_str()) {
            for dep in &t.deps {
                if !allowed.contains(dep) {
                    stack.push(dep.clone());
                }
            }
        }
    }
    Ok(allowed.into_iter().collect())
}



fn cmd_build(args: &[String]) -> Result<(), String> {
    let ba = parse_build_args(args)?;
    let dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let (script, script_dir) = load(&dir)?;
    let resolved = script.resolve(&script_dir, &ba.defines, ba.profile)?;
    // 指定目标时自动包含其（递归）依赖。
    let targets = if ba.targets.is_empty() {
        None
    } else {
        Some(expand_targets(&resolved, &ba.targets)?)
    };
    let mut cache = Cache::load(&resolved.cache_dir)?;
    let mut p = plan(&resolved, &cache)?;

    let opts = ExecOptions {
        parallel: ba.jobs.unwrap_or(resolved.parallel),
        quiet: ba.quiet,
        verbose: ba.verbose,
        targets,
    };
    let report = execute(&mut p, &mut cache, &opts, &resolved.dir)?;
    cache.save()?;
    // 生成 compile_commands.json（供 clangd / IDE）。
    let cc_json = ns_build::planner::generate_compile_commands(&p, &resolved.dir);
    let cc_path = resolved.out_dir.join("compile_commands.json");
    if let Some(parent) = cc_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&cc_path, cc_json);

    println!(
        "[ns-build] 完成: {} 构建, {} 跳过, {} 失败 / {} 任务, {:.2}s",
        report.built, report.skipped, report.failed, report.total, report.elapsed
    );
    if report.failed > 0 {
        return Err(format!("{} 个任务失败", report.failed));
    }
    Ok(())
}

fn cmd_clean() -> Result<(), String> {
    let dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let (script, script_dir) = load(&dir)?;
    let resolved = script.resolve_default(&script_dir)?;
    // 删除 out 目录与缓存。
    for p in [&resolved.out_dir, &resolved.cache_dir] {
        if p.exists() {
            std::fs::remove_dir_all(p)
                .map_err(|e| format!("删除 {} 失败: {e}", p.display()))?;
            println!("[ns-build] 已删除 {}", p.display());
        }
    }
    println!("[ns-build] clean 完成");
    Ok(())
}

fn cmd_test(args: &[String]) -> Result<(), String> {
    let mut all = false;
    let mut labels: Vec<String> = vec![];
    let mut jobs: Option<usize> = None;
    let mut profile_args: Vec<String> = vec![];
    let mut verbose = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--all" => all = true,
            "--label" => {
                i += 1;
                let v = args.get(i).ok_or("--label 需要标签名")?;
                labels.push(v.clone());
            }
            v if v.starts_with("--label=") => labels.push(v["--label=".len()..].to_string()),
            "-j" | "--jobs" | "--parallel" => {
                i += 1;
                let v = args.get(i).ok_or("选项后需要并行数参数")?;
                jobs = Some(v.parse().map_err(|_| format!("无效并行数 '{v}'"))?);
            }
            t if parse_jobs_arg(t).is_some() => jobs = parse_jobs_arg(t),
            "--release" | "--debug" => profile_args.push(args[i].clone()),
            "--profile" => {
                i += 1;
                profile_args.push("--profile".into());
                profile_args.push(
                    args.get(i).ok_or("--profile 需要参数")?.clone(),
                );
            }
            "--verbose" | "-v" => {
                verbose = true;
                profile_args.push("--verbose".into());
            }
            "--quiet" | "-q" => profile_args.push("--quiet".into()),
            v if v.starts_with('-') => return Err(format!("未知选项 '{v}'")),
            _ => {}
        }
        i += 1;
    }
    // 先构建，再运行测试。
    let mut build_args = profile_args;
    if !verbose {
        build_args.push("--quiet".to_string());
    }
    if let Some(j) = jobs {
        build_args.push("-j".to_string());
        build_args.push(j.to_string());
    }
    cmd_build(&build_args)?;
    let dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let (script, script_dir) = load(&dir)?;
    let resolved = script.resolve_default(&script_dir)?;
    let report = run_tests(&resolved, all, &labels)?;
    println!(
        "[ns-build] 测试: {} 通过, {} 失败, {} 跳过",
        report.passed, report.failed, report.skipped
    );
    if report.failed > 0 {
        return Err(format!("{} 个测试失败", report.failed));
    }
    Ok(())
}

fn cmd_install(args: &[String]) -> Result<(), String> {
    let mut prefix: Option<PathBuf> = None;
    let mut build_args = vec!["--quiet".to_string()];
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--prefix" => {
                i += 1;
                prefix = Some(PathBuf::from(args.get(i).ok_or("--prefix 需要参数")?));
            }
            "-j" | "--jobs" | "--parallel" => {
                i += 1;
                let v = args.get(i).ok_or("选项后需要并行数参数")?;
                build_args.push("-j".into());
                build_args.push(v.clone());
            }
            t if parse_jobs_arg(t).is_some() => {
                build_args.push(t.to_string());
            }
            "--profile" => {
                i += 1;
                build_args.push("--profile".into());
                build_args.push(args.get(i).ok_or("--profile 需要参数")?.clone());
            }
            "--release" | "--debug" => build_args.push(args[i].clone()),
            t if t.starts_with("--define") => build_args.push(t.to_string()),
            "--verbose" | "-v" => {
                build_args.push("--verbose".into());
                build_args.retain(|a| a != "--quiet");
            }
            "-h" | "--help" => {
                println!("用法: ns-build install [--prefix <路径>] [-j N] [--release|--debug] [--define name=value]");
                return Ok(());
            }
            other => return Err(format!("未知选项 '{other}'")),
        }
        i += 1;
    }
    let dir = std::env::current_dir().map_err(|e| e.to_string())?;
    // 先构建，再安装。
    cmd_build(&build_args)?;
    let (script, script_dir) = load(&dir)?;
    let resolved = script.resolve_default(&script_dir)?;
    let prefix = prefix.unwrap_or_else(ns_build::install::default_prefix);
    let report: InstallReport = install_all(&resolved, &prefix)?;
    println!(
        "[ns-build] 安装: {} 已安装, {} 跳过 → {}",
        report.installed, report.skipped, prefix.display()
    );
    Ok(())
}

fn cmd_run(args: &[String]) -> Result<(), String> {
    let mut target = None;
    let mut run_args: Vec<String> = vec![];
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--" {
            run_args = args[i + 1..].to_vec();
            break;
        }
        if target.is_none() {
            target = Some(args[i].clone());
        }
        i += 1;
    }
    let target = target.ok_or("用法: ns-build run <目标> [-- 参数...]")?;
    // 先构建（含依赖），再运行。
    cmd_build(&[target.clone(), "--quiet".to_string()])?;
    let dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let (script, script_dir) = load(&dir)?;
    let resolved = script.resolve_default(&script_dir)?;
    let Some(path) = find_exe(&resolved, &target) else {
        return Err(format!("找不到可执行目标 '{target}'"));
    };
    if !path.exists() {
        return Err(format!("目标 '{target}' 未构建成功"));
    }
    let status = run_one(path, &run_args)?;
    if !status.success() {
        return Err(format!("目标 '{target}' 退出码 {:?}", status.code()));
    }
    Ok(())
}

fn cmd_show() -> Result<(), String> {
    let dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let (script, script_dir) = load(&dir)?;
    let resolved = script.resolve_default(&script_dir)?;
    println!("project: {} {}", resolved.script.project.name, resolved.script.project.version);
    println!("out: {}", resolved.out_dir.display());
    println!("parallel: {}", resolved.parallel);
    println!("targets:");
    for t in &resolved.targets {
        println!(
            "  - {} [{}] 源文件={} 输出={}",
            t.name,
            kind_str(t.kind),
            t.sources.len(),
            t.output.display()
        );
    }
    Ok(())
}

fn cmd_graph() -> Result<(), String> {
    let dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let (script, script_dir) = load(&dir)?;
    let resolved = script.resolve_default(&script_dir)?;
    let cache = Cache::load(&resolved.cache_dir)?;
    let p = plan(&resolved, &cache)?;
    println!("digraph ns_build {{");
    for (i, node) in p.graph.nodes().iter().enumerate() {
        let shape = match node.kind {
            ns_build::TaskKind::Compile => "box",
            ns_build::TaskKind::Link => "ellipse",
            ns_build::TaskKind::Command => "diamond",
        };
        println!("  n{i} [label=\"{}\", shape={shape}];", node.label.replace('"', "'"));
        for d in p.graph.deps_of(i) {
            println!("  n{d} -> n{i};");
        }
    }
    println!("}}");
    Ok(())
}

fn cmd_init(args: &[String]) -> Result<(), String> {
    let mut name = "app".to_string();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--name" => {
                i += 1;
                name = args.get(i).ok_or("--name 需要参数")?.clone();
            }
            other => return Err(format!("未知选项 '{other}'")),
        }
        i += 1;
    }
    let dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let file = dir.join("ns-build.toml");
    if file.exists() {
        return Err("ns-build.toml 已存在".into());
    }
    let tmpl = format!(
        r#"[project]
name = "{name}"
version = "0.1.0"
root = "src"
out = "build"

[defaults]
cc = "cc"
cxx = "c++"
ar = "ar"
warnings = true
unity = "auto"

[[targets]]
name = "{name}"
kind = "exe"
sources = ["**/*.c"]
link = []
"#
    );
    std::fs::create_dir_all(dir.join("src")).map_err(|e| e.to_string())?;
    let main_c = format!(
        "#include <stdio.h>\nint main(void) {{\n    printf(\"hello from {name}\\n\");\n    return 0;\n}}\n"
    );
    std::fs::write(dir.join("src/main.c"), main_c).map_err(|e| e.to_string())?;
    std::fs::write(&file, tmpl).map_err(|e| format!("写入 ns-build.toml 失败: {e}"))?;
    println!("[ns-build] 初始化完成: {}", file.display());
    Ok(())
}

fn kind_str(k: ns_build::TargetKind) -> &'static str {
    match k {
        ns_build::TargetKind::Exe => "exe",
        ns_build::TargetKind::Lib => "lib",
        ns_build::TargetKind::Static => "static",
        ns_build::TargetKind::Object => "object",
        ns_build::TargetKind::Phony => "phony",
        ns_build::TargetKind::Module => "module",
        ns_build::TargetKind::Interface => "interface",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_arg_forms() {
        assert_eq!(parse_jobs_arg("-j4"), Some(4));
        assert_eq!(parse_jobs_arg("-j8"), Some(8));
        assert_eq!(parse_jobs_arg("-j100"), Some(100));
        assert_eq!(parse_jobs_arg("--jobs=8"), Some(8));
        assert_eq!(parse_jobs_arg("--parallel=4"), Some(4));
        assert_eq!(parse_jobs_arg("-j"), None);
        assert_eq!(parse_jobs_arg("-jabc"), None);
        assert_eq!(parse_jobs_arg("--jobs"), None);
        assert_eq!(parse_jobs_arg("build"), None);
    }

    #[test]
    fn build_args_parses_jobs_forms() {
        let ba = parse_build_args(&["-j4".into(), "app".into()]).unwrap();
        assert_eq!(ba.jobs, Some(4));
        assert_eq!(ba.targets, vec!["app".to_string()]);

        let ba = parse_build_args(&["--jobs=16".into()]).unwrap();
        assert_eq!(ba.jobs, Some(16));

        let ba = parse_build_args(&["-j".into(), "8".into()]).unwrap();
        assert_eq!(ba.jobs, Some(8));
    }

    #[test]
    fn build_args_rejects_unknown() {
        assert!(parse_build_args(&["--bogus".into()]).is_err());
    }
}
