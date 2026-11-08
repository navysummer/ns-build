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
  ns-build presets
  ns-build fetch [--update]
  ns-build init [--name <名>]

选项:
  -j, --jobs <N>      并行任务数 (默认: CPU 核数; 支持 -j4/-j8/-j100)
  --release           优化构建 (默认)
  --debug             调试构建
  --profile <配置>    release | debug | relwithdebinfo | minsizerel
  --preset <名字>     应用 [presets] 中的命名预设（命令行显式选项优先）
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
        "presets" => cmd_presets(),
        "fetch" => cmd_fetch(&args[1..]),
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
    /// `--preset <name>`：应用 `[presets]` 中的命名预设。
    preset: Option<String>,
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
        preset: None,
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
            "--define" => {
                i += 1;
                let v = args.get(i).ok_or("--define 需要 name=value")?;
                if let Some((name, value)) = v.split_once('=') {
                    out.defines.insert(name.to_string(), value.to_string());
                } else {
                    out.defines.insert(v.to_string(), "1".to_string());
                }
            }
            t if t.starts_with("--define=") => {
                let v = &t["--define=".len()..];
                if let Some((name, value)) = v.split_once('=') {
                    out.defines.insert(name.to_string(), value.to_string());
                } else if !v.is_empty() {
                    out.defines.insert(v.to_string(), "1".to_string());
                } else {
                    return Err("--define 需要 name=value".into());
                }
            }
            "--preset" => {
                i += 1;
                let v = args.get(i).ok_or("--preset 需要预设名")?;
                out.preset = Some(v.clone());
            }
            t if t.starts_with("--preset=") => {
                out.preset = Some(t["--preset=".len()..].to_string());
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
    // 展开生成源引用的自定义命令（及其自身的依赖）：
    // 目标过滤时不能把生成步骤一起裁掉。
    if !script.custom_commands.is_empty() {
        let mut needed: BTreeSet<String> = BTreeSet::new();
        for name in &allowed {
            if let Some(t) = by_name.get(name.as_str()) {
                for src in &t.sources {
                    for cc in &script.custom_commands {
                        if cc.outputs.iter().any(|o| o == src) {
                            needed.insert(cc.name.clone());
                        }
                    }
                }
            }
        }
        // fixpoint：自定义命令自身的 deps 也必须保留。
        let by_custom: BTreeMap<&str, &ns_build::script::ResolvedCustomCommand> = script
            .custom_commands
            .iter()
            .map(|cc| (cc.name.as_str(), cc))
            .collect();
        let mut changed = true;
        while changed {
            changed = false;
            for cc in &script.custom_commands {
                if needed.contains(&cc.name) {
                    for d in &cc.deps {
                        if by_custom.contains_key(d.as_str()) && needed.insert(d.clone()) {
                            changed = true;
                        }
                    }
                }
            }
        }
        allowed.extend(needed);
    }
    Ok(allowed.into_iter().collect())
}



