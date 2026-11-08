# ns-build — 项目指南

现代化、声明式、跨平台的构建系统。读取 `ns-build.toml` 构建脚本，调度外部编译器（gcc/clang/cc）完成编译与链接。

## 核心理念

- **声明式**：脚本只描述"想要什么"（编译器、源文件、链接库、产物），无隐式状态、无过程式控制流。
- **任务 DAG**：每个编译/链接步骤是一个节点，依赖构成有向无环图，调度器并行执行。
- **增量构建**：输入内容指纹（blake3）+ 头文件依赖（`-MMD`），未变任务直接跳过。
- **分步构建**：`kind = "object"` 目标只产出 `.o`，供其他目标链接。
- **并行构建**：`-j N` 控制并行度，默认 CPU 核数；unity build 合并多源文件减少启动开销。
- **跨平台跨架构**：OS/架构检测、`os`/`arch` 平台过滤、`[[toolchains]]` 交叉工具链。
- **多语言**：`language` 字段支持 c / c++ / rust / go / java / zig / python / shell。
- **声明式语法扩展**：`${var}` 变量替换、`when` 条件构建、`include` 子脚本、`[[tests]]`、`pkg_config`、`[[install_headers]]`、`c_std`/`cxx_std`。
- **工具链集成**：构建时自动生成 `build/compile_commands.json`（CMake 兼容，供 clangd/IDE）。
- **生成代码管线**：`[[custom_commands]]` 产物可作为源，自动建 DAG 边 + 增量追踪（CMake `add_custom_command(OUTPUT)`）。
- **包导出/导入**：`[[exports]]` 写 `<name>-config.toml`，`[[imports]]` 全局并入（`install(EXPORT)` / `find_package`）。
- **构建预设**：`[presets]` + `--preset`（`cmake --preset`）；**外部依赖**：`[[fetch]]` + `ns-build fetch`（`FetchContent`）。
- **依赖粒度**：`private_deps` 参与编译/链接但不重导出（CMake PRIVATE）；**特性检测**：`[[checks]]` try-compile 缓存注入变量（`check_cxx_compiler_flag`）。
- **库版本**：`soversion`（soname/install_name + 开发者符号链接）；**打包**：`ns-build package`（`.tar.gz`/`.zip`）。

## 构建脚本（`ns-build.toml`）格式

```toml
[project]
name = "hello"
version = "0.1.0"
root = "src"          # 源码根（相对脚本），sources 相对它解析；init 默认生成
out = "build"         # 产物目录（默认 build）

[defaults]
cc = "cc"             # 编译器（gcc/clang/cc）
cxx = "c++"
ar = "ar"
flags = []            # 全局编译标志
cflags = []           # C 编译标志
cxxflags = []         # C++ 编译标志
link_flags = []       # 全局链接标志
include_dirs = []     # 全局 include 目录
defines = []          # 全局宏定义 (-D)
profile = "release"   # release | debug
parallel = 0          # 0 = CPU 核数
unity = "auto"        # off | on | auto（auto = ≥4 源自动分组，组数≈n/4，封顶并行度）
strip = false
warnings = true
werror = false        # 可选：警告即错误（-Werror / /WX）
toolchain = "mingw"   # 可选：默认工具链名；targets 可逐个覆盖
cache = ".ns-cache"   # 增量缓存目录

[[targets]]
name = "app"
kind = "exe"          # exe | lib | static | object | module | interface | phony
cc = "gcc"            # 覆盖 defaults.cc（覆盖工具链）
toolchain = "mingw"   # 可选：指定 [[toolchains]]（覆盖 defaults.toolchain）
sources = ["src/**/*.c"]   # 支持 glob，相对 project.root
include_dirs = []
flags = []
cflags = []
cxxflags = []
link = ["m", "pthread"]     # 链接库（-l）或路径
link_dirs = []
link_flags = []
defines = []
deps = ["core"]       # object 库对象并入；静态/动态库产物直接链接，静态链式依赖递归收集
private_deps = ["implib"]   # 私有依赖：参与编译/链接但不重导出给下游（CMake PRIVATE）
output = "app"        # 产物名（默认 target 名 + 平台后缀：.exe / lib*.so|.dylib|.dll / lib*.a）
soversion = "1"       # 可选：动态库 → lib*.so.1 + soname / lib*.1.dylib + @rpath（unix）
rpath = ["$ORIGIN/../lib"]   # 可选：写入 -Wl,-rpath，支持 $ORIGIN 与绝对路径
groups = 2            # unity 分组数（每组合并编译一个 .o）
group_size = 4        # 每组源文件数（与 groups 二选一）
parallel = true       # 是否参与并行
os = ["linux", "macos"]   # 平台过滤（按目标平台，即工具链平台）
arch = ["x86_64"]
profile = "release"
strip = false
install = true
language = "c"        # c | c++ | rust | go | java | zig | python | shell（省略按源文件后缀推断 C/C++）
when = 'os == "linux"'  # 条件构建表达式（支持 && || ! 与括号分组）
pkg_config = []       # pkg-config 包（--cflags 编译展开 / --libs 链接展开）
c_std = "c11"         # 编译标准（c11/gnu11/c17）
cxx_std = "c++17"     # C++ 标准（c++17/c++20）
pch = "include/pre.h" # 预编译头（-include）

[targets.hooks]       # 构建前后钩子
before = ["echo building"]
after = [{ program = "sh", args = ["-c", "echo done"] }]

[[toolchains]]        # 交叉编译工具链
name = "mingw"
cc = "x86_64-w64-mingw32-gcc"
os = "windows"
arch = "x86_64"
flags = []
sysroot = "/usr/x86_64-w64-mingw32"

[[custom_commands]]    # 生成代码（add_custom_command(OUTPUT)）
name = "version"
outputs = ["build/gen/version.c"]
command = { program = "sh", args = ["-c", "printf 'int version(void){return 42;}\n' > build/gen/version.c"] }
depends = ["template.txt"]

[presets]              # 命名预设（cmake --preset）
dist = { profile = "release", jobs = 8 }
```

