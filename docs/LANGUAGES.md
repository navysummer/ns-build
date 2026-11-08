# ns-build 多语言指南

每种语言一个最小可构建示例。所有示例共用 `ns-build.toml`，用 `language` 字段区分。

## C

```toml
[project]
name = "hello"

[[targets]]
name = "hello"
kind = "exe"            # 默认 language = "c"
sources = ["src/main.c"]
```

```c
// src/main.c
#include <stdio.h>
int main(void) { printf("hello\n"); return 0; }
```

```bash
ns-build build && ./build/hello
```

## C++

```toml
[[targets]]
name = "app"
kind = "exe"
language = "c++"       # 显式指定；省略时按源文件后缀（.cpp/.cc/...）自动推断
sources = ["src/main.cpp"]
cxx_std = "c++20"        # 默认 c++17
```

```cpp
// src/main.cpp
#include <iostream>
int main() { std::cout << "hi\n"; }
```

## Rust

```toml
[[targets]]
name = "app"
kind = "exe"
language = "rust"
sources = ["src/main.rs"]
```

```rust
// src/main.rs
fn main() { println!("hi"); }
```

## Go

```toml
[[targets]]
name = "app"
kind = "exe"
language = "go"
sources = ["src/main.go"]
```

```go
// src/main.go
package main
import "fmt"
func main() { fmt.Println("hi") }
```

## Java

```toml
[[targets]]
name = "app"
kind = "exe"
language = "java"
sources = ["src/Main.java"]
```

```java
// src/Main.java
public class Main {
    public static void main(String[] a) {
        System.out.println("hi");
    }
}
```

编译产物在 `build/classes/`，运行：

```bash
java -cp build/classes Main
```

## Zig

```toml
[[targets]]
name = "app"
kind = "exe"
language = "zig"
sources = ["src/main.zig"]
```

```zig
// src/main.zig
const std = @import("std");
pub fn main() !void {
    std.debug.print("hi\n", .{});
}
```

## Python

```toml
[[targets]]
name = "app"
kind = "exe"
language = "python"
sources = ["src/main.py"]
```

```python
# src/main.py
#!/usr/bin/env python3
print("hi")
```

产物拷贝并加可执行位：`./build/app`。

## Shell

```toml
[[targets]]
name = "app"
kind = "exe"
language = "shell"
sources = ["src/main.sh"]
```

```sh
# src/main.sh
#!/bin/sh
echo hi
```

## 分步 + 并行（多文件 C）

把 N 个 `.c` 编译成多组 `.o`，再链接成可执行：

```toml
[project]
name = "multi"

[[targets]]
name = "objs"
kind = "object"
sources = ["src/**/*.c"]
group_size = 4          # 每 4 个文件一组，并行编译

[[targets]]
name = "app"
kind = "exe"
sources = ["src/main.c"]
deps = ["objs"]         # 链接所有 .o
```

`group_size = 4` 表示每组 4 个源文件编译为 1 个 `.o`；`groups = 10` 表示固定分 10 组。两者不并存。

## 平台过滤

```toml
[[targets]]
name = "win-only"
os = ["windows"]        # 仅 Windows 构建
arch = ["x86_64"]
```

## 交叉编译

```toml
[[toolchains]]
name = "mingw"
cc = "x86_64-w64-mingw32-gcc"
cxx = "x86_64-w64-mingw32-g++"
ar = "x86_64-w64-mingw32-ar"
os = "windows"
arch = "x86_64"
sysroot = "/usr/x86_64-w64-mingw32"

[[targets]]
name = "app"
kind = "exe"
toolchain = "mingw"   # 指定工具链；交叉编译的目标 os/arch/${os} 以工具链为准
sources = ["src/main.c"]
```

不指定 `toolchain` 时按 `os`/`arch` 自动匹配（第一个都匹配的工具链生效）。全局默认可用 `[defaults] toolchain = "mingw"`。

`cc` 为 `cl` / `clang-cl`（或 `ar = "lib"` / `link`）时自动使用 MSVC 风格命令行生成（`/nologo /Fo /OUT:` 等），裸库名映射为 `foo.lib`。注意：MSVC 风格不生成依赖文件（改头文件不触发重编），`pkg_config` 被忽略，且未在真实 MSVC SDK 上本机验证。
