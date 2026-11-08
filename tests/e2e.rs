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
fn custom_command_generates_source_and_rebuilds_on_change() {
    let d = tmpdir("custom-gen");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "gendemo"

[[custom_commands]]
name = "version"
outputs = ["build/gen/version.c"]
command = { program = "sh", args = ["-c", "printf 'int version(void){return 42;}\n' > build/gen/version.c"] }
depends = ["template.txt"]

[[targets]]
name = "gendemo"
kind = "exe"
sources = ["src/main.c", "build/gen/version.c"]
"#,
    );
    write(&d.join("template.txt"), "v1\n");
    write(
        &d.join("src/main.c"),
        r#"
#include <stdio.h>
int version(void);
int main(void) { printf("%d\n", version()); return 0; }
"#,
    );

    let (_, err, code) = run_ns_build(&["build"], &d);
    assert_eq!(code, 0, "首次构建失败: {err}");
    assert!(d.join("build/gen/version.c").exists(), "生成源未产生");
    let out = Command::new(d.join("build/gendemo")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "42\n");

    // 无变化：全部跳过。
    let (out2, err2, code2) = run_ns_build(&["build"], &d);
    assert_eq!(code2, 0, "二次构建失败: {err2}");
    assert!(out2.contains("0 构建"), "增量应全部跳过: {out2} {err2}");

    // 改 depends → 生成命令 + 生成源编译 + 链接重建（main.c 不动）。
    write(&d.join("template.txt"), "v2\n");
    let (out3, err3, code3) = run_ns_build(&["build", "--verbose"], &d);
    assert_eq!(code3, 0, "depends 变更后构建失败: {err3}");
    assert!(out3.contains("3 构建"), "应重建 gen+cc(version)+link: {out3} {err3}");
    assert!(d.join("build/gen/version.c").exists());

    // 删除生成产物 → 必须重新生成。
    std::fs::remove_file(d.join("build/gen/version.c")).unwrap();
    let (_out4, err4, code4) = run_ns_build(&["build"], &d);
    assert_eq!(code4, 0, "产物缺失后构建失败: {err4}");
    assert!(d.join("build/gen/version.c").exists(), "产物缺失未重新生成");
}

#[test]
fn custom_command_chained_by_deps() {
    let d = tmpdir("custom-chain");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "chaingen"

[[custom_commands]]
name = "stage1"
outputs = ["build/gen/stage1.txt"]
command = { program = "sh", args = ["-c", "printf 'stage1-content\n' > build/gen/stage1.txt"] }

[[custom_commands]]
name = "stage2"
outputs = ["build/gen/version.c"]
command = { program = "sh", args = ["-c", "printf 'int version(void){return 7;}\n' > build/gen/version.c"] }
deps = ["stage1"]
depends = ["build/gen/stage1.txt"]

[[targets]]
name = "chaingen"
kind = "exe"
sources = ["src/main.c", "build/gen/version.c"]
"#,
    );
    write(
        &d.join("src/main.c"),
        r#"
#include <stdio.h>
int version(void);
int main(void) { printf("%d\n", version()); return 0; }
"#,
    );
    let (_, err, code) = run_ns_build(&["build"], &d);
    assert_eq!(code, 0, "链式生成构建失败: {err}");
    assert!(d.join("build/gen/stage1.txt").exists());
    let out = Command::new(d.join("build/chaingen")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "7\n");
}

#[test]
fn preset_applies_compile_flags_and_fails_unknown() {
    let d = tmpdir("preset");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "predemo"

[presets]
fast = { profile = "debug", defines = { DIST_VALUE = "11" }, unity = "off" }
dist = { profile = "release", defines = { DIST_VALUE = "7" } }

[[targets]]
name = "predemo"
kind = "exe"
sources = ["src/main.c"]
defines = ["FROM_PRESET=${DIST_VALUE}"]
"#,
    );
    write(
        &d.join("src/main.c"),
        "#ifndef FROM_PRESET\n#error FROM_PRESET 未定义\n#endif\nint main(void){return FROM_PRESET;}\n",
    );
    // 预设变量替换进 -D 宏 → 编译通过，退出码 = 宏值。
    let (_, err, code) = run_ns_build(&["build", "--preset", "fast"], &d);
    assert_eq!(code, 0, "预设构建失败: {err}");
    let out = Command::new(d.join("build/predemo")).output().unwrap();
    assert_eq!(out.status.code(), Some(11), "预设 defines 未生效");

    // 命令行 --define 覆盖预设同名变量。
    let (_, err, code) = run_ns_build(&["build", "--preset", "fast", "--define", "DIST_VALUE=3"], &d);
    assert_eq!(code, 0, "CLI define 覆盖失败: {err}");
    let out = Command::new(d.join("build/predemo")).output().unwrap();
    assert_eq!(out.status.code(), Some(3), "CLI --define 应覆盖预设");

    // 未知预设：报错并列出可选项。
    let (_, err, code) = run_ns_build(&["build", "--preset", "nope"], &d);
    assert_ne!(code, 0, "未知预设应失败");
    assert!(err.contains("predemo") || err.contains("fast") || err.contains("dist"), "错误信息应列出可选预设: {err}");

    // presets 列表命令能列出两个预设。
    let (out, _, code) = run_ns_build(&["presets"], &d);
    assert_eq!(code, 0);
    assert!(out.contains("fast"), "presets 应列出 fast: {out}");
    assert!(out.contains("dist"), "presets 应列出 dist: {out}");
}

