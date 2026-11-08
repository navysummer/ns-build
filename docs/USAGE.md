# ns-build 使用文档

`ns-build` 是现代化、声明式、跨平台的构建系统。读取 `ns-build.toml`，调度外部编译器（cc / gcc / clang / g++ / rustc / go / javac / zig）完成构建。

## 命令

| 命令 | 说明 |
|------|------|
| `ns-build build [目标...]` | 构建（默认所有目标；指定则只构建这些） |
| `ns-build clean` | 删除 `build/` 与缓存 |
| `ns-build test [--all] [--label L] [-j N]` | 构建并运行测试（`--label` 只跑带该标签的 `[[tests]]`，可重复） |
| `ns-build install [--prefix P]` | 安装到前缀（默认 `/usr/local`；`[[exports]]` 的配置文件一并写入） |
| `ns-build run <目标> [-- 参数...]` | 构建并运行目标 |
| `ns-build show` | 列出目标 |
| `ns-build graph` | 打印依赖图 |
| `ns-build presets` | 列出 `[presets]` 中的预设 |
| `ns-build fetch [--update]` | 拉取 `[[fetch]]` 外部依赖到 `.ns-fetch/` |
| `ns-build init [--name N]` | 生成初始 `ns-build.toml` |
| `ns-build --version` / `-V` | 版本 |
| `ns-build --help` / `-h` | 帮助 |

### build 选项

| 选项 | 说明 |
|------|------|
| `-j N` / `--jobs N` / `--parallel N` | 并行任务数（默认 CPU 核数） |
| `--release` / `--debug` / `--profile <配置>` | 构建配置（`release`/`debug`/`relwithdebinfo`/`minsizerel`；未指定时用脚本 `defaults.profile` 或 `release`） |
| `--preset <名字>` | 应用 `[presets]` 中的预设（命令行显式选项优先） |
| `--define name=value` | 覆盖 `[defaults].variables` 中的变量（支持分离与 `=` 两种形式） |
| `--verbose` / `-v` | 打印完整命令 |
| `--quiet` / `-q` | 只输出错误 |
| `<目标>...` | 只构建指定目标（含传递依赖） |

## 构建脚本（`ns-build.toml`）

TOML 声明式。顶层节：

- `[project]`：`name`、`version`、`root`（源码根，相对脚本；默认 `.`）、`out`（产物目录，默认 `build`）
- `[defaults]`：全局编译设置（见下）
- `[[targets]]`：目标定义
- `[[toolchains]]`：交叉编译工具链
- `[[exports]]`：安装导出（`install(EXPORT)`）
- `[[imports]]`：导入已安装的包（`find_package`）
- `[[custom_commands]]`：生成代码（`add_custom_command(OUTPUT)`）
- `[[fetch]]`：外部依赖拉取（`FetchContent`）＋ `ns-build fetch`
- `[[install_headers]]`：头文件安装规则
- `[[install_files]]`：通用文件/目录安装规则
- `[[tests]]`：显式测试注册
- `[presets]`：命名构建预设（`--preset`）
- `includes = [...]`：子脚本（目录或 `ns-build.toml` 文件路径）

### `[defaults]`

| 字段 | 说明 | 默认 |
|------|------|------|
| `cc` / `cxx` / `ar` | 编译器 | `cc` / `c++` / `ar` |
| `flags` / `cflags` / `cxxflags` | 编译标志 | `[]` |
| `link_flags` | 链接标志 | `[]` |
| `include_dirs` | 头文件目录 | `[]` |
| `defines` | 宏定义 | `[]` |
| `profile` | `release` / `debug` / `relwithdebinfo` / `minsizerel` | 未设置（等价 `release`） |
| `parallel` | 并行度 | CPU 核数 |
| `unity` | `off` / `on` / `auto`（`auto` = ≥4 源自动分组） | `auto` |
| `toolchain` | 默认使用的 `[[toolchains]]` 名 | 自动按 os/arch 匹配 |
| `strip` | 剥离符号 | `false` |
| `warnings` | 警告标志（`-Wall -Wextra`） | `true` |
| `cache` | 缓存目录（相对项目） | `.ns-cache` |
| `variables` | 变量表（`${name}` 替换） | `{}` |

