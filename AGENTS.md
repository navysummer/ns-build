# ns-build — 项目指南

现代化、声明式、跨平台的构建系统。读取 `ns-build.toml` 构建脚本，调度外部编译器（gcc/clang/cc）完成编译与链接。

## 核心理念

- **声明式**：脚本只描述"想要什么"（编译器、源文件、链接库、产物），无隐式状态、无过程式控制流。
- **任务 DAG**：每个编译/链接步骤是一个节点，依赖构成有向无环图，调度器并行执行。
- **增量构建**：输入内容指纹（blake3）+ 头文件依赖（`-MMD`），未变任务直接跳过。
- **分步构建**：`kind = "object"` 目标只产出 `.o`，供其他目标链接。
- **并行构建**：`-j N` 控制并行度，默认 CPU 核数；unity build 合并多源文件减少启动开销。
- **跨平台跨架构**：OS/架构检测、`os`/`arch` 平台过滤、`[[toolchains]]` 交叉工具链。

## 构建脚本（`ns-build.toml`）格式

```toml
[project]
name = "hello"
version = "0.1.0"
root = "src"          # 源码根（相对脚本）
out = "build"         # 产物目录

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
unity = "auto"        # off | on | auto
strip = false
warnings = true
cache = ".ns-cache"   # 增量缓存目录

[[targets]]
name = "app"
kind = "exe"          # exe | lib | static | object | phony
cc = "gcc"            # 覆盖 defaults.cc
sources = ["src/**/*.c"]   # 支持 glob
include_dirs = []
flags = []
cflags = []
cxxflags = []
link = ["m", "pthread"]     # 链接库（-l）或路径
link_dirs = []
link_flags = []
defines = []
deps = ["core"]       # 依赖其他 target（对象并入链接）
output = "app"        # 产物名（默认 target 名）
groups = 2            # unity 分组数（每组合并编译一个 .o）
group_size = 4        # 每组源文件数（与 groups 二选一）
parallel = true       # 是否参与并行
os = ["linux", "macos"]   # 平台过滤
arch = ["x86_64"]
profile = "release"
strip = false
install = true

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
```

### Target 种类

| kind | 产物 | 说明 |
|------|------|------|
| `exe` | 可执行文件 | 编译源 → 链接 |
| `lib` | 动态库 (`.so`/`.dylib`/`.dll`) | `-shared` 链接 |
| `static` | 静态库 (`.a`) | `ar rcs` 归档 |
| `object` | `.o` 文件 | 分步构建中间产物，供链接 |
| `phony` | 无 | 聚合节点，仅依赖 |

## CLI 命令

```bash
ns-build build [-j N] [--debug] [--verbose] [--quiet] [目标...]
ns-build clean
ns-build test [--all]
ns-build install [--prefix <路径>]
ns-build run <目标> [-- 参数...]
ns-build show
ns-build graph        # 输出 DOT 依赖图
ns-build init [--name <名>]
```

## 架构（源码组织）

```
src/
├── lib.rs           # 模块根，重新导出公共 API
├── platform.rs      # OS/架构检测、命名规则（dylib/exe/static 后缀）
├── script.rs        # ns-build.toml schema、解析、校验、规范化（glob 展开/平台过滤/默认值合并）
├── depfile.rs       # Makefile 风格 .d 依赖解析（-MMD）
├── cache.rs         # blake3 内容指纹增量缓存（持久化 JSON）
├── fsutil.rs        # 哈希/路径规范化/原子写/时间戳
├── compiler.rs      # 编译/链接/归档命令生成 + unity 合并源
├── graph.rs         # 构建图 DAG：节点、依赖边、拓扑排序
├── planner.rs       # 目标→任务 DAG 规划（unity 分组、deps 对象并入、缓存判定）
├── executor.rs      # 并行调度器（Kahn 就绪队列 + 有界并发 + 缓存失效传播）
├── install.rs       # 安装规则（产物→prefix）
├── runner.rs        # 测试/单目标运行
└── main.rs          # CLI 入口
```

### 数据流

1. `load(dir)` → 读 `ns-build.toml`，`BuildScript::parse` 反序列化。
2. `BuildScript::resolve(dir)` → `ResolvedScript`：glob 展开、平台过滤、工具链匹配、defaults 合并、相对路径解析。
3. `planner::plan(script, cache)` → `Plan { graph, outputs, skipped }`：
   - 第一遍：每个非 phony target 生成对象编译节点（unity 分组）。
   - 第二遍：构造链接/归档节点，`deps` 对象并入链接输入。
   - 第三遍：target 间依赖边 + 钩子节点。
   - 每任务用 `cache.hit` 判定是否可跳过。
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
cargo test --lib      # 单元测试（解析/depfile/缓存/图/调度/编译器）
cargo test --test e2e # 端到端：真实 C 项目构建 + 运行 + 增量 + 分步 + unity + 安装
cargo test --test perf # 性能：300 文件全量 vs 增量（增量应显著更快）
```

性能结果（本机验证）：300 文件全量 ≈ 0.71s，增量 ≈ 0.03s（约 24× 加速）。

## 编码约定

- 正确性优先，其次是可维护性；避免不必要的抽象。
- 新功能加对应测试；改动行为需更新相关测试。
- 编译命令生成集中在 `compiler.rs`，调度逻辑在 `executor.rs`，不跨模块混杂。
- 错误返回 `Result<_, String>`，消息含具体上下文（文件路径、target 名）。
