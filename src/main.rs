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
  ns-build build [--release|--debug] [-j N] [--verbose] [--quiet] [目标...]
  ns-build clean
  ns-build test [--all] [-j N]
  ns-build install [--prefix <路径>]
  ns-build run <目标> [-- 参数...]
  ns-build show
  ns-build graph
  ns-build init [--name <名>]

选项:
  -j, --jobs <N>      并行任务数 (默认: CPU 核数)
  --release           优化构建 (默认)
  --debug             调试构建
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
    profile: ProfileFlag,
    quiet: bool,
    verbose: bool,
    targets: Vec<String>,
}


enum ProfileFlag {
    Release,
    Debug,
}

fn parse_build_args(args: &[String]) -> Result<BuildArgs, String> {
    let mut out = BuildArgs {
        jobs: None,
        profile: ProfileFlag::Release,
        quiet: false,
        verbose: false,
        targets: vec![],
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-j" | "--jobs" | "--parallel" => {
                i += 1;
                let v = args.get(i).ok_or("-j 需要参数")?;
                out.jobs = Some(v.parse().map_err(|_| format!("无效并行数 '{v}'"))?);
            }
            "--release" => out.profile = ProfileFlag::Release,
            "--debug" => out.profile = ProfileFlag::Debug,
            "--quiet" | "-q" => out.quiet = true,
            "--verbose" | "-v" => out.verbose = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            t if t.starts_with("-j") && t.len() > 2 => {
                let v = &t[2..];
                out.jobs = Some(v.parse().map_err(|_| format!("无效并行数 '{v}'"))?);
            }
            t if t.starts_with('-') => return Err(format!("未知选项 '{t}'")),
            t => out.targets.push(t.to_string()),
        }
        i += 1;
    }
    Ok(out)
}


fn cmd_build(args: &[String]) -> Result<(), String> {
    let ba = parse_build_args(args)?;
    let dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let (script, script_dir) = load(&dir)?;
    let resolved = script.resolve(&script_dir)?;
    let mut cache = Cache::load(&resolved.cache_dir)?;
    let mut p = plan(&resolved, &cache)?;

    let opts = ExecOptions {
        parallel: ba.jobs.unwrap_or(resolved.parallel),
        quiet: ba.quiet,
        verbose: ba.verbose,
        targets: if ba.targets.is_empty() { None } else { Some(ba.targets) },
    };
    let report = execute(&mut p, &mut cache, &opts, &resolved.dir)?;
    cache.save()?;

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
    let resolved = script.resolve(&script_dir)?;
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
    let jobs: Option<usize> = None;
    for a in args {
        match a.as_str() {
            "--all" => all = true,
            "-j" | "--jobs" | "--parallel" => {
                // 下一项
            }
            v if v.starts_with('-') => return Err(format!("未知选项 '{v}'")),
            _ => {}
        }
    }
    // 先构建（quiet），再运行测试。
    let mut build_args = vec!["--quiet".to_string()];
    if let Some(j) = jobs {
        build_args.push("-j".to_string());
        build_args.push(j.to_string());
    }
    cmd_build(&build_args)?;
    let dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let (script, script_dir) = load(&dir)?;
    let resolved = script.resolve(&script_dir)?;
    let report = run_tests(&resolved, all)?;
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
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--prefix" => {
                i += 1;
                prefix = Some(PathBuf::from(args.get(i).ok_or("--prefix 需要参数")?));
            }
            "-h" | "--help" => {
                println!("用法: ns-build install [--prefix <路径>]");
                return Ok(());
            }
            other => return Err(format!("未知选项 '{other}'")),
        }
        i += 1;
    }
    let dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let (script, script_dir) = load(&dir)?;
    let resolved = script.resolve(&script_dir)?;
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
    let dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let (script, script_dir) = load(&dir)?;
    let resolved = script.resolve(&script_dir)?;
    let Some(path) = find_exe(&resolved, &target) else {
        return Err(format!("找不到可执行目标 '{target}'"));
    };
    if !path.exists() {
        return Err(format!("目标 '{target}' 未构建，请先运行 ns-build build"));
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
    let resolved = script.resolve(&script_dir)?;
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
    let resolved = script.resolve(&script_dir)?;
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

[[targets]]
name = "{name}"
kind = "exe"
sources = ["src/**/*.c"]
link = []
"#
    );
    std::fs::create_dir_all(dir.join("src")).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("src/main.c"), "#include <stdio.h>\nint main(void) {\n    printf(\"hello from {name}\\n\");\n    return 0;\n}\n")
        .map_err(|e| e.to_string())?;
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
    }
}