### `[[targets]]`

| 字段 | 说明 |
|------|------|
| `name` | 目标名（必填） |
| `kind` | `exe` / `lib`（动态库）/ `module`（dlopen 模块）/ `static`（.a）/ `object` / `interface`（头文件库）/ `phony`（聚合） |
| `language` | `c` / `c++`（`cpp` 亦可）/ `rust` / `go` / `java` / `zig` / `python` / `shell`。未指定时按源文件后缀推断 C/C++ |
| `cc` | 编译器覆盖（覆盖默认与工具链） |
| `toolchain` | 指定 `[[toolchains]]` 名（覆盖 `defaults.toolchain`）；决定目标平台 os/arch |
| `sources` | 源文件 / glob（`["src/**/*.c"]`，相对 `project.root`） |
| `include_dirs` | 头文件目录 |
| `flags` / `cflags` / `cxxflags` | 编译标志 |
| `link` | 链接库（`-lfoo` 式库名，或路径） |
| `link_dirs` | 库搜索目录 |
| `link_flags` | 链接标志 |
| `defines` | 宏定义（`-D`） |
| `deps` | 依赖目标：object 库对象并入；静态/动态库产物直接链接（静态链式依赖递归收集）；include/defines 等使用要求传递传播 |
| `output` | 产物名（相对 out）；默认按 target 名 + 平台后缀（见下） |
| `groups` / `group_size` | 分步编译：分组数 / 每组文件数（二选一） |
| `parallel` | 是否并行（默认 `true`） |
| `hooks.before` / `hooks.after` | 构建钩子（字符串 `"sh -c '...'"` 或 `{ program, args }`） |
| `os` / `arch` | 平台过滤（按**目标平台**过滤，`["linux", "macos"]`） |
| `profile` | 目标级配置覆盖 |
| `strip` | 目标级剥离 |
| `install` | 是否安装（默认 `true`） |
| `when` | 条件表达式（`os == "linux" && arch == "x86_64"`，支持 `!`/括号） |
| `c_std` / `cxx_std` | 编译标准（`c11` / `gnu11` / `c++17` / `c++20`） |
| `pkg_config` | pkg-config 包名（编译展开 `--cflags`，链接展开 `--libs`） |
| `pch` | 预编译头（`-include`） |

### `[[toolchains]]`

```toml
[[toolchains]]
name = "mingw"
cc = "x86_64-w64-mingw32-gcc"
cxx = "x86_64-w64-mingw32-g++"
ar = "x86_64-w64-mingw32-ar"
os = "windows"      # 可选：声明目标平台 os（过滤与 ${os} 替换用）
arch = "x86_64"     # 可选：声明目标架构
flags = ["-static"]
include_dirs = ["/usr/x86_64-w64-mingw32/include"]
link_flags = ["-static"]
sysroot = "/usr/x86_64-w64-mingw32"
```

选择方式（按优先级）：
1. target 级 `toolchain = "mingw"`
2. `[defaults] toolchain = "mingw"`
3. 按 `os`/`arch` 与主机自动匹配（未显式选择时，第一个 os/arch 都匹配的工具链生效）

工具链决定目标平台：交叉编译时 `os`/`arch` 过滤、`when` 表达式与 `${os}`/`${arch}` 替换都基于工具链声明的平台，而非主机。

**MSVC 风格工具链**：`cc`/`cxx` 为 `cl` / `clang-cl`（或 `ar`/`link`/`lib`）时，命令自动生成 MSVC 风格参数（`/nologo /c /O2 /Fo<obj> /D /I /FI` 编译，`link /OUT: /DLL /DEBUG` 链接，`lib /OUT:` 归档，`foo` → `foo.lib`）。差异与限制：

- **不生成依赖文件**：MSVC 无 `-MMD` 等价物（`/sourceDependencies` 是 JSON），增量检测仅基于源文件内容与命令指纹，**改头文件不会触发重编**。
- `pkg_config` 是 POSIX 工具，MSVC 风格下被忽略。
- `strip` 仅 GNU 风格；`rpath` 在 Windows 本来就不生成。
- 未在真实 MSVC SDK 上本机验证（本开发环境为 macOS）；命令生成已有单元测试覆盖。
- unity build、/FI 强制包含（`pch`）同样支持。