### Target 种类

| kind | 产物 | 说明 |
|------|------|------|
| `exe` | 可执行文件 | 编译源 → 链接 |
| `lib` | 动态库 (`.so`/`.dylib`/`.dll`) | `-shared` 链接 |
| `module` | 动态模块（`.so`/`.dylib`/`.dll`） | dlopen 用，默认不进链接 |
| `static` | 静态库 (`.a`) | `ar rcs` 归档 |
| `object` | `.o` 文件 | 分步构建中间产物，供链接 |
| `interface` | 无产物 | 头文件库：只向依赖者传递 include_dirs/defines |
| `phony` | 无 | 聚合节点，仅依赖 |

## CLI 命令

```bash
ns-build build [-j N] [--release|--debug|--profile <配置>] [--define name=value] [--verbose] [--quiet] [目标...]
ns-build clean
ns-build test [--all]
ns-build install [--prefix <路径>]
ns-build run <目标> [-- 参数...]
ns-build show
ns-build graph        # 输出 DOT 依赖图
ns-build presets      # 列出 [presets] 预设
ns-build fetch [--update]  # 拉取 [[fetch]] 外部依赖
ns-build package [--format tar.gz|zip]  # install 到 staging 并打包
ns-build init [--name <名>]
```

## 架构（源码组织）

```
src/
├── lib.rs           # 模块根，重新导出公共 API
├── platform.rs      # OS/架构检测、命名规则（dylib/exe/static 后缀）
├── script.rs        # ns-build.toml schema、解析、校验、规范化（glob 展开/平台过滤/默认值合并）
├── depfile.rs       # Makefile 风格 .d 依赖解析（-MMD，支持 `\ ` 转义空格、`$$` 转义美元符）
├── cache.rs         # blake3 内容指纹增量缓存（持久化 JSON）
├── fsutil.rs        # 哈希/路径规范化/原子写/时间戳
├── compiler.rs      # 编译/链接/归档命令生成 + unity 合并源
├── graph.rs         # 构建图 DAG：节点、依赖边、拓扑排序
├── planner.rs       # 目标→任务 DAG 规划（unity 分组、deps 对象并入、缓存判定）
├── executor.rs      # 并行调度器（Kahn 就绪队列 + 有界并发 + 缓存失效传播）
├── install.rs       # 安装规则（产物→prefix）
├── runner.rs        # 测试/单目标运行
├── export.rs        # 安装导出/导入（install(EXPORT)/find_package，TOML 配置）
└── main.rs          # CLI 入口
```

### 数据流

1. `load(dir)` → 读 `ns-build.toml`，`BuildScript::parse` 反序列化。
2. `BuildScript::resolve(dir)` → `ResolvedScript`：imports 合并 → `[[checks]]` try-compile（磁盘缓存注入 `${var}` 与全局 -D）→ 自定义命令两遍解析（先收集 outputs 再解析 depends/command）→ include 子脚本合并（父级 outputs 作为继承存在性检查）→ targets 解析（glob 展开、平台过滤）；依赖使用要求由 propagate_usage 全局传播（`deps` PUBLIC 重导出 / `private_deps` 仅本级）。
3. `planner::plan(script, cache)` → `Plan { graph, outputs, skipped }`：
   - 第零遍：`[[custom_commands]]` 生成命令节点（`custom:<name>` 键，缓存按 depends 内容哈希，outputs 全在才缓存命中），再按 `deps` 连成自身 DAG。
   - 第一遍：每个非 phony/interface target 生成对象编译节点（unity 分组）；引用自定义产物的 chunk/源，自动挂依赖边到对应 GEN 节点。
   - 第二遍：构造链接/归档节点；`deps` 处理：object 对象并入、静态/动态库产物直接链接、静态链式依赖递归收集；链接构建树内 `.so`/`.dylib` 自动加 `-Wl,-rpath`。
   - 第三遍：target 间依赖边 + 钩子节点。
   - 每任务用 `cache.hit` 判定是否可跳过（无产物节点不缓存、不传播失效）。