#[test]
fn export_and_import_package() {
    let d = tmpdir("export-import");

    // ---- 项目 A：mylib（静态库 + 头文件 + 导出）----
    let a = d.join("mylib");
    write(
        &a.join("ns-build.toml"),
        r#"
[project]
name = "mylib"
version = "1.2.3"

[[install_headers]]
from = "include"
to = "include"
files = ["**/*.h"]

[[targets]]
name = "mylib"
kind = "static"
sources = ["src/lib.c"]
defines = ["MYLIB_BONUS=100"]

[[exports]]
name = "mylib"
"#,
    );
    write(&a.join("include/mylib.h"), "int mylib_value(void);\n");
    write(&a.join("src/lib.c"), "int mylib_value(void){return 7;}\n");
    let prefix = d.join("prefix");
    let (_, err, code) = run_ns_build(
        &["install", "--prefix", prefix.to_str().unwrap()],
        &a,
    );
    assert_eq!(code, 0, "install/export 失败: {err}");
    assert!(prefix.join("lib/libmylib.a").exists(), "静态库未安装");
    assert!(prefix.join("include/mylib.h").exists(), "头文件未安装");
    let cfg_path = prefix.join("lib/cmake/mylib/mylib-config.toml");
    assert!(cfg_path.exists(), "导出配置未生成: {}", cfg_path.display());
    let cfg_text = std::fs::read_to_string(&cfg_path).unwrap();
    assert!(cfg_text.contains("name = \"mylib\""), "配置缺包名: {cfg_text}");
    assert!(cfg_text.contains("libmylib.a"), "配置缺产物: {cfg_text}");
    assert!(cfg_text.contains("1.2.3"), "配置缺版本: {cfg_text}");

    // ---- 项目 B：app（import 已安装的 mylib）----
    let b = d.join("app");
    write(
        &b.join("ns-build.toml"),
        &format!(
            r#"
[project]
name = "app"

[[imports]]
from = "{}"
name = "mylib"

[[targets]]
name = "app"
kind = "exe"
sources = ["src/main.c"]
"#,
            prefix.display()
        ),
    );
    write(
        &b.join("src/main.c"),
        r#"
#include <stdio.h>
#include <mylib.h>
#ifndef MYLIB_BONUS
#error MYLIB_BONUS 未被导入
#endif
int main(void) { printf("%d\n", mylib_value() + MYLIB_BONUS); return 0; }
"#,
    );
    let (_, err, code) = run_ns_build(&["build"], &b);
    assert_eq!(code, 0, "import 构建失败: {err}");
    let out = Command::new(b.join("build/app")).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "107\n",
        "import 链接/运行失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn fetch_git_and_url_packages() {
    let d = tmpdir("fetch");

    // ---- 本地 "远程" git 仓库 ----
    let remote = d.join("remote-git");
    std::fs::create_dir_all(&remote).unwrap();
    let git = |args: &[&str]| {
        let st = Command::new("git").args(args).current_dir(&remote).status().unwrap();
        assert!(st.success(), "git {:?} 失败", args);
    };
    git(&["init", "-b", "main"]);
    write(&remote.join("include/hello.h"), "#define FETCH_MAGIC 99\n");
    git(&["add", "."]);
    git(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init"]);

    // ---- 本地压缩包 ----
    let pkgdir = d.join("archive-src");
    write(&pkgdir.join("top/pkg.txt"), "pkg-content\n");
    let archive = d.join("pkg.tar.gz");
    let st = Command::new("tar")
        .args(["-czf", archive.to_str().unwrap(), "-C", pkgdir.to_str().unwrap(), "top"])
        .status()
        .unwrap();
    assert!(st.success(), "tar 打包失败");

    write(
        &d.join("ns-build.toml"),
        &format!(
            r#"
[project]
name = "fetchtest"

[[fetch]]
name = "myrepo"
git = "file://{}"

[[fetch]]
name = "mypkg"
url = "file://{}"
strip_components = 1
"#,
            remote.display(),
            archive.display()
        ),
    );

    let (_, err, code) = run_ns_build(&["fetch"], &d);
    assert_eq!(code, 0, "fetch 失败: {err}");
    assert_eq!(
        std::fs::read_to_string(d.join(".ns-fetch/myrepo/include/hello.h")).unwrap(),
        "#define FETCH_MAGIC 99\n"
    );
    assert_eq!(
        std::fs::read_to_string(d.join(".ns-fetch/mypkg/pkg.txt")).unwrap(),
        "pkg-content\n"
    );
    assert!(d.join(".ns-fetch/myrepo/.ns-fetch.toml").exists());
    assert!(d.join(".ns-fetch/mypkg/.ns-fetch.toml").exists());

    // 再次 fetch：已存在则跳过。
    let (out, _, code) = run_ns_build(&["fetch"], &d);
    assert_eq!(code, 0);
    assert!(out.contains("跳过 myrepo"), "应跳过已存在的包: {out}");

    // --update：重新拉取。
    let (_out2, err2, code2) = run_ns_build(&["fetch", "--update"], &d);
    assert_eq!(code2, 0, "--update 失败: {err2}");
    assert!(d.join(".ns-fetch/myrepo/include/hello.h").exists());

    // 非法 name：报错。
    let d2 = tmpdir("fetch-bad");
    write(
        &d2.join("ns-build.toml"),
        "[project]\nname = \"bad\"\n\n[[fetch]]\nname = \"a/b\"\ngit = \"file:///tmp/x\"\n",
    );
    let (_, err, code) = run_ns_build(&["fetch"], &d2);
    assert_ne!(code, 0);
    assert!(err.contains("非法"), "应报告非法 name: {err}");
}

#[test]
fn private_deps_link_but_not_propagate() {
    let d = tmpdir("private-deps");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "privdemo"

[[targets]]
name = "a"
kind = "static"
sources = ["src/a.c"]
include_dirs = ["include/a"]
defines = ["A_FLAG=55"]

[[targets]]
name = "b"
kind = "static"
sources = ["src/b.c"]
include_dirs = ["include/b"]
private_deps = ["a"]

[[targets]]
name = "app"
kind = "exe"
sources = ["src/main.c"]
deps = ["b"]
"#,
    );
    write(&d.join("include/a/a.h"), "int a_val(void);\n");
    write(&d.join("src/a.c"), "int a_val(void){return 5;}\n");
    write(&d.join("include/b/b.h"), "int b_val(void);\n");
    write(&d.join("src/b.c"), "#include <a.h>\n#include <b.h>\nint b_val(void){return a_val() + 2;}\n");
    write(&d.join("src/main.c"), "#include <stdio.h>\n#include <b.h>\n#ifdef A_FLAG\n#error A_FLAG 不应从 b 传播到 app\n#endif\nint main(void){printf(\"%d\\n\", b_val());return 0;}\n");
    let (_, err, code) = run_ns_build(&["build"], &d);
    assert_eq!(code, 0, "私有依赖构建失败: {err}");
    let out = Command::new(d.join("build/app")).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "7\n",
        "私有依赖链接/运行失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn checks_try_compile_and_inject() {
    let d = tmpdir("checks");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "chk"

[[checks]]
name = "has_it"
source = "int main(void){return 0;}"
define = "HAVE_IT"

[[checks]]
name = "never_true"
source = "this is not valid C code ((("
fail_fatal = false

[[targets]]
name = "chk"
kind = "exe"
sources = ["src/main.c"]
defines = ["NEVER_TRUE=${never_true}", "HAS_IT=${has_it}"]
"#,
    );
    write(
        &d.join("src/main.c"),
        "#ifndef HAVE_IT\n#error HAVE_IT 未注入\n#endif\nint main(void){return NEVER_TRUE + HAS_IT - 1;}\n",
    );
    let (_, err, code) = run_ns_build(&["build"], &d);
    assert_eq!(code, 0, "checks 构建失败: {err}");
    let out = Command::new(d.join("build/chk")).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "NEVER_TRUE=0 HAS_IT=1 应返回 0");
    assert!(d.join(".ns-cache/checks.json").exists(), "检测结果未缓存");

    // fail_fatal：检测失败直接构建报错。
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "chk2"

