//! 性能测试：300 文件全量构建 vs 增量构建，验证增量显著快于全量。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("nsbuild-perf-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run(args: &[&str], cwd: &Path) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_ns-build"))
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn perf_300_files_full_vs_incremental() {
    let d = tmpdir("300");
    std::fs::write(
        d.join("ns-build.toml"),
        r#"
[project]
name = "perf"

[defaults]
cc = "cc"

[[targets]]
name = "perf"
kind = "exe"
sources = ["src/*.c"]
"#,
    )
    .unwrap();
    std::fs::create_dir_all(d.join("src")).unwrap();
    // 生成 300 个源文件（main.c 之外 299 个函数 + main）。
    for i in 0..300 {
        let body: String = if i == 0 {
            "#include <stdio.h>\nint f1(void);int f2(void);int f3(void);\nint main(void){printf(\"%d\\n\",f1()+f2()+f3());return 0;}\n".to_string()
        } else {
            format!("int f{i}(void) {{ return {i}; }}\n")
        };
        std::fs::write(d.join(format!("src/f{i}.c")), body).unwrap();
    }

    // 全量构建（计时）。
    let t0 = Instant::now();
    let (_, err, code) = run(&["build", "-j", "8"], &d);
    let full_time = t0.elapsed().as_secs_f64();
    assert_eq!(code, 0, "全量构建失败: {err}");
    assert!(
        full_time < 60.0,
        "300 文件全量构建应 <60s，实际 {full_time:.2}s"
    );

    // 增量构建（计时）。
    let t1 = Instant::now();
    let (out2, err2, code2) = run(&["build", "-j", "8"], &d);
    let inc_time = t1.elapsed().as_secs_f64();
    assert_eq!(code2, 0, "增量构建失败: {err2}");
    assert!(
        out2.contains("0 构建"),
        "增量构建应全部跳过: {out2} {err2}"
    );
    // 增量应显著快于全量（允许宽松：增量 < 全量）。
    assert!(
        inc_time < full_time,
        "增量 ({inc_time:.3}s) 应快于全量 ({full_time:.3}s)"
    );

    // 修改一个文件 → 只重建相关任务。
    std::fs::write(d.join("src/f5.c"), "int f5(void) { return 500; }\n").unwrap();
    let (out3, err3, code3) = run(&["build", "-j", "8"], &d);
    assert_eq!(code3, 0, "修改后构建失败: {err3}");
    assert!(
        out3.contains("2 构建"),
        "修改 1 文件应重建其 unity 组 + 链接 (2 构建): {out3}"
    );

    eprintln!("[perf] 全量 {full_time:.2}s, 增量 {inc_time:.3}s");
}
