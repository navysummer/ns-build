//! 端到端集成测试：真实构建 C 项目，验证产物、增量、并行、分步、unity、安装、运行。

use std::path::{Path, PathBuf};
use std::process::Command;

/// 运行 ns-build 二进制，返回 (stdout, stderr, 退出码)。
fn run_ns_build(args: &[&str], cwd: &Path) -> (String, String, i32) {
    let out = Command::new(env!("CARGO_BIN_EXE_ns-build"))
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("运行 ns-build 失败");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

/// 临时目录。
fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("nsbuild-e2e-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write(p: &Path, s: &str) {
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(p, s).unwrap();
}

#[test]
fn builds_hello_c_and_runs() {
    let d = tmpdir("hello");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "hello"
version = "1.0.0"

[[targets]]
name = "hello"
kind = "exe"
sources = ["src/main.c"]
"#,
    );
    write(
        &d.join("src/main.c"),
        r#"
#include <stdio.h>
int main(void) {
    printf("hello world\n");
    return 0;
}
"#,
    );
    let (out, err, code) = run_ns_build(&["build"], &d);
    assert_eq!(code, 0, "build 失败: {err}");
    let exe = d.join("build/hello");
    assert!(exe.exists(), "产物不存在: {}", exe.display());
    let out_run = Command::new(&exe).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out_run.stdout), "hello world\n");
    // 增量：再次构建应全部跳过。
    let (out2, err2, code2) = run_ns_build(&["build"], &d);
    assert_eq!(code2, 0, "二次 build 失败: {err2}");
    assert!(out2.contains("0 构建"), "增量构建应跳过全部: {out2} {err2}");
}

#[test]
fn builds_multi_target_with_deps() {
    let d = tmpdir("multi");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "multi"

[defaults]
cc = "cc"

# 分步：先编译核心为对象。
[[targets]]
name = "core"
kind = "object"
sources = ["src/a.c", "src/b.c"]

# 可执行：依赖 core 的产物 + 自己源码。
[[targets]]
name = "app"
kind = "exe"
sources = ["src/main.c"]
deps = ["core"]
link = []
"#,
    );
    write(&d.join("src/a.c"), "int add(int x, int y) { return x + y; }\n");
    write(&d.join("src/b.c"), "int mul(int x, int y) { return x * y; }\n");
    write(
        &d.join("src/main.c"),
        r#"
#include <stdio.h>
int add(int, int);
int mul(int, int);
int main(void) {
    printf("%d %d\n", add(2, 3), mul(2, 3));
    return 0;
}
"#,
    );
    let (_, err, code) = run_ns_build(&["build"], &d);
    assert_eq!(code, 0, "build 失败: {err}");
    let exe = d.join("build/app");
    assert!(exe.exists(), "app 产物不存在");
    let out = Command::new(&exe).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "5 6\n");
}

#[test]
fn unity_grouped_build() {
    let d = tmpdir("unity");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "unity"

[[targets]]
name = "u"
kind = "exe"
sources = ["src/*.c"]
groups = 2
"#,
    );
    for i in 0..4 {
        write(
            &d.join(format!("src/f{i}.c")),
            &format!("int f{i}(void) {{ return {i}; }}\n"),
        );
    }
    write(
        &d.join("src/main.c"),
        r#"
#include <stdio.h>
int f0(void); int f1(void); int f2(void); int f3(void);
int main(void) {
    printf("%d\n", f0() + f1() + f2() + f3());
    return 0;
}
"#,
    );
    let (_, err, code) = run_ns_build(&["build"], &d);
    assert_eq!(code, 0, "unity build 失败: {err}");
    let exe = d.join("build/u");
    assert!(exe.exists());
    let out = Command::new(&exe).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "6\n");
    // 应只有 2 个对象文件（2 组 unity）。
    let obj_count = std::fs::read_dir(d.join("build/obj")).unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("o"))
        .count();
    assert_eq!(obj_count, 2, "unity 应产生 2 个对象: {err}");
}