fn cmd_build(args: &[String]) -> Result<(), String> {
    let ba = parse_build_args(args)?;
    let dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let (script, script_dir) = load(&dir)?;
    let mut ba = ba;
    let mut script = script;
    // 应用 --preset：预设中的值优先级低于命令行显式选项。
    if let Some(name) = &ba.preset {
        let p = script
            .presets
            .get(name)
            .cloned()
            .ok_or_else(|| {
                let avail = if script.presets.is_empty() {
                    "无".to_string()
                } else {
                    script.presets.keys().cloned().collect::<Vec<_>>().join(", ")
                };
                format!("未知预设 '{name}'（可选：{avail}）")
            })?;
        if let Some(t) = &p.toolchain {
            script.defaults.toolchain = Some(t.clone());
        }
        if let Some(u) = &p.unity {
            script.defaults.unity = u.clone();
        }
        if let Some(w) = p.warnings {
            script.defaults.warnings = w;
        }
        if ba.profile.is_none() {
            ba.profile = p.profile;
        }
        if ba.jobs.is_none() {
            ba.jobs = p.jobs;
        }
        for (k, v) in p.defines {
            ba.defines.entry(k).or_insert(v);
        }
    }
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
            "--preset" => {
                i += 1;
                profile_args.push("--preset".into());
                profile_args.push(args.get(i).ok_or("--preset 需要预设名")?.clone());
            }
            v if v.starts_with("--preset=") => {
                profile_args.push("--preset".into());
                profile_args.push(v["--preset=".len()..].to_string());
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

fn profile_name(p: &ns_build::script::Profile) -> &'static str {
    use ns_build::script::Profile::*;
    match p {
        Release => "release",
        Debug => "debug",
        RelWithDebInfo => "relwithdebinfo",
        MinSizeRel => "minsizerel",
    }
}

fn cmd_presets() -> Result<(), String> {
    let dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let (script, _) = load(&dir)?;
    if script.presets.is_empty() {
        println!("[ns-build] 未定义预设（在 ns-build.toml 的 [presets] 节中定义）");
        return Ok(());
    }
    println!("[ns-build] 可用预设：");
    for (name, p) in &script.presets {
        println!("  {name}");
        if let Some(profile) = &p.profile {
            println!("    profile = {}", profile_name(profile));
        }
        if let Some(j) = p.jobs {
            println!("    jobs = {j}");
        }
        if let Some(u) = &p.unity {
            println!("    unity = {u}");
        }
        if let Some(t) = &p.toolchain {
            println!("    toolchain = {t}");
        }
        if let Some(w) = p.warnings {
            println!("    warnings = {w}");
        }
        if !p.defines.is_empty() {
            let ds = p
                .defines
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(", ");
            println!("    defines = {ds}");
        }
    }
    Ok(())
}

/// 拉取 `[[fetch]]` 外部依赖到 `.ns-fetch/<name>/`。
/// 默认已存在则跳过；`--update` 强制重新拉取。
fn cmd_fetch(args: &[String]) -> Result<(), String> {
    let mut update = false;
    for a in args {
        match a.as_str() {
            "--update" | "-u" => update = true,
            v => return Err(format!("未知选项 '{v}'（ns-build fetch [--update]）")),
        }
    }
    let dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let (script, _) = load(&dir)?;
    if script.fetch.is_empty() {
        println!("[ns-build] 未定义 [[fetch]]（ns-build.toml）");
        return Ok(());
    }
    let fetch_root = dir.join(".ns-fetch");
    std::fs::create_dir_all(&fetch_root)
        .map_err(|e| format!("创建 {} 失败: {e}", fetch_root.display()))?;
    for f in &script.fetch {
        fetch_one(f, &fetch_root, update)?;
    }
    println!("[ns-build] fetch 完成");
    Ok(())
}

fn fetch_one(f: &ns_build::script::Fetch, fetch_root: &std::path::Path, update: bool) -> Result<(), String> {
    let name = f.name.trim();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(format!(
            "fetch：name '{name}' 非法（仅字母数字 / '-' / '_' / '.'）"
        ));
    }
    let dest = fetch_root.join(name);
    if dest.exists() {
        if !update {
            println!("[ns-build] 跳过 {name}（已存在；--update 强制更新）");
            return Ok(());
        }
        if dest.is_dir() {
            std::fs::remove_dir_all(&dest)
                .map_err(|e| format!("删除 {} 失败: {e}", dest.display()))?;
        } else {
            std::fs::remove_file(&dest)
                .map_err(|e| format!("删除 {} 失败: {e}", dest.display()))?;
        }
    }
    match (&f.git, &f.url) {
        (Some(_), Some(_)) => Err(format!("fetch '{name}'：git 与 url 只能二选一")),
        (None, None) => Err(format!("fetch '{name}'：需要 git 或 url")),
        (Some(git), None) => fetch_git(name, git, f.rev.as_deref(), &dest),
        (None, Some(url)) => fetch_url(name, url, f.strip_components, &dest),
    }
}

