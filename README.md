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
| 预编译头 | `pch`（`-include`，对应 `target_precompile_headers`） |
| 测试注册 | `[[tests]]`（含 `timeout`/`labels`，对应 `add_test`） |
| pkg-config | `pkg_config = ["openssl"]` 展开 `--cflags`（编译）与 `--libs`（链接） |
| 构建配置 | `--release`/`--debug`/`--profile`（4 种：Release/Debug/RelWithDebInfo/MinSizeRel） |
| 跨平台 | OS/架构检测，`os`/`arch` 平台过滤 |
| 交叉工具链 | `[[toolchains]]`，可用 `target.toolchain` / `defaults.toolchain` 选择 |
| 产物命名 | 按目标平台命名：`app.exe` / `libfoo.dylib` / `libfoo.so` / `foo.dll` / `libfoo.a` |
| 依赖使用传播 | 依赖目标的 `include_dirs`/`defines`/`link` 等使用要求**传递性**传播到下游 |
| 静态/动态库链接 | `deps` 直接静态/动态库产物并链接；静态库链式依赖自动递归收集 |
| 动态库运行 | 链接构建树内 `.so`/`.dylib` 时自动写入 `-Wl,-rpath`，未安装也可直接运行 |
| 产物类型 | `exe` / `lib` / `module` / `static` / `object` / `interface` / `phony` |
| 钩子 | `hooks.before`/`hooks.after` 构建前后自定义命令 |
| 目标过滤 | `ns-build build <target>` 只构建指定目标（含传递依赖） |

## 与 CMake 对比

| CMake 功能 | ns-build 等价写法 | 支持 |
|------------|------------------|------|
| `add_executable` | `[[targets]] kind = "exe"` | ✅ |
| `add_library` (STATIC) | `kind = "static"` | ✅ |
| `add_library` (SHARED) | `kind = "lib"` | ✅ |
| `add_library` (MODULE) | `kind = "module"` | ✅ |
| `add_library` (OBJECT) | `kind = "object"` | ✅ |
| `add_library` (INTERFACE) | `kind = "interface"`（头文件库） | ✅ |
| `target_link_libraries` | `deps` + `link` | ✅ |
| `target_include_directories` | `include_dirs` | ✅ |
| `target_compile_definitions` | `defines` | ✅ |
| `target_compile_options` | `flags` / `cflags` / `cxxflags` | ✅ |
| `target_compile_features` | `c_std` / `cxx_std` | ✅ |
| `target_precompile_headers` | `pch`（`-include`） | ✅ |
| `add_dependencies` | `deps`（顺序依赖） | ✅ |
| `add_custom_command` | `hooks.before`/`hooks.after`（构建前后命令，不追踪自定义产物） | ➖ 部分 |
| 构建配置 | `--release`/`--debug`/`--profile`（Release/Debug/RelWithDebInfo/MinSizeRel） | ✅ |
| `option()` / 缓存变量 | `[defaults].variables` + `--define name=value` | ✅ |
| `install(TARGETS)` | `install = true` | ✅ |
| `install(FILES/DIRECTORY)` | `[[install_files]]` | ✅ |
| `install(DIRECTORY ... PATTERN EXCLUDE)` | `[[install_files]] exclude` | ✅ |
| `enable_testing()` / `add_test` | `[[tests]]`（含 `timeout`/`labels`） | ✅ |
| 生成器 | 自动平台默认（无需指定） | ✅ |
| 交叉编译 / 工具链 | `[[toolchains]]` + `toolchain` 选择（或按 os/arch 自动匹配） | ✅ |
| `find_package` / pkg-config | `pkg_config` | ✅ |
| `file(GLOB)` | `sources = ["src/**/*.c"]` glob | ✅ |
| `add_subdirectory` | `includes = ["sub"]` | ✅ |
| unity build | `groups` / `group_size`（默认 auto ≥4 源） | ✅ |
| 生成 compile_commands.json | 自动生成 | ✅ |
| `add_compile_options`/`add_definitions`（全局） | `[defaults].flags` / `defines` | ✅ |
| `target_link_options` | `link_flags` | ✅ |
| 多配置生成器 | 单配置（`--profile` 选择） | ➖ 声明式简化 |
| 生成器表达式 `$<...>` | `${var}` 变量 + `when` 条件 | ➖ 声明式覆盖 |
| `ExternalProject`/`FetchContent` | 外部工具链 + `hooks` | ➖ 规划中 |
| 预设（presets） | 脚本即预设（声明式） | ➖ 设计取舍 |

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

- 无 MSVC 支持（命令行假定 GCC/Clang/MinGW 风格）。
- 钩子只在构建前后执行，不追踪自定义产物；无 `add_custom_command(OUTPUT)` 式生成步骤。
- 单配置构建（用 `--profile` 切换）。
- `pkg-config` 查询失败时按无该依赖处理（不展开 `-I`/`-l`），不会报错中断。

## 开发

```bash
cargo test --lib          # 单元测试（43 个：解析/depfile/缓存/图/调度/编译器）
cargo test --test e2e     # 端到端（真实 C 项目：构建/运行/增量/分步/unity/静态/动态库/安装）
cargo test --test languages  # 多语言（C++ unity 推断等）
cargo test --test perf    # 性能（300 文件全量 vs 增量）
./scripts/bench.sh        # 与 CMake+Ninja 对比基准
```

详细架构与约定见 [AGENTS.md](AGENTS.md)。

## 许可

MIT
