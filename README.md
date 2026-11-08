# ns-build

现代化、声明式、跨平台的构建系统，用 Rust 编写，性能高、速度快。

读取 `ns-build.toml` 构建脚本，调度外部编译器（gcc/clang/cc）完成编译与链接。支持分步构建、并行构建、增量构建、unity build、跨平台交叉编译。

## 快速开始

```bash
cargo build --release

# 初始化一个 C 项目
mkdir hello && cd hello
/path/to/ns-build init --name hello

# 构建
/path/to/ns-build build
# 产物在 build/hello

# 增量构建（未变任务自动跳过）
/path/to/ns-build build

# 运行
/path/to/ns-build run hello

# 测试 / 安装 / 清理
/path/to/ns-build test
/path/to/ns-build install --prefix ~/.local
/path/to/ns-build clean
```

## 构建脚本示例

`ns-build init` 生成的脚本以 `src/` 为源码根：

```toml
[project]
name = "hello"
root = "src"          # 源码根，sources 相对它解析

[[targets]]
name = "hello"
kind = "exe"
sources = ["**/*.c"]
```

一个简单的 C 可执行文件（无 `root` 时 `sources` 相对脚本目录）：

```toml
[project]
name = "hello"

[[targets]]
name = "hello"
kind = "exe"
sources = ["src/main.c"]
```

分步构建 + 并行 + 依赖（`core` 先编译成 `.o`，`app` 链接它）：

```toml
[project]
name = "multi"

[defaults]
cc = "gcc"

# 分步：编译核心为对象文件
[[targets]]
name = "core"
kind = "object"
sources = ["src/a.c", "src/b.c"]

# 可执行：依赖 core，源码 main.c + 链接库
[[targets]]
name = "app"
kind = "exe"
sources = ["src/main.c"]
deps = ["core"]
link = ["m", "pthread"]
```

unity build（多源合并编译，减少启动开销）：

```toml
[[targets]]
name = "u"
kind = "exe"
sources = ["src/**/*.c"]
groups = 2        # 300 个源文件分成 2 组，每组编译一个 .o
```

## 功能一览