fn fetch_git(name: &str, git: &str, rev: Option<&str>, dest: &std::path::Path) -> Result<(), String> {
    let run = |args: &[&str]| -> Result<bool, String> {
        let out = std::process::Command::new("git")
            .args(args)
            .output()
            .map_err(|e| format!("运行 git 失败: {e}"))?;
        Ok(out.status.success())
    };
    let dest_s = dest.to_string_lossy().into_owned();
    println!("[ns-build] 拉取 git {git} -> {}", dest.display());
    let ok = match rev {
        // 先尝试浅克隆指定分支/标签；失败则完整克隆后 checkout。
        Some(r) => {
            if run(&["clone", "--depth", "1", "--branch", r, git, &dest_s])? {
                true
            } else {
                if dest.exists() {
                    let _ = std::fs::remove_dir_all(dest);
                }
                run(&["clone", git, &dest_s])? && run(&["-C", &dest_s, "checkout", r])?
            }
        }
        None => run(&["clone", "--depth", "1", git, &dest_s])?,
    };
    if !ok {
        return Err(format!("fetch '{name}'：git 拉取失败（{git}）"));
    }
    let manifest = format!("source = \"git\"\nurl = \"{git}\"\nrev = {rev:?}\n");
    std::fs::write(dest.join(".ns-fetch.toml"), manifest)
        .map_err(|e| format!("fetch '{name}'：写入清单失败: {e}"))?;
    println!("[ns-build] 已拉取 {name}");
    Ok(())
}

fn fetch_url(name: &str, url: &str, strip_components: usize, dest: &std::path::Path) -> Result<(), String> {
    println!("[ns-build] 下载 {url} -> {}", dest.display());
    std::fs::create_dir_all(dest).map_err(|e| format!("创建 {} 失败: {e}", dest.display()))?;
    // 下载到临时文件（系统 temp 目录下，按进程隔离）。
    let tmp = std::env::temp_dir().join(format!(
        "ns-fetch-{}-{}",
        std::process::id(),
        url.rsplit('/').next().unwrap_or("archive")
    ));
    let st = std::process::Command::new("curl")
        .args(["-sfL", url, "-o"])
        .arg(&tmp)
        .status()
        .map_err(|e| format!("运行 curl 失败: {e}"))?;
    if !st.success() {
        return Err(format!("fetch '{name}'：下载失败（{url}）"));
    }
    let lower = url.to_lowercase();
    let is_zip = lower.ends_with(".zip");
    let is_tgz = lower.ends_with(".tar.gz") || lower.ends_with(".tgz");
    let is_tar = lower.ends_with(".tar") || is_tgz;
    let extract_ok = if is_zip {
        if strip_components > 0 {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!(
                "fetch '{name}'：zip 不支持 strip_components（请用 tar 或预处理）"
            ));
        }
        std::process::Command::new("unzip")
            .arg("-q")
            .arg(&tmp)
            .arg("-d")
            .arg(dest)
            .status()
            .map_err(|e| format!("运行 unzip 失败: {e}"))?
            .success()
    } else if is_tar {
        let mut args: Vec<String> = vec![if is_tgz { "-zxf".into() } else { "-xf".into() }];
        args.push(tmp.to_string_lossy().into_owned());
        args.push("-C".into());
        args.push(dest.to_string_lossy().into_owned());
        if strip_components > 0 {
            args.push("--strip-components".into());
            args.push(strip_components.to_string());
        }
        std::process::Command::new("tar")
            .args(&args)
            .status()
            .map_err(|e| format!("运行 tar 失败: {e}"))?
            .success()
    } else {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "fetch '{name}'：无法识别的压缩包格式（{url}，支持 .tar.gz/.tgz/.tar/.zip）"
        ));
    };
    let _ = std::fs::remove_file(&tmp);
    if !extract_ok {
        return Err(format!("fetch '{name}'：解压失败（{url}）"));
    }
    let manifest = format!("source = \"url\"\nurl = \"{url}\"\nstrip_components = {strip_components}\n");
    std::fs::write(dest.join(".ns-fetch.toml"), manifest)
        .map_err(|e| format!("fetch '{name}'：写入清单失败: {e}"))?;
    println!("[ns-build] 已拉取 {name}");
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
