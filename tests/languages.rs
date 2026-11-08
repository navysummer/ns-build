//! 多语言端到端：真实构建并运行 c++/rust/go/java/zig/python/shell 产物。

use std::path::{Path, PathBuf};
use std::process::Command;

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

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("nsbuild-lang-{}-{}", name, std::process::id()));
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
fn cpp_builds_and_runs() {
    if Command::new("g++").arg("--version").output().is_err() {
        eprintln!("跳过 c++: g++ 不可用");
        return;
    }
    let d = tmpdir("cpp");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "cpp"

[defaults]
cxx = "g++"

[[targets]]
name = "cpp"
kind = "exe"
sources = ["src/main.cpp"]
"#,
    );
    write(
        &d.join("src/main.cpp"),
        r#"
#include <iostream>
int main() {
    std::cout << "cpp-ok" << std::endl;
    return 0;
}
"#,
    );
    let (_, err, code) = run_ns_build(&["build"], &d);
    assert_eq!(code, 0, "c++ 构建失败: {err}");
    let exe = d.join("build/cpp");
    assert!(exe.exists(), "c++ 产物不存在");
    let out = Command::new(&exe).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "cpp-ok\n");
}

#[test]
fn cpp_unity_builds_and_runs() {
    if Command::new("c++").arg("--version").output().is_err() {
        eprintln!("跳过 c++ unity: c++ 不可用");
        return;
    }
    let d = tmpdir("cpp-unity");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "cppu"

[defaults]
cxx = "c++"

[[targets]]
name = "cppu"
kind = "exe"
sources = ["src/*.cpp"]
groups = 2
"#,
    );
    for i in 0..4 {
        write(
            &d.join(format!("src/f{i}.cpp")),
            &format!("int f{i}() {{ return {i}; }}\n"),
        );
    }
    write(
        &d.join("src/main.cpp"),
        r#"
#include <iostream>
int f0(); int f1(); int f2(); int f3();
int main() {
    std::cout << (f0() + f1() + f2() + f3()) << std::endl;
    return 0;
}
"#,
    );
    let (_, err, code) = run_ns_build(&["build"], &d);
    assert_eq!(code, 0, "c++ unity 构建失败: {err}");
    let out = Command::new(d.join("build/cppu")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "6\n");
}

#[test]
fn rust_builds_and_runs() {
    if Command::new("rustc").arg("--version").output().is_err() {
        eprintln!("跳过 rust: rustc 不可用");
        return;
    }
    let d = tmpdir("rust");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "rs"

[[targets]]
name = "rs"
kind = "exe"
language = "rust"
sources = ["src/main.rs"]
"#,
    );
    write(
        &d.join("src/main.rs"),
        r#"
fn main() {
    println!("rust-ok");
}
"#,
    );
    let (_, err, code) = run_ns_build(&["build"], &d);
    assert_eq!(code, 0, "rust 构建失败: {err}");
    let exe = d.join("build/rs");
    assert!(exe.exists(), "rust 产物不存在");
    let out = Command::new(&exe).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "rust-ok\n");
}

#[test]
fn go_builds_and_runs() {
    if Command::new("go").arg("version").output().is_err() {
        eprintln!("跳过 go: go 不可用");
        return;
    }
    let d = tmpdir("go");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "goapp"

[[targets]]
name = "goapp"
kind = "exe"
language = "go"
sources = ["src/main.go"]
"#,
    );
    write(
        &d.join("src/main.go"),
        r#"
package main
import "fmt"
func main() {
    fmt.Println("go-ok")
}
"#,
    );
    let (_, err, code) = run_ns_build(&["build"], &d);
    assert_eq!(code, 0, "go 构建失败: {err}");
    let exe = d.join("build/goapp");
    assert!(exe.exists(), "go 产物不存在");
    let out = Command::new(&exe).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "go-ok\n");
}

#[test]
fn java_builds_and_runs() {
    if Command::new("javac").arg("-version").output().is_err() {
        eprintln!("跳过 java: javac 不可用");
        return;
    }
    let d = tmpdir("java");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "japp"

[[targets]]
name = "japp"
kind = "exe"
language = "java"
sources = ["src/Main.java"]
"#,
    );
    write(
        &d.join("src/Main.java"),
        r#"
public class Main {
    public static void main(String[] args) {
        System.out.println("java-ok");
    }
}
"#,
    );
    let (_, err, code) = run_ns_build(&["build"], &d);
    assert_eq!(code, 0, "java 构建失败: {err}");
    // 运行编译出的类。
    let classes = d.join("build/classes");
    assert!(classes.join("Main.class").exists(), "java 类不存在");
    let out = Command::new("java")
        .arg("-cp")
        .arg(&classes)
        .arg("Main")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "java-ok\n");
}

#[test]
fn zig_builds_and_runs() {
    if Command::new("zig").arg("version").output().is_err() {
        eprintln!("跳过 zig: zig 不可用");
        return;
    }
    let d = tmpdir("zig");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "zapp"

[[targets]]
name = "zapp"
kind = "exe"
language = "zig"
sources = ["src/main.zig"]
"#,
    );
    write(
        &d.join("src/main.zig"),
        r#"
const std = @import("std");
pub fn main() !void {
    std.debug.print("zig-ok\n", .{});
}
"#,
    );
    let (_, err, code) = run_ns_build(&["build"], &d);
    assert_eq!(code, 0, "zig 构建失败: {err}");
    let exe = d.join("build/zapp");
    assert!(exe.exists(), "zig 产物不存在");
    let out = Command::new(&exe).output().unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(combined.contains("zig-ok"), "zig 输出: {combined}");
}

#[test]
fn python_packages_and_runs() {
    let d = tmpdir("python");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "papp"

[[targets]]
name = "papp"
kind = "exe"
language = "python"
sources = ["src/main.py"]
"#,
    );
    write(
        &d.join("src/main.py"),
        "#!/usr/bin/env python3\nprint(\"python-ok\")\n",
    );
    let (_, err, code) = run_ns_build(&["build"], &d);
    assert_eq!(code, 0, "python 构建失败: {err}");
    let exe = d.join("build/papp");
    assert!(exe.exists(), "python 产物不存在");
    let out = Command::new(&exe).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "python-ok\n");
}

#[test]
fn shell_packages_and_runs() {
    let d = tmpdir("shell");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "sapp"

[[targets]]
name = "sapp"
kind = "exe"
language = "shell"
sources = ["src/main.sh"]
"#,
    );
    write(
        &d.join("src/main.sh"),
        "#!/bin/sh\necho shell-ok\n",
    );
    let (_, err, code) = run_ns_build(&["build"], &d);
    assert_eq!(code, 0, "shell 构建失败: {err}");
    let exe = d.join("build/sapp");
    assert!(exe.exists(), "shell 产物不存在");
    let out = Command::new(&exe).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "shell-ok\n");
}
