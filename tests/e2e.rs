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
fn init_scaffolds_builds_and_runs() {
    let d = tmpdir("init");
    let (_, err, code) = run_ns_build(&["init", "--name", "initdemo"], &d);
    assert_eq!(code, 0, "init 失败: {err}");
    assert!(d.join("ns-build.toml").exists());
    let main_c = std::fs::read_to_string(d.join("src/main.c")).unwrap();
    assert!(
        main_c.contains("initdemo"),
        "main.c 应包含项目名（模板替换）: {main_c}"
    );
    let (_, err, code) = run_ns_build(&["build"], &d);
    assert_eq!(code, 0, "init 生成的项目构建失败: {err}");
    let out = Command::new(d.join("build/initdemo")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "hello from initdemo\n");
    // 已有脚本时 init 必须拒绝覆盖。
    let (_, _, code) = run_ns_build(&["init", "--name", "x"], &d);
    assert_ne!(code, 0);
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
    let (_out, err, code) = run_ns_build(&["build"], &d);
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

#[test]
fn static_library_dependency_filter_and_install() {
    let d = tmpdir("static-dep");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "libdemo"

[[targets]]
name = "util"
kind = "static"
sources = ["src/util.c"]
include_dirs = ["include"]
defines = ["UTIL_BONUS=5"]
link = ["m"]

[[targets]]
name = "app"
kind = "exe"
sources = ["src/main.c"]
deps = ["util"]

[[targets]]
name = "other"
kind = "exe"
sources = ["src/other.c"]
"#,
    );
    write(&d.join("include/util.h"), "#define UTIL_BASE 7\n");
    write(
        &d.join("src/util.c"),
        "#include \"util.h\"\ndouble sq(double x){return x*x + UTIL_BASE + UTIL_BONUS;}\n",
    );
    write(
        &d.join("src/main.c"),
        "#include <stdio.h>\n#include \"util.h\"\ndouble sq(double);\nint main(void){printf(\"%.0f\\n\", sq(2.0));return 0;}\n",
    );
    write(&d.join("src/other.c"), "int main(void){return 0;}\n");

    // 只构建 app：必须自动带上 util，且不构建无关目标。
    let (out, err, code) = run_ns_build(&["build", "app"], &d);
    assert_eq!(code, 0, "build app 失败: {err}");
    assert!(d.join("build/app").exists(), "app 未生成");
    assert!(
        d.join("build/libutil.a").exists(),
        "静态库未生成: {} {out}",
        err
    );
    assert!(!d.join("build/other").exists(), "未请求的 other 不应构建");
    let run = Command::new(d.join("build/app")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), "16\n");

    // 增量：未变化应全部跳过。
    let (out2, err2, code2) = run_ns_build(&["build", "app"], &d);
    assert_eq!(code2, 0, "二次构建失败: {err2}");
    assert!(out2.contains("0 构建"), "增量应跳过: {out2} {err2}");

    // 安装：可执行到 bin，静态库到 lib。
    let prefix = d.join("prefix");
    let (_, err3, code3) = run_ns_build(
        &["install", "--prefix", prefix.to_str().unwrap()],
        &d,
    );
    assert_eq!(code3, 0, "install 失败: {err3}");
    assert!(prefix.join("bin/app").exists());
    assert!(prefix.join("lib/libutil.a").exists());
}

#[test]
fn header_change_triggers_recompile() {
    let d = tmpdir("header-inc");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "hdrdemo"

[[targets]]
name = "hdrdemo"
kind = "exe"
sources = ["src/main.c"]
"#,
    );
    write(&d.join("src/value.h"), "#define VALUE 1\n");
    write(
        &d.join("src/main.c"),
        "#include <stdio.h>\n#include \"value.h\"\nint main(void){printf(\"%d\\n\", VALUE);return 0;}\n",
    );
    let (_, err, code) = run_ns_build(&["build"], &d);
    assert_eq!(code, 0, "首次构建失败: {err}");
    let out1 = Command::new(d.join("build/hdrdemo")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out1.stdout), "1\n");

    // 只改头文件 → 依赖它的源文件必须重编译。
    write(&d.join("src/value.h"), "#define VALUE 2\n");
    let (out2, err2, code2) = run_ns_build(&["build"], &d);
    assert_eq!(code2, 0, "头文件变更后构建失败: {err2}");
    assert!(out2.contains("2 构建"), "改头文件应重编译+重链接: {out2} {err2}");
    let out3 = Command::new(d.join("build/hdrdemo")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out3.stdout), "2\n");
}