### `[[install_headers]]`

```toml
[[install_headers]]
from = "include"
to = "include"              # prefix 下相对路径
files = ["**/*.h"]
```

### `[[tests]]`

```toml
[[tests]]
name = "unit"
target = "mytest"           # 目标名（exe）
args = ["--self-test"]      # 可选
workdir = "test"            # 可选工作目录
timeout = 30                # 可选：超时秒数
labels = ["smoke"]          # 可选：标签；`ns-build test --label smoke` 只跑它
require_success = false     # 可选：退出码非 0 也算通过（默认 true）
```

### 变量替换

`${name}` 在脚本字符串中替换。内置：`os`、`arch`、`project`、`out`。可经 `[defaults].variables` 或 `--define name=value` 提供。

```toml
[defaults.variables]
APP_VERSION = "1.2.3"

[[targets]]
name = "app"
defines = ["VERSION=\"${APP_VERSION}\""]
```

### 条件构建

```toml
[[targets]]
name = "linux-only"
when = 'os == "linux" && arch == "x86_64"'
```

表达式支持 `==`、`!=`、`&&`、`||`、`!`、括号分组，操作数为字符串/布尔/数字/变量。

### 子脚本

```toml
# 根 ns-build.toml
includes = ["sub"]      # sub/ns-build.toml，或直接 "sub/ns-build.toml"
```

### 依赖使用要求的传播

依赖目标（`deps`）声明的 `include_dirs`、`defines`、`link`、`link_dirs`、`link_flags`、`pkg_config` 会**传递性传播**给依赖者（可执行文件与别名除外——它们不可链接）。`INTERFACE` 头文件库只传递 `include_dirs`/`defines`，自身无产物。链接时：

- `object` 依赖 → 对象并入链接输入（静态库归档也是）
- `static`/`lib`/`module` 依赖 → 其产物直接链接；静态库链式依赖会递归收集，无需在最终 target 重复声明
- 链接构建树内的动态库时会自动加 `-Wl,-rpath`，未安装也可直接运行

### unity build

`unity = "auto"`（默认）：源文件数 ≥4 时，按约每 4 个文件一组、最多为并行度 N 组合并编译（不足 2 组则退回逐文件）。`groups = N` 固定分 N 组；`group_size = N` 每组 N 个；二者与 `auto` 互斥。`off` 关闭合并。

### `[[custom_commands]]`

生成代码并参与构建（`add_custom_command(OUTPUT)` 等价）。产物可作为 target 的 `sources` 使用，按依赖关系在编译前自动生成；产物变化会触发下游重编译。

```toml
[[custom_commands]]
name = "version"                    # 可选；缺省 custom_0, custom_1...
outputs = ["build/gen/version.c"]   # 生成的文件（相对脚本目录）
command = { program = "sh", args = ["-c", "printf 'int version(void){return 42;}\n' > build/gen/version.c"] }
# 或者简单字符串（sh -c / cmd /C）：command = "echo 'int version(void){return 42;}' > build/gen/version.c"
depends = ["template.txt"]          # 输入（相对脚本目录，支持 glob）
deps = ["other-gen"]                # 自定义命令之间的先后关系（按 name）

[[targets]]
name = "app"
kind = "exe"
sources = ["src/main.c", "build/gen/version.c"]
```

- 多产物 `outputs` 均会登记为下游可引用的路径；产物路径在脚本解析期只检查重复声明，不要求已存在（构建时才生成）。
- `depends` 中文件必须是**已存在**的输入（或另一个自定义命令的产物），否则报错。
- 命令变量替换支持 `${var}`，与其他脚本字符串一致。
- 自定义命令节点参与增量缓存（依赖内容变化才重跑），且会让所有依赖它的编译/链接任务缓存失效。

### `[presets]`

命名配置覆盖（`cmake --preset` 等价）；命令行显式选项优先于预设：

```toml
[presets]
fast = { profile = "debug", defines = { DIST_VALUE = "11" }, unity = "off" }
dist = { profile = "release", jobs = 8, toolchain = "mingw", defines = { DIST = "1" } }
```

