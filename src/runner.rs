//! 测试运行器：运行构建产物（可执行）并汇报退出码。

use std::path::{Path, PathBuf};

use crate::script::{ResolvedScript, TargetKind};

/// 测试结果。
#[derive(Debug, Default, Clone)]
pub struct TestReport {
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
}

/// 一个待运行的测试用例。
struct TestCase {
    name: String,
    path: std::path::PathBuf,
    args: Vec<String>,
    workdir: Option<std::path::PathBuf>,
    timeout: Option<u64>,
    require_success: bool,
    /// Java 目标：classes 目录 + main class（此时 path 指向该目录）。
    java_class_dir: Option<std::path::PathBuf>,
    java_main_class: Option<String>,
}

/// 收集阶段产物：要么直接汇报，要么需要运行。
enum PlannedTest {
    Immediate(TestOutcome),
    Run(TestCase),
}

enum TestOutcome {
    Passed(String),
    Failed(String, String),
    Skipped(String),
}

/// 运行所有测试。
/// - `labels`：只运行带有指定标签之一的显式 `[[tests]]`（空 = 全部显式测试）。
///   隐式"name 含 test 的 exe"目标仅在空时参与。
/// - `parallel`：并行运行的测试数（>1 时启用线程池，输出仍按声明顺序打印）。
pub fn run_tests(script: &ResolvedScript, all: bool, labels: &[String], parallel: usize) -> Result<TestReport, String> {
    let mut report = TestReport::default();

    // 收集（显式优先，声明顺序）。
    let mut planned: Vec<PlannedTest> = Vec::new();
    for ts in &script.script.tests {
        if !labels.is_empty() && !ts.labels.iter().any(|l| labels.contains(l)) {
            continue;
        }
        // 测试目标名经 alias 解析。
        let unaliased = crate::script::unalias(&ts.target, &script.targets);
        let path = script
            .targets
            .iter()
            .find(|t| t.name == unaliased)
            .map(|t| t.output.clone());
        let Some(path) = path else {
            planned.push(PlannedTest::Immediate(TestOutcome::Failed(
                ts.name.clone(),
                format!("目标 {} 不存在", ts.target),
            )));
            continue;
        };
        if !path.exists() {
            planned.push(PlannedTest::Immediate(TestOutcome::Skipped(ts.name.clone())));
            continue;
        }
        // Java 目标：从 sources 推断 main class。
        let ud = crate::script::unalias(&ts.target, &script.targets);
        let java_info = script
            .targets
            .iter()
            .find(|t| t.name == ud && t.language == crate::script::Language::Java)
            .and_then(|t| {
                detect_java_main_class(&t.sources)
                    .map(|m| (t.output.clone(), m))
            });
        let (java_class_dir, java_main_class) = match java_info {
            Some((d, m)) => (Some(d), Some(m)),
            None => (None, None),
        };
        planned.push(PlannedTest::Run(TestCase {
            name: ts.name.clone(),
            path,
            args: ts.args.clone(),
            workdir: ts.workdir.as_ref().map(|w| script.dir.join(w)),
            timeout: ts.timeout,
            require_success: ts.require_success,
            java_class_dir,
            java_main_class,
        }));
    }
    if labels.is_empty() {
        for t in &script.targets {
            if t.kind != TargetKind::Exe {
                continue;
            }
            let is_test = t.name.contains("test") || t.name.contains("Test");
            if !all && !is_test {
                continue;
            }
            let path = t.output.clone();
            if !path.exists() {
                planned.push(PlannedTest::Immediate(TestOutcome::Skipped(t.name.clone())));
                continue;
            }
            let java_info = if t.language == crate::script::Language::Java {
                detect_java_main_class(&t.sources).map(|m| (t.output.clone(), m))
            } else {
                None
            };
            let (java_class_dir, java_main_class) = match java_info {
                Some((d, m)) => (Some(d), Some(m)),
                None => (None, None),
            };
            planned.push(PlannedTest::Run(TestCase {
                name: t.name.clone(),
                path,
                args: vec![],
                workdir: None,
                timeout: None,
                require_success: true,
                java_class_dir,
                java_main_class,
            }));
        }
    }

    // 拆分立即汇报与待运行用例。
    let mut results: Vec<(usize, TestOutcome)> = Vec::with_capacity(planned.len());
    let mut run_items: std::collections::VecDeque<(usize, TestCase)> = std::collections::VecDeque::new();
    for (i, p) in planned.into_iter().enumerate() {
        match p {
            PlannedTest::Immediate(o) => results.push((i, o)),
            PlannedTest::Run(tc) => run_items.push_back((i, tc)),
        }
    }

    // 有界线程池并行执行待运行用例。
    let workers = parallel.max(1).min(run_items.len().max(1));
    if workers <= 1 {
        while let Some((i, case)) = run_items.pop_front() {
            results.push((i, run_case(case)));
        }
    } else {
        let queue = std::sync::Mutex::new(run_items);
        let collected = std::sync::Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| loop {
                    let item = queue.lock().unwrap().pop_front();
                    let Some((idx, case)) = item else { break };
                    let outcome = run_case(case);
                    collected.lock().unwrap().push((idx, outcome));
                });
            }
        });
        results.extend(collected.into_inner().unwrap());
    }
    results.sort_by_key(|(i, _)| *i);

    for (_, outcome) in results {
        match outcome {
            TestOutcome::Passed(name) => {
                report.passed += 1;
                println!("  [PASS] {name}");
            }
            TestOutcome::Failed(name, detail) => {
                report.failed += 1;
                eprintln!("  [FAIL] {name}: {detail}");
            }
            TestOutcome::Skipped(name) => {
                report.skipped += 1;
                eprintln!("  [SKIP] {name} 未构建");
            }
        }
    }
    Ok(report)
}