| 功能 | 说明 |
|------|------|
| 增量构建 | blake3 内容指纹 + `-MMD` 头文件依赖，未变任务跳过 |
| 并行构建 | `-j N` 控制并行度（支持 -j4/-j8/-j100），默认 CPU 核数 |
| 分步构建 | `kind = "object"` 产出 `.o`，`deps` 并入链接 |
| unity build | `groups`/`group_size` 合并多源编译 |
| 多语言 | c / c++ / rust / go / java / zig / python / shell |
| 变量替换 | `${var}` 在脚本字符串中替换（`defaults.variables` + `--define`） |
| 条件构建 | `when` 表达式（`os == "linux" && arch == "x86_64"`） |
| 子脚本 | `includes = ["sub"]` 合并子目录目标（对应 `add_subdirectory`） |
| compile_commands.json | 构建时生成（供 clangd / IDE） |
| 头文件安装 | `[[install_headers]]` glob 规则 |
| 通用文件安装 | `[[install_files]]`（对应 `install(FILES/DIRECTORY)`，可排除） |
| 编译标准 | `c_std` / `cxx_std` 覆盖默认 `-std` |
| 预编译头 | `pch`（`-include` / `/FI`，支持多个头，对应 `target_precompile_headers`） |
| 测试注册 | `[[tests]]`（含 `timeout`/`labels`，对应 `add_test`） |
| pkg-config | `pkg_config = ["openssl"]` 展开 `--cflags`（编译）与 `--libs`（链接） |
| 编译器检测 | `[[checks]]` try-compile+link，结果注入 `${var}` 与 `-D` 宏（对应 `check_cxx_compiler_flag`） |
| 别名目标 | `kind = "alias"` + `alias = "name"`（对应 `add_library(ALIAS)`，零开销转发） |
| 环境自检 | `ns-build doctor`（列出外部工具是否可用及其版本） |
| LTO | `lto = true`（target/defaults → `-flto` / `/GL` + `/LTCG`） |
| 警告即错误 | `werror`（target/defaults → `-Werror` / `/WX`） |
| 构建配置 | `--release`/`--debug`/`--profile`（4 种：Release/Debug/RelWithDebInfo/MinSizeRel） |
| 跨平台 | OS/架构检测，`os`/`arch` 平台过滤 |
| 交叉工具链 | `[[toolchains]]`，可用 `target.toolchain` / `defaults.toolchain` 选择 |
| 产物命名 | 按目标平台命名：`app.exe` / `libfoo.dylib` / `libfoo.so` / `foo.dll` / `libfoo.a` |
| 依赖使用传播 | `deps` 的 `include_dirs`/`defines`/`link` 等使用要求**传递性**传播；`private_deps` 仅本级使用（CMake PRIVATE） |
| 静态/动态库链接 | `deps` 直接静态/动态库产物并链接；静态库链式依赖自动递归收集（含 private_deps） |
| 动态库运行 | 链接构建树内 `.so`/`.dylib` 时自动写入 `-Wl,-rpath`；target `rpath` 可嵌入 `$ORIGIN`（安装后可定位 prefix/lib） |
| 库版本/SONAME | `soversion = "1"` → `libfoo.so.1` / `libfoo.1.dylib` + 开发者符号链接（soname / install_name） |
| 产物类型 | `exe` / `lib` / `module` / `static` / `object` / `interface` / `phony` / `alias` |
| 钩子 | `hooks.before`/`hooks.after` 构建前后自定义命令 |
| 目标过滤 | `ns-build build <target>` 只构建指定目标（含传递依赖） |
| 生成代码管线 | `[[custom_commands]]`（产物 + 依赖 + 增量，等价 `add_custom_command(OUTPUT)`） |
| 安装导出 | `[[exports]]` 写入 `<name>-config.toml`（可选 `pc = true` 生成 relocatable `.pc`）；`[[imports]]` 读回并链接（`install(EXPORT)` / `find_package` 等价） |
| 打包 | `ns-build package`（install → staging → `tar.gz`/`zip`，CPack 简化版） |
| 卸载 | `ns-build uninstall [--prefix P]`（按 install 清单删除已安装文件） |
| 依赖完整性 | `[[fetch]]` 的 `sha256`（URL_HASH 对等） |
| 静态闭包 | `pkg_config_static`（`--libs --static` 展开 `Libs.private`） |
| 安装目录名 | `[install] bindir`/`libdir`/`includedir`（CMAKE_INSTALL_BINDIR/LIBDIR/INCLUDEDIR 对等） |
| 导入版本约束 | `[[imports]]` 的 `version`（`>=` / `=` / `<` 半语义比较，对等 find_package 版本 REQUIRED） |
| 包元数据 | `[project] description` / `license`（进 `.pc` 的 `Description`） |
| 构建预设 | `[presets]` + `--preset` / `ns-build presets`（对应 `cmake --preset`） |
| 外部依赖 | `[[fetch]]` + `ns-build fetch`（git / url，`FetchContent` 等价，支持 file:// 离线） |
| MSVC 工具链 | `cl` / `clang-cl` 命令生成（未在真实 MSVC SDK 本机验证） |
| 子脚本菱形去重 | include 菱形引用只解析一次 + 环检测（CMake `include_guard(GLOBAL)`） |
| 机器可读输出 | `ns-build show --json` |
| 安装瘦身 | `ns-build install --strip` / `package --strip`（strip 安装副本） |

## 与 CMake 对比