#[test]
fn incremental_recompiles_on_change() {
    let d = tmpdir("incremental");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "inc"

[[targets]]
name = "inc"
kind = "exe"
sources = ["src/main.c"]
"#,
    );
    write(
        &d.join("src/main.c"),
        "int main(void) { return 1; }\n",
    );
    let (out, err, code) = run_ns_build(&["build"], &d);
    assert_eq!(code, 0, "首次 build: {err}");
    // 首次应构建（统计在 stdout）。
    assert!(out.contains("2 构建") || out.contains("构建: 2"), "首次应构建 2 个: {out}");
    // 修改源码 → 应重编译。
    write(&d.join("src/main.c"), "int main(void) { return 0; }\n");
    let (out2, err2, code2) = run_ns_build(&["build"], &d);
    assert_eq!(code2, 0, "修改后 build: {err2}");
    assert!(out2.contains("2 构建") || out2.contains("构建: 2"), "修改后应重编译: {out2}");
    // 再次不变 → 跳过。
    let (out3, err3, code3) = run_ns_build(&["build"], &d);
    assert_eq!(code3, 0);
    assert!(out3.contains("0 构建"), "不变应跳过: {out3} {err3}");
}

#[test]
fn custom_toolchain_and_flags() {
    let d = tmpdir("custom");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "custom"

[defaults]
cc = "cc"
cflags = ["-DVALUE=42"]
defines = ["EXTRA=7"]

[[targets]]
name = "custom"
kind = "exe"
sources = ["src/main.c"]
defines = ["LOCAL=1"]
"#,
    );
    write(
        &d.join("src/main.c"),
        r#"
#include <stdio.h>
#ifndef VALUE
#error "VALUE missing"
#endif
int main(void) {
    printf("%d\n", VALUE + LOCAL);
    return 0;
}
"#,
    );
    let (_, err, code) = run_ns_build(&["build"], &d);
    assert_eq!(code, 0, "custom 构建失败: {err}");
    let exe = d.join("build/custom");
    let out = Command::new(&exe).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "43\n");
}

#[test]
fn install_and_clean() {
    let d = tmpdir("install");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "inst"

[[targets]]
name = "inst"
kind = "exe"
sources = ["src/main.c"]
"#,
    );
    write(&d.join("src/main.c"), "int main(void) { return 0; }\n");
    let (_, err, code) = run_ns_build(&["build"], &d);
    assert_eq!(code, 0, "build: {err}");
    let prefix = d.join("prefix");
    let (_, err3, code3) = run_ns_build(
        &["install", "--prefix", prefix.to_str().unwrap()],
        &d,
    );
    assert_eq!(code3, 0, "install 失败: {err3}");
    assert!(prefix.join("bin/inst").exists(), "安装产物不存在");
    // clean。
    let (_, err4, code4) = run_ns_build(&["clean"], &d);
    assert_eq!(code4, 0, "clean: {err4}");
    assert!(!d.join("build").exists(), "build 目录应被删除");
}

#[test]
fn run_target_with_args() {
    let d = tmpdir("run");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "r"

[[targets]]
name = "r"
kind = "exe"
sources = ["src/main.c"]
"#,
    );
    write(
        &d.join("src/main.c"),
        r#"
#include <stdio.h>
int main(int argc, char **argv) {
    printf("argc=%d\n", argc);
    return 0;
}
"#,
    );
    let (_, err, code) = run_ns_build(&["build"], &d);
    assert_eq!(code, 0, "build: {err}");
    let (out, err2, code2) = run_ns_build(&["run", "r", "--", "a", "b"], &d);
    assert_eq!(code2, 0, "run 失败: {err2}");
    assert!(out.contains("argc=3"), "run 输出: {out}");
}

#[test]
fn show_lists_targets() {
    let d = tmpdir("show");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "s"

[[targets]]
name = "s"
kind = "exe"
sources = ["src/main.c"]
"#,
    );
    write(&d.join("src/main.c"), "int main(void){return 0;}\n");
    let (out, _, code) = run_ns_build(&["show"], &d);
    assert_eq!(code, 0);
    assert!(out.contains("s"), "show 输出: {out}");
    assert!(out.contains("exe"), "show 应含 kind: {out}");
}