[[checks]]
name = "never_fatal"
source = "this is not valid C code ((("
fail_fatal = true

[[targets]]
name = "chk2"
kind = "exe"
sources = ["src/main.c"]
"#,
    );
    write(&d.join("src/main.c"), "int main(void){return 0;}\n");
    let (_, err, code) = run_ns_build(&["build"], &d);
    assert_ne!(code, 0, "fail_fatal 检测失败必须构建失败");
    assert!(err.contains("never_fatal"), "错误信息应包含检测名: {err}");
}

#[test]
fn soversion_links_and_installs() {
    let d = tmpdir("soversion");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "solver"
version = "1.0.0"

[[targets]]
name = "mylib"
kind = "lib"
sources = ["src/lib.c"]
soversion = "1"

[[targets]]
name = "app"
kind = "exe"
sources = ["src/main.c"]
deps = ["mylib"]
"#,
    );
    write(&d.join("src/lib.c"), "int mylib_answer(void){return 42;}\n");
    write(&d.join("src/main.c"), "#include <stdio.h>\nint mylib_answer(void);\nint main(void){printf(\"%d\\n\", mylib_answer());return 0;}\n");

    let (real, dev) = if cfg!(target_os = "macos") {
        ("libmylib.1.dylib", "libmylib.dylib")
    } else {
        ("libmylib.so.1", "libmylib.so")
    };

    let (_, err, code) = run_ns_build(&["build"], &d);
    assert_eq!(code, 0, "soversion 构建失败: {err}");
    let real_path = d.join("build").join(real);
    let dev_path = d.join("build").join(dev);
    assert!(real_path.exists(), "带版本动态库未生成: {}", real_path.display());
    let link_target = std::fs::read_link(&dev_path).expect("读取开发者符号链接失败");
    assert_eq!(link_target, std::path::Path::new(real), "开发者符号链接应指向带版本文件");
    // 可直接运行（链接了带版本库）。
    let out = Command::new(d.join("build/app")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "42\n");

    // install：两个名字都安装并重建符号链接。
    let prefix = d.join("prefix");
    let (_, err, code) = run_ns_build(
        &["install", "--prefix", prefix.to_str().unwrap()],
        &d,
    );
    assert_eq!(code, 0, "install 失败: {err}");
    assert!(prefix.join("lib").join(real).exists(), "install 缺少版本库");
    assert_eq!(
        std::fs::read_link(prefix.join("lib").join(dev)).unwrap(),
        std::path::PathBuf::from(real),
        "install 前缀下开发者符号链接错误"
    );
}

