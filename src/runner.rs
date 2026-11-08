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

/// 运行所有 kind = exe 且名含 `test`（或以 `--all` 运行时全部）的产物。
pub fn run_tests(script: &ResolvedScript, all: bool) -> Result<TestReport, String> {
    let mut report = TestReport::default();
    for t in &script.targets {
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
