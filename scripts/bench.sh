#!/usr/bin/env bash
# 基准对比：ns-build (unity auto) vs ns-build (unity off) vs CMake + Ninja
# 用法: scripts/bench.sh   环境变量: NS_BENCH_N (源文件数) · NS_BENCH_KEEP=1 (保留临时目录)
set -u -o pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
NS="${NS_BUILD_BIN:-$ROOT/target/release/ns-build}"
N="${NS_BENCH_N:-300}"
JOBS="$(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4)"
BASE="$(mktemp -d "${TMPDIR:-/tmp}/nsbuild-bench-XXXX")"

if [ ! -x "$NS" ]; then
    echo "ns-build release binary not found: $NS" >&2
    echo "run: cargo build --release" >&2
    exit 1
fi
if ! command -v cmake >/dev/null || ! command -v ninja >/dev/null; then
    echo "cmake/ninja not available; CMAKE_AVAILABLE=0" >&2
fi
if [ "${NS_BENCH_KEEP:-0}" = "1" ]; then
    trap 'echo "[bench] kept: $BASE"' EXIT
else
    trap 'rm -rf "$BASE"' EXIT
fi

echo "[bench] workdir=$BASE n=$N jobs=$JOBS ns-build=$NS"

# --- 生成 N 个 C 源文件（f0.c 含 main）---
mkdir -p "$BASE/src"
for i in $(seq 0 $((N - 1))); do
    if [ "$i" = "0" ]; then
        cat > "$BASE/src/f0.c" <<'EOF'
#include <stdio.h>
int f1(void); int f2(void); int f3(void);
int main(void){ printf("%d\n", f1()+f2()+f3()); return 0; }
EOF
    else
        printf 'int f%s(void) { return %s; }\n' "$i" "$i" > "$BASE/src/f${i}.c"
    fi
done

nsmb_script() {  # $1 = auto|off
    cat > "$2" <<EOF
[project]
name = "bench"

[defaults]
cc = "cc"
unity = "$1"

[[targets]]
name = "bench"
kind = "exe"
sources = ["src/*.c"]
EOF
}

# 高精度计时（秒，3 位小数）：$1 = 工作目录 $2.. = 命令
timeit() {
    local cwd="$1"; shift
    python3 - "$cwd" "$@" <<'PY'
import subprocess, sys, time
cwd, cmd = sys.argv[1], sys.argv[2:]
t0 = time.perf_counter()
r = subprocess.run(cmd, cwd=cwd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
print(f"{time.perf_counter()-t0:.3f} {r.returncode}")
PY
}

run_nsbuild() {  # $1 = mode(auto|off)
    local mode="$1" d="$BASE/ns-$1"
    mkdir -p "$d"
    cp -R "$BASE/src" "$d/src"
    nsmb_script "$mode" "$d/ns-build.toml"
    local t; t=$(timeit "$d" "$NS" build -j "$JOBS")
    printf 'ns-build (%-4s) full        : %s\n' "$mode" "$t"
    local t2; t2=$(timeit "$d" "$NS" build -j "$JOBS")
    printf 'ns-build (%-4s) incremental : %s\n' "$mode" "$t2"
    printf '// touch\n' >> "$d/src/f5.c"
    local t3; t3=$(timeit "$d" "$NS" build -j "$JOBS")
    printf 'ns-build (%-4s) 1-file change: %s\n' "$mode" "$t3"
}

run_cmake() {
    local d="$BASE/cmake"
    mkdir -p "$d"
    cp -R "$BASE/src" "$d/src"
    cat > "$d/CMakeLists.txt" <<'EOF'
cmake_minimum_required(VERSION 3.16)
project(bench C)
file(GLOB SRCS "src/*.c")
add_executable(bench ${SRCS})
EOF
    (cd "$d" && cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release >/dev/null 2>&1)
    local t; t=$(timeit "$d" cmake --build build --parallel "$JOBS")
    printf 'cmake + ninja   full        : %s\n' "$t"
    local t2; t2=$(timeit "$d" cmake --build build --parallel "$JOBS")
    printf 'cmake + ninja   incremental : %s\n' "$t2"
    printf '// touch\n' >> "$d/src/f5.c"
    local t3; t3=$(timeit "$d" cmake --build build --parallel "$JOBS")
    printf 'cmake + ninja   1-file change: %s\n' "$t3"
}

run_nsbuild auto
run_nsbuild off
command -v cmake >/dev/null && command -v ninja >/dev/null && run_cmake || true