| CMake 功能 | ns-build 等价写法 | 支持 |
|------------|------------------|------|
| `add_executable` | `[[targets]] kind = "exe"` | ✅ |
| `add_library` (STATIC) | `kind = "static"` | ✅ |
| `add_library` (SHARED) | `kind = "lib"` | ✅ |
| `add_library` (MODULE) | `kind = "module"` | ✅ |
| `add_library` (OBJECT) | `kind = "object"` | ✅ |
| `add_library` (INTERFACE) | `kind = "interface"`（头文件库） | ✅ |
| `target_link_libraries(PRIVATE/PUBLIC)` | `private_deps`（PRIVATE）/ `deps`（PUBLIC） | ✅ |
| `target_link_libraries` | `deps` + `link` | ✅ |
| `target_include_directories` | `include_dirs` | ✅ |
| `target_compile_definitions` | `defines` | ✅ |
| `target_compile_options` | `flags` / `cflags` / `cxxflags` | ✅ |
| `target_compile_options`（警告） | `warnings = true`（`-Wall -Wextra`）/ `warnings = false` | ✅ |
| `target_compile_options`（as error） | `werror = true`（`-Werror` / `/WX`） | ✅ |
| `target_compile_features` | `c_std` / `cxx_std` | ✅ |
| `INTERPROCEDURAL_OPTIMIZATION` | `lto = true`（`-flto` / `/GL` + `/LTCG`） | ✅ |
| `add_library(ALIAS ...)` | `kind = "alias"` + `alias` | ✅ |
| `enable_testing()` 并行运行 | `ns-build test -j N` | ✅ |
| `target_precompile_headers` | `pch`（字符串或数组，`-include` / `/FI`） | ✅ |
| `check_cxx_compiler_flag` / `CHECK_INCLUDE_FILE` | `[[checks]]` try-compile + 磁盘缓存 | ✅ |
| `SOVERSION` | `soversion = "1"`（soname / install_name + 开发者符号链接） | ✅ |
| `BUILD_RPATH` / `INSTALL_RPATH` | `rpath = [...]`（含 `$ORIGIN`） | ✅ |
| `install(EXPORT)` / `find_package`（含 `.pc`） | `[[exports]]` / `[[imports]]`（TOML 配置；`pc = true` 生成 relocatable `.pc`；含 `version` 约束） | ✅ |
| `CMAKE_INSTALL_BINDIR` 等 | `[install] bindir` / `libdir` / `includedir` | ✅ |
| `CPack` | `ns-build package`（staging + `.tar.gz`/`.zip`） | ✅ |
| `add_dependencies` | `deps`（顺序依赖） | ✅ |
| `add_custom_command` | `[[custom_commands]]`（产物追踪 + 增量 + 链式构建） | ✅ |
| `install(EXPORT)` / `find_package` | `[[exports]]` / `[[imports]]`（TOML 配置） | ✅ |
| `cmake --preset` | `[presets]` + `--preset` | ✅ |
| `FetchContent` / `ExternalProject` | `[[fetch]]`（git/url，按需拉取，含 `sha256` 校验） | ✅ |
| `EXTPROJECT_URL_HASH` | `[[fetch]]` 的 `sha256` | ✅ |
| `CPACK_MORE_FILES`/清单卸载 | `ns-build uninstall`（install 清单） | ✅ |
| `pkg_check_modules(STATIC)` | `pkg_config_static`（`--libs --static`） | ✅ |
| 构建配置 | `--release`/`--debug`/`--profile`（Release/Debug/RelWithDebInfo/MinSizeRel） | ✅ |
| `option()` / 缓存变量 | `[defaults].variables` + `--define name=value` | ✅ |
| `install(TARGETS)` | `install = true` | ✅ |
| `install(FILES/DIRECTORY)` | `[[install_files]]` | ✅ |
| `install(EXPORT)` + `.pc` | `[[exports]] pc = true`（relocatable `.pc`） | ✅ |
| `install(DIRECTORY ... PATTERN EXCLUDE)` | `[[install_files]] exclude` | ✅ |
| `enable_testing()` / `add_test` | `[[tests]]`（含 `timeout`/`labels`） | ✅ |
| 生成器 | 自动平台默认（无需指定） | ✅ |
| 交叉编译 / 工具链 | `[[toolchains]]` + `toolchain` 选择（或按 os/arch 自动匹配） | ✅ |
| `find_package` / pkg-config | `pkg_config` | ✅ |
| `file(GLOB)` | `sources = ["src/**/*.c"]` glob | ✅ |
| `add_subdirectory` | `includes = ["sub"]` | ✅ |
| `include_guard(GLOBAL)` | includes 菱形引用去重 | ✅ |
| unity build | `groups` / `group_size`（默认 auto ≥4 源） | ✅ |
| 生成 compile_commands.json | 自动生成 | ✅ |
| `add_compile_options`/`add_definitions`（全局） | `[defaults].flags` / `defines` | ✅ |
| `target_link_options` | `link_flags` | ✅ |
| 多配置生成器 | 单配置（`--profile` 选择） | ➖ 声明式简化 |
| 生成器表达式 `$<...>` | `${var}` 变量 + `when` 条件 | ➖ 声明式覆盖 |
| 预设（presets） | `[presets]` + `--preset` | ✅ |
| MSVC（cl/clang-cl） | 目标/工具链 cc=cl | ➖ 命令生成已测，未本机编译验证 |

