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

一个简单的 C 可执行文件：

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
| 并行构建 | `-j N` 控制并行度，默认 CPU 核数 |
| 分步构建 | `kind = "object"` 产出 `.o`，`deps` 并入链接 |
| unity build | `groups`/`group_size` 合并多源编译 |
| 跨平台 | OS/架构检测，`os`/`arch` 平台过滤，`[[toolchains]]` 交叉编译 |
| 产物类型 | `exe` / `lib` / `static` / `object` / `phony` |
| 钩子 | `hooks.before`/`hooks.after` 自定义命令 |
| 目标过滤 | `ns-build build <target>` 只构建指定目标 |

## 性能

300 文件全量构建 ≈ **0.7s**，增量 ≈ **0.03s**（约 **24×** 加速），本机验证（clang 17，macOS）。

## 开发

```bash
cargo test --lib       # 单元测试
cargo test --test e2e  # 端到端（真实 C 构建 + 运行）
cargo test --test perf # 性能基准（300 文件）
```

详细架构与约定见 [AGENTS.md](AGENTS.md)。

## 许可

MIT