/// 运行单个测试用例。
fn run_case(case: TestCase) -> TestOutcome {
    // Java 测试：用 java -cp 运行。
    if let Some(class_dir) = case.java_class_dir.as_deref() {
        let Some(main_class) = case.java_main_class.clone() else {
            return TestOutcome::Failed(case.name, "Java 目标缺 main class".to_string());
        };
        let status = run_java(class_dir, &main_class, &case.args);
        return match status {
            Ok(s) if s.success() || !case.require_success => TestOutcome::Passed(case.name),
            Ok(s) => TestOutcome::Failed(case.name, format!("退出码 {:?}", s.code())),
            Err(e) => TestOutcome::Failed(case.name, format!("启动 java 失败: {e}")),
        };
    }
    let mut cmd = std::process::Command::new(&case.path);
    cmd.args(&case.args);
    if let Some(wd) = &case.workdir {
        cmd.current_dir(wd);
    }
    let status = if let Some(tmo) = case.timeout {
        run_with_timeout(cmd, tmo)
    } else {
        match cmd.status() {
            Ok(s) => Some(s),
            Err(e) => {
                return TestOutcome::Failed(case.name, format!("运行失败: {e}"));
            }
        }
    };
    let Some(status) = status else {
        return TestOutcome::Failed(case.name, "运行失败或超时".to_string());
    };
    if status.success() || !case.require_success {
        TestOutcome::Passed(case.name)
    } else {
        TestOutcome::Failed(case.name, format!("退出码 {:?}", status.code()))
    }
}

/// 运行命令并超时；超时返回 None（调用方计失败）。
fn run_with_timeout(mut cmd: std::process::Command, secs: u64) -> Option<std::process::ExitStatus> {
    use std::time::{Duration, Instant};
    let mut child = cmd.spawn().ok()?;
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        match child.try_wait().ok()? {
            Some(status) => return Some(status),
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// 查找可运行目标。
/// 推断 Java main 类名：
/// 1. 扫描 `.java` 源文件找 `public class X`；
/// 2. 找不到则用第一个 `.java` 文件名（去 `.java` 后缀）。
pub fn detect_java_main_class(sources: &[PathBuf]) -> Option<String> {
    for s in sources {
        let Ok(text) = std::fs::read_to_string(s) else {
            continue;
        };
        for line in text.lines() {
            let l = line.trim();
            if let Some(rest) = l.strip_prefix("public class ") {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    return Some(name);
                }
            }
        }
    }
    sources.iter().find_map(|s| {
        if s.extension().and_then(|e| e.to_str()) == Some("java") {
            s.file_stem().and_then(|n| n.to_str()).map(|n| n.to_string())
        } else {
            None
        }
    })
}

/// 运行 Java 目标：`java -cp <classes> <MainClass> [args...]`。
pub fn run_java(class_dir: &Path, main_class: &str, args: &[String]) -> std::io::Result<std::process::ExitStatus> {
    std::process::Command::new("java")
        .arg("-cp")
        .arg(class_dir)
        .arg(main_class)
        .args(args)
        .status()
}

/// 查找可运行目标（经 alias 解析到底层真实目标）。
pub fn find_exe<'a>(script: &'a ResolvedScript, name: &str) -> Option<&'a Path> {
    let unaliased = crate::script::unalias(name, &script.targets);
    script
        .targets
        .iter()
        .find(|t| t.name == unaliased && t.kind == TargetKind::Exe)
        .map(|t| t.output.as_path())
}

/// 运行单个目标。
pub fn run_one(path: &Path, args: &[String]) -> Result<std::process::ExitStatus, String> {
    std::process::Command::new(path)
        .args(args)
        .status()
        .map_err(|e| format!("运行 {} 失败: {e}", path.display()))
}