> **设计取舍**：CMake 是命令式语言（if/循环/函数）；ns-build 刻意保持**声明式** TOML，用 `when` 条件 + `include` 子脚本 + 变量替换覆盖常见动态需求，换来更简单、更现代的脚本。

## 性能（实测）

300 文件 C 项目（macOS 12 核 · clang 17 · 运行 `scripts/bench.sh`）：

| 构建方式 | 全量 | 增量（无变化） | 增量（改 1 文件） |
|----------|------|---------------|-------------------|
| **ns-build**（unity auto → 12 组） | **0.28s** | **0.03s** | 0.18s |
| **ns-build**（unity off，逐文件） | 3.0–3.5s | 0.20s | 0.35s |
| **CMake + Ninja**（302 任务） | 3.2–3.6s | 0.04s | 0.21s |

- **unity 模式**：ns-build 将 300 文件自动分组合并编译，**比 CMake 快约 12×**（≈0.28s vs ≈3.4s）。
- **逐文件模式**：与 CMake+Ninja 基本持平，差距来自 ns-build 额外记录增量指纹与生成 compile_commands.json。
- **增量**：未变任务 0.03s（近乎零开销）；改 1 文件重编译对应 unity 组 + 链接。
- **性能核心**：unity 自动分组（≥4 源自动开）+ 并行调度，是相对 CMake 的主要优势。

## 已知边界

- MSVC：命令生成（cl/clang-cl）已单元测试覆盖，但未在真实 MSVC SDK 上本机验证；MSVC 风格不生成依赖文件（改头文件不触发重编）。
- `pkg-config` 是 POSIX 工具，MSVC 风格下被忽略。
- `zip` 压缩包不支持 `strip_components`（仅 tar）。
- 单配置构建（用 `--profile` 切换）。
- `pkg-config` 查询失败时按无该依赖处理（不展开 `-I`/`-l`），不会报错中断。

## 开发

```bash
cargo test --lib          # 单元测试（59 个：解析/depfile/缓存/图/调度/编译器/导出配置/检测/LTO）
cargo test --test e2e     # 端到端（43 个：构建/运行/增量/分步/unity/静态·动态库/安装/过滤/钩子/配置/init/生成代码/预设/导出导入/fetch/sha256/checks/soversion/package/alias/doctor/并行测试/include/show --json/strip/link_dirs/pkg_config_static/uninstall/macOS@loader_path/imports version/自定义 install 目录名/审计回归）
cargo test --test languages  # 多语言（C++ unity 推断等）
cargo test --test perf    # 性能（300 文件全量 vs 增量）
./scripts/bench.sh        # 与 CMake+Ninja 对比基准
```

详细架构与约定见 [AGENTS.md](AGENTS.md)。

## 许可

MIT