#[test]
fn interface_phony_and_hooks() {
    let d = tmpdir("interface");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "iface"

[[targets]]
name = "hdrs"
kind = "interface"
include_dirs = ["includes"]
defines = ["MAGIC=5"]

[[targets]]
name = "all"
kind = "phony"
deps = ["app"]

[[targets]]
name = "app"
kind = "exe"
sources = ["main.c"]
deps = ["hdrs"]
hooks.before = ["sh -c 'echo before > before.txt'"]
hooks.after = [{ program = "sh", args = ["-c", "echo after > after.txt"] }]
"#,
    );
    write(&d.join("includes/config.h"), "#define VALUE 9\n");
    write(
        &d.join("main.c"),
        "#include <stdio.h>\n#include \"config.h\"\nint main(void){printf(\"%d\\n\", MAGIC + VALUE);return 0;}\n",
    );
    let (_, err, code) = run_ns_build(&["build", "all"], &d);
    assert_eq!(code, 0, "build all 失败: {err}");
    // interface 目标不应产出同名文件。
    assert!(!d.join("build/hdrs").exists(), "interface 不应链接产物");
    assert!(d.join("before.txt").exists(), "before 钩子未运行");
    assert!(d.join("after.txt").exists(), "after 钩子未运行");
    let out = Command::new(d.join("build/app")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "14\n");
}

#[test]
fn chained_static_libraries_link_transitively() {
    let d = tmpdir("static-chain");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "chain"

[[targets]]
name = "b"
kind = "static"
sources = ["src/b.c"]

[[targets]]
name = "a"
kind = "static"
sources = ["src/a.c"]
deps = ["b"]

[[targets]]
name = "app"
kind = "exe"
sources = ["src/main.c"]
deps = ["a"]
"#,
    );
    write(&d.join("src/b.c"), "int b(void){return 20;}\n");
    write(&d.join("src/a.c"), "int b(void);\nint a(void){return b() + 1;}\n");
    write(
        &d.join("src/main.c"),
        "#include <stdio.h>\nint a(void);\nint main(void){printf(\"%d\\n\", a());return 0;}\n",
    );
    let (_, err, code) = run_ns_build(&["build", "app"], &d);
    assert_eq!(code, 0, "静态库链式链接失败: {err}");
    let out = Command::new(d.join("build/app")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "21\n");
}

#[test]
fn shared_library_links_and_runs() {
    let d = tmpdir("shared");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "shared"

[[targets]]
name = "shared"
kind = "lib"
sources = ["src/lib.c"]

[[targets]]
name = "app"
kind = "exe"
sources = ["src/main.c"]
deps = ["shared"]
"#,
    );
    write(&d.join("src/lib.c"), "int answer(void){return 42;}\n");
    write(
        &d.join("src/main.c"),
        "#include <stdio.h>\nint answer(void);\nint main(void){printf(\"%d\\n\", answer());return 0;}\n",
    );
    let (_, err, code) = run_ns_build(&["build", "app"], &d);
    assert_eq!(code, 0, "动态库构建失败: {err}");
    let out = Command::new(d.join("build/app")).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "42\n",
        "动态库运行失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn script_profile_and_cli_override() {
    let d = tmpdir("profile");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "prof"

[defaults]
profile = "debug"

[[targets]]
name = "prof"
kind = "exe"
sources = ["src/main.c"]
"#,
    );
    write(&d.join("src/main.c"), "int main(void){return 0;}\n");
    // 脚本 profile = debug 应生效。
    let (_, err, code) = run_ns_build(&["build", "--verbose"], &d);
    assert_eq!(code, 0, "debug 构建失败: {err}");
    assert!(err.contains("-O0"), "应使用 debug 优化级别: {err}");
    assert!(err.contains("-g"), "应包含调试符号: {err}");
    // CLI --release 应覆盖脚本。
    let (_, err2, code2) = run_ns_build(&["build", "--release", "--verbose"], &d);
    assert_eq!(code2, 0, "release 构建失败: {err2}");
    assert!(err2.contains("-O2"), "CLI 应覆盖为 release: {err2}");
}
