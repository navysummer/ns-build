//! 测试运行器：运行构建产物（可执行）并汇报退出码。

use std::path::Path;

use crate::script::{ResolvedScript, TargetKind};

/// 测试结果。
#[derive(Debug, Default, Clone)]
pub struct TestReport {
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
}

/// 运行所有测试。`labels`：只运行带有指定标签之一的显式 `[[tests]]`
///（空 = 全部显式测试；隐式"name 含 test 的 exe"目标仅在空时参与）。
pub fn run_tests(script: &ResolvedScript, all: bool, labels: &[String]) -> Result<TestReport, String> {
    let mut report = TestReport::default();
    // 显式注册的 `[[tests]]` 优先。
    for ts in &script.script.tests {
        if !labels.is_empty() && !ts.labels.iter().any(|l| labels.contains(l)) {
            // 不匹配标签：不计入统计。
            continue;
        }
        let path = script
            .targets
            .iter()
            .find(|t| t.name == ts.target)
            .map(|t| t.output.clone());
        let Some(path) = path else {
            report.failed += 1;
            eprintln!("  [FAIL] {} 目标 {} 不存在", ts.name, ts.target);
            continue;
        };
        if !path.exists() {
            report.skipped += 1;
            eprintln!("  [SKIP] {} 未构建", ts.name);
            continue;
        }
        let mut cmd = std::process::Command::new(&path);
        cmd.args(&ts.args);
        if let Some(wd) = &ts.workdir {
            cmd.current_dir(script.dir.join(wd));
        }
        let status = if let Some(tmo) = ts.timeout {
            run_with_timeout(cmd, tmo)
        } else {
            match cmd.status() {
                Ok(s) => Some(s),
                Err(e) => {
                    eprintln!("  [FAIL] {} 运行失败: {e}", ts.name);
                    None
                }
            }
        };
        let Some(status) = status else {
            report.failed += 1;
            continue;
        };
        if status.success() || !ts.require_success {
            report.passed += 1;
            println!("  [PASS] {}", ts.name);
        } else {
            report.failed += 1;
            eprintln!("  [FAIL] {} 退出码 {:?}", ts.name, status.code());
        }
    }
    for t in &script.targets {
        if !labels.is_empty() {
            break; // 指定标签时不跑隐式 exe 测试
        }
        if t.kind != TargetKind::Exe {
            continue;
        }
        let is_test = t.name.contains("test") || t.name.contains("Test");
        if !all && !is_test {
            continue;
        }
        let path = &t.output;
        if !path.exists() {
            report.skipped += 1;
            eprintln!("  [SKIP] {} 未构建", t.name);
            continue;
        }
        let status = std::process::Command::new(path).status();
        match status {
            Ok(s) if s.success() => {
                report.passed += 1;
                println!("  [PASS] {}", t.name);
            }
            Ok(s) => {
                report.failed += 1;
                eprintln!("  [FAIL] {} 退出码 {:?}", t.name, s.code());
            }
            Err(e) => {
                report.failed += 1;
                eprintln!("  [FAIL] {} 运行失败: {e}", t.name);
            }
        }
    }
    Ok(report)
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
pub fn find_exe<'a>(script: &'a ResolvedScript, name: &str) -> Option<&'a Path> {
    script
        .targets
        .iter()
        .find(|t| t.name == name && t.kind == TargetKind::Exe)
        .map(|t| t.output.as_path())
}

/// 运行单个目标。
pub fn run_one(path: &Path, args: &[String]) -> Result<std::process::ExitStatus, String> {
    std::process::Command::new(path)
        .args(args)
        .status()
        .map_err(|e| format!("运行 {} 失败: {e}", path.display()))
}