| 预设字段 | 对应 CLI |
|----------|----------|
| `profile`（`release`/`debug`/`relwithdebinfo`/`minsizerel`） | `--profile` |
| `jobs` | `-j` |
| `defines`（`${var}` 替换表） | `--define name=value` |
| `unity`（`off`/`on`/`auto`） | `[defaults].unity` |
| `toolchain` | `[defaults].toolchain` |
| `warnings` | `[defaults].warnings` |

```bash
ns-build presets               # 列出预设
ns-build build --preset fast   # 应用预设
ns-build build --preset fast --define DIST_VALUE=3   # CLI 覆盖预设同名变量
```

### `[[exports]]` 与 `[[imports]]`

导出已构建的库（CMake `install(EXPORT)` 等价）：`ns-build install` 额外写入
`<prefix>/lib/cmake/<name>/<name>-config.toml`（TOML 格式），包含产物、defines、链接库名与 include 信息。

```toml
[[exports]]
name = "mylib"                 # 导出包名
targets = ["core", "util"]      # 空 = 所有 install 的 static/lib/module 目标
to = "lib/cmake/mylib"         # 相对 prefix（默认 lib/cmake/<name>）
version = "1.0.0"               # 默认 project.version
```

下游项目导入该包（`find_package` 等价）——`include_dirs` 并入所有目标，产物以绝对路径参与链接：

```toml
[[imports]]
from = "/usr/local"            # 前缀目录 / 配置文件路径 / 含 <name>-config.toml 的目录
name = "mylib"
```

### `[[fetch]]`

外部依赖拉取（`FetchContent` 等价）。构建本身不需要网络；CI 中显式执行：

```toml
[[fetch]]
name = "fmt"                       # .ns-fetch/<name>/
git = "https://github.com/fmtlib/fmt.git"
rev = "10.2.1"                     # tag / 分支 / 提交（默认远程 HEAD）

# 或下载压缩包（.tar.gz/.tgz/.tar/.zip）：
[[fetch]]
name = "zlib"
url = "https://zlib.net/zlib-1.3.1.tar.gz"
strip_components = 1               # 解压去掉的前缀层数（仅 tar）
```

```bash
ns-build fetch            # 拉取（已存在则跳过）
ns-build fetch --update   # 强制重新拉取
```

拉取到项目根下 `.ns-fetch/<name>/`，并附 `.ns-fetch.toml` 清单。`clean` 不删除（同 git submodule，属源码而非产物）。

## 完整示例

```toml
[project]
name = "demo"
version = "0.1.0"

[defaults]
cc = "cc"
parallel = 8
unity = "auto"
variables = { VER = "0.1.0" }

# 分步：300 个源文件 → 10 组对象
[[targets]]
name = "objs"
kind = "object"
sources = ["src/**/*.c"]
groups = 10
defines = ["VER=\"${VER}\""]

# 可执行：链接对象 + 系统库
[[targets]]
name = "demo"
kind = "exe"
sources = ["src/main.c"]
deps = ["objs"]
link = ["m"]
c_std = "c11"
pkg_config = ["sdl2"]

# 子目录库
[[targets]]
name = "util"
kind = "static"
sources = ["util/*.c"]

# 测试
[[tests]]
name = "unit"
target = "demo"
args = ["--test"]

[[install_headers]]
from = "include"
to = "include"
files = ["**/*.h"]

[[exports]]
name = "demo"          # 安装时写入 lib/cmake/demo/demo-config.toml

[presets]
dist = { profile = "release", jobs = 8 }
```

## 产物路径

未显式 `output` 时，按 target 名 + 平台命名：

- `exe` → `build/<name>`（Windows 为 `<name>.exe`；Python/Shell/Java 不加后缀）
- `static` → `build/lib<name>.a`
- `lib` / `module` → `build/lib<name>.so`（Linux）/ `lib<name>.dylib`（macOS）/ `<name>.dll`（Windows）
- `object` → `build/obj/*.o`（unity 合并源文件生成在 `build/unity/<target>_<i>.c|.cpp`，对象仍输出到 `build/obj/`）
- Java 类文件 → `build/classes/`

显式 `output = "custom"` 时完全按用户写法，不追加任何后缀。

## 编译命令

构建时生成 `build/compile_commands.json`（CMake 兼容，供 clangd / IDE 使用）。
