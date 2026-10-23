#!/usr/bin/env bash
# Builds rustc's LLVM (with lld) for wasm32-wasip1-threads, so that rustc itself can be built for it.
#
#  1. a native build of the same source and options, for llvm-tblgen and llvm-config;
#  2. the cross build with wasi-sdk, libraries only (rustc links them statically);
#  3. an llvm-config that answers for the cross build (bootstrap can't run a wasm llvm-config).
#
# The LLVM source is rustc's own (src/llvm-project of the rustc source), with YoWASP's WASI patch applied.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
RUST_SRC=${RUST_SRC:-$ROOT/build/src/rustc-1.98.0-src}
LLVM_SRC=$RUST_SRC/src/llvm-project
WASI_SDK=${WASI_SDK:-$ROOT/build/wasi-sdk-34.0-x86_64-linux}
OUT=$ROOT/build/llvm
TARGET=wasm32-wasip1-threads
JOBS=${JOBS:-$(nproc)}
export PATH=$ROOT/.venv/bin:$PATH

# Options both builds share, so that the native llvm-config describes the cross build's libraries.
COMMON=(
  -G Ninja
  -DCMAKE_BUILD_TYPE=Release
  -DLLVM_TARGETS_TO_BUILD=WebAssembly
  -DLLVM_EXPERIMENTAL_TARGETS_TO_BUILD=
  -DLLVM_ENABLE_PROJECTS=lld
  -DLLVM_ENABLE_ASSERTIONS=OFF
  -DLLVM_ENABLE_ZLIB=OFF
  -DLLVM_ENABLE_ZSTD=OFF
  -DLLVM_ENABLE_LIBXML2=OFF
  -DLLVM_ENABLE_LIBEDIT=OFF
  -DLLVM_ENABLE_TERMINFO=OFF
  -DLLVM_ENABLE_LIBPFM=OFF
  -DLLVM_ENABLE_Z3_SOLVER=OFF
  -DLLVM_ENABLE_CURL=OFF
  -DLLVM_ENABLE_HTTPLIB=OFF
  -DLLVM_ENABLE_BINDINGS=OFF
  -DLLVM_ENABLE_PLUGINS=OFF
  -DLLVM_INCLUDE_EXAMPLES=OFF
  -DLLVM_INCLUDE_TESTS=OFF
  -DLLVM_INCLUDE_BENCHMARKS=OFF
  -DLLVM_INCLUDE_DOCS=OFF
  -DLLVM_INCLUDE_UTILS=OFF
  -DLLVM_BUILD_UTILS=OFF
  -DLLVM_INCLUDE_RUNTIMES=OFF
  -DLLVM_BUILD_RUNTIME=OFF
  -DLLVM_ENABLE_RUNTIMES=
  -DLLVM_BUILD_LLVM_DYLIB=OFF
  -DLLVM_LINK_LLVM_DYLIB=OFF
  -DLLVM_DEFAULT_TARGET_TRIPLE=wasm32-unknown-unknown
)

step() { echo; echo "=== $* ($(date +%H:%M:%S))"; }

# 1. Native tools.
if ! [ -x "$OUT/native/bin/llvm-config" ]; then
  step "native tblgen and llvm-config"
  cmake -S "$LLVM_SRC/llvm" -B "$OUT/native" "${COMMON[@]}" \
    -DCMAKE_C_COMPILER=clang -DCMAKE_CXX_COMPILER=clang++ \
    -DLLVM_BUILD_TOOLS=OFF
  ninja -C "$OUT/native" -j "$JOBS" llvm-tblgen llvm-min-tblgen llvm-config lld-tblgen 2>/dev/null \
    || ninja -C "$OUT/native" -j "$JOBS" llvm-tblgen llvm-min-tblgen llvm-config
fi

# 2. The cross build.
cat > "$OUT/Toolchain-WASI.cmake" <<END
set(CMAKE_SYSTEM_NAME WASI)
set(CMAKE_SYSTEM_VERSION 1)
set(CMAKE_SYSTEM_PROCESSOR wasm32)
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)
set(CMAKE_C_COMPILER $WASI_SDK/bin/clang)
set(CMAKE_C_COMPILER_TARGET $TARGET)
set(CMAKE_CXX_COMPILER $WASI_SDK/bin/clang++)
set(CMAKE_CXX_COMPILER_TARGET $TARGET)
set(CMAKE_LINKER $WASI_SDK/bin/wasm-ld)
set(CMAKE_AR $WASI_SDK/bin/ar)
set(CMAKE_RANLIB $WASI_SDK/bin/ranlib)
set(CMAKE_SYSROOT $WASI_SDK/share/wasi-sysroot)
# LLVM has some calls to mmap that are unreachable in this configuration.
set(CMAKE_C_FLAGS "-pthread -D_WASI_EMULATED_MMAN -D_WASI_EMULATED_SIGNAL -D_WASI_EMULATED_PROCESS_CLOCKS")
set(CMAKE_CXX_FLAGS "-pthread -D_WASI_EMULATED_MMAN -D_WASI_EMULATED_SIGNAL -D_WASI_EMULATED_PROCESS_CLOCKS")
set(CMAKE_EXE_LINKER_FLAGS "-pthread -lwasi-emulated-mman -lwasi-emulated-signal -lwasi-emulated-process-clocks")
END

step "LLVM and lld for $TARGET"
cmake -S "$LLVM_SRC/llvm" -B "$OUT/wasm" "${COMMON[@]}" \
  -DCMAKE_TOOLCHAIN_FILE="$OUT/Toolchain-WASI.cmake" \
  -DLLVM_NATIVE_TOOL_DIR="$OUT/native/bin" \
  -DLLVM_TABLEGEN="$OUT/native/bin/llvm-tblgen" \
  -DLLVM_HOST_TRIPLE=$TARGET \
  -DLLVM_ENABLE_THREADS=ON \
  -DLLVM_ENABLE_PIC=OFF \
  -DLLVM_BUILD_STATIC=ON \
  -DLLVM_BUILD_SHARED_LIBS=OFF \
  -DLLVM_BUILD_TOOLS=OFF \
  -DLLD_BUILD_TOOLS=OFF \
  -DLLVM_INSTALL_UTILS=OFF \
  -DCMAKE_INSTALL_PREFIX="$OUT/wasm-prefix" \
  -DHAVE_DLOPEN=

# All libraries (tools are off), installed with their headers into a prefix.
ninja -C "$OUT/wasm" -j "$JOBS" install
cp -r "$LLVM_SRC/lld/include/lld" "$OUT/wasm-prefix/include/"

# 3. llvm-config for the cross build: the native one, copied into the prefix. Outside its build tree
# llvm-config answers relative to its own location, so it reports the prefix's wasm libraries.
step "llvm-config"
mkdir -p "$OUT/wasm-prefix/bin"
cp "$OUT/native/bin/llvm-config" "$OUT/wasm-prefix/bin/llvm-config"
"$OUT/wasm-prefix/bin/llvm-config" --version
"$OUT/wasm-prefix/bin/llvm-config" --link-static --libnames | wc -w
step done