#[test]
fn package_creates_archive_with_pc_export() {
    let d = tmpdir("package");
    write(
        &d.join("ns-build.toml"),
        r#"
[project]
name = "mypkg"
version = "2.3.4"

[[install_headers]]
from = "include"
to = "include"
files = ["**/*.h"]

[[targets]]
name = "mypkg"
kind = "static"
sources = ["src/lib.c"]
link = ["m"]

[[exports]]
name = "mypkg"
pc = true
"#,
    );
    write(&d.join("include/mypkg.h"), "int mypkg_ans(void);\n");
    write(&d.join("src/lib.c"), "int mypkg_ans(void){return 42;}\n");

    // install：验证 .pc 内容（prefix 无关，relocatable）。
    let prefix = d.join("prefix");
    let (_, err, code) = run_ns_build(
        &["install", "--prefix", prefix.to_str().unwrap()],
        &d,
    );
    assert_eq!(code, 0, "install 失败: {err}");
    let pc_text = std::fs::read_to_string(prefix.join("lib/pkgconfig/mypkg.pc"))
        .expect("mypkg.pc 未生成");
    assert!(pc_text.contains("prefix=${pcfiledir}/../.."), "pc prefix 应 relocatable: {pc_text}");
    assert!(pc_text.contains("Name: mypkg"), "pc 缺 Name: {pc_text}");
    assert!(pc_text.contains("Version: 2.3.4"), "pc 缺版本: {pc_text}");
    assert!(pc_text.contains("-lmypkg"), "pc 缺 -lmypkg: {pc_text}");
    assert!(pc_text.contains("-lm"), "pc 缺 Libs.private: {pc_text}");
    assert!(pc_text.contains("Cflags: -I${includedir}"), "pc 缺 Cflags: {pc_text}");

    // package：打包 dist/<name>-<version>.tar.gz。
    let (_, err, code) = run_ns_build(&["package", "--format", "tar.gz"], &d);
    assert_eq!(code, 0, "package 失败: {err}");
    let archive = d.join("dist/mypkg-2.3.4.tar.gz");
    assert!(archive.exists(), "打包产物不存在: {}", archive.display());
    let listing = Command::new("tar")
        .args(["-tzf"])
        .arg(&archive)
        .output()
        .unwrap();
    let listing = String::from_utf8_lossy(&listing.stdout);
    for needle in [
        "include/mypkg.h",
        "lib/libmypkg.a",
        "lib/pkgconfig/mypkg.pc",
        "lib/cmake/mypkg/mypkg-config.toml",
    ] {
        assert!(listing.contains(needle), "打包缺少 {needle}: {listing}");
    }
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