4. `executor::execute(plan, cache, opts, cwd)` → `ExecReport`：
   - Kahn 就绪队列，就绪任务派发到线程池（≤ `parallel`）。
   - 串行任务（钩子）等待并发清空。
   - 真执行的节点完成后：记录缓存 + **清除所有后继的缓存标记**（输入已变）。
   - 失败后停止派发新任务，等待在途完成。

### 关键设计决策

- **缓存失效传播**：对象重编译后，其链接后继必须重跑（不能只看链接输入路径不变）。executor 在任务真正执行后清除后继 `cached` 标志。
- **unity build**：生成 `build/unity/<target>_<i>.c`（内容为各源 `#include`），编译单文件避免多源 `-o` 冲突与重复启动开销。
- **命令指纹**：完整命令数组 join 后 blake3 哈希，任何编译参数变化都会使缓存失效。
- **并行度控制**：`running` 原子计数与 `opts.parallel` 比较，防止超限派发。

## 测试

```bash
cargo test --lib      # 单元测试（58：解析/depfile/缓存/图/调度/编译器/脚本规范化/导出配置/soversion）
cargo test --test e2e # 端到端（24：构建/运行/增量/分步/unity/静态·动态库/安装/过滤/钩子/配置/init/生成代码/预设/导出导入/fetch/checks/package）
cargo test --test languages # 多语言 e2e（8：含 C++ unity 与 C++ 推断）
cargo test --test perf # 性能：300 文件全量 vs 增量（增量应显著更快）
./scripts/bench.sh    # 与 CMake + Ninja 对比基准（NS_BENCH_N 可调规模）
```

**当前全部绿**：58 lib + 3 main(CLI) + 24 e2e + 8 languages + 1 perf = **94 个测试，0 警告 0 错误**（不使用 `#[allow]`/`#[warn]` 等抑制手段；`cargo clippy --all-targets` 亦零警告）。

**性能对比**（300 文件 C 项目，macOS 12 核 clang 17，`scripts/bench.sh`）：

| 方式 | 全量 | 增量 | 改 1 文件 |
|------|------|------|-----------|
| ns-build（unity auto → 12 组） | ≈0.27s | ≈0.03s | ≈0.18s |
| ns-build（unity off，逐文件 302 任务） | ≈3.0s | ≈0.20s | ≈0.35s |
| CMake + Ninja（302 任务） | ≈3.3s | ≈0.04s | ≈0.21s |

ns-build 默认 unity 自动分组，比 CMake 快约 12×；逐文件模式与 CMake+Ninja 基本持平。

## 已知边界

- MSVC（cl/clang-cl）：命令生成已单元测试覆盖，但未在真实 MSVC SDK 上本机验证；MSVC 风格不生成依赖文件（改头文件不触发重编），`pkg_config` 被忽略。
- `zip` 压缩包不支持 `strip_components`（仅 tar）。
- 单配置构建（用 `--profile` 切换）；`when` 覆盖条件编译的常见场景，无生成器表达式。
- `pkg-config` 查询失败时按无该依赖处理（不展开 `-I`/`-l`），不阻塞构建。
- `install` 会构建所有目标（不限于安装产物）；`[[exports]]` 只写配置，不自动安装头文件（头文件走 `[[install_headers]]`）。
- 导入包的使用要求是**全局**的（如同全局 find_package），无 target 级作用域；导入链接顺序按配置声明顺序追加。
- `soversion` 仅 unix 流派（Linux `.so.N` + soname、macOS `.N.dylib` + `@rpath` install_name）；Windows 忽略。
- `ns-build package` 需要 bsdtar（macOS/Windows 自带）或 GNU tar / zip；zip 不支持 `strip_components`。
- `[[checks]]` 结果按（编译器+源+宏+库）键缓存于 `.ns-cache/checks.json`；改任一输入即重新检测。

## 编码约定

- 正确性优先，其次是可维护性；避免不必要的抽象。
- 新功能加对应测试；改动行为需更新相关测试。
- 编译命令生成集中在 `compiler.rs`，调度逻辑在 `executor.rs`，不跨模块混杂。
- 错误返回 `Result<_, String>`，消息含具体上下文（文件路径、target 名）。
- 质量门槛：`cargo build`、`cargo test --all`、`cargo clippy --all-targets` 全部 0 警告 0 错误；禁止为了消警告添加 `#[allow]` / `#[warn]` 等抑制。
