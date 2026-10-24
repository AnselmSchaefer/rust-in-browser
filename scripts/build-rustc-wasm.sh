#!/usr/bin/env bash
# Builds rustc 1.98.0 for wasm32-wasip1-threads (to run in the browser) into build/rustc.wasm.
#
#  1. the official source tarball, with patches/llvm-wasi.patch (YoWASP's WASI patch for LLVM 22, plus
#     two fixes for rustc's LLVM) and patches/rustc-wasm-host.patch (in-process lld, proc macros linked
#     in, no dlopen, no file locks, no pids; see the patch);
#  2. LLVM and lld for wasm32-wasip1-threads (scripts/build-llvm-wasm.sh);
#  3. bootstrap, with the official 1.98.0 (rustup toolchain `1.98.0`) as stage0.
#
# Needs: rustup, clang, python3; wasi-sdk 34 is downloaded. Takes about an hour on 12 cores.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
. "$ROOT/scripts/host.sh"
VERSION=1.98.0
SRC=$ROOT/build/src/rustc-$VERSION-src
WASI_SDK=$ROOT/build/$WASI_SDK_NAME
export PATH=$ROOT/.venv/bin:$PATH

step() { echo; echo "=== $* ($(date +%H:%M:%S))"; }

[ -d "$ROOT/.venv" ] || { python3 -m venv "$ROOT/.venv"; "$ROOT/.venv/bin/pip" install -q cmake ninja; }
rustup toolchain install $VERSION --profile minimal -t wasm32-wasip1-threads -t wasm32-unknown-unknown

if ! [ -d "$WASI_SDK" ]; then
  step "wasi-sdk 34"
  mkdir -p "$ROOT/build"
  curl -sSfL https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-34/$WASI_SDK_NAME.tar.gz \
    | tar xz -C "$ROOT/build"
fi

if ! [ -d "$SRC" ]; then
  step "rustc $VERSION source"
  # Downloaded to a file, resumed when the connection drops, and checked against the published hash.
  URL=https://static.rust-lang.org/dist/rustc-$VERSION-src.tar.xz
  TARBALL=$ROOT/build/rustc-$VERSION-src.tar.xz
  mkdir -p "$ROOT/build/src"
  EXPECTED=$(curl -sSfL --retry 5 --retry-all-errors "$URL.sha256" | cut -d' ' -f1)
  checked() { [ -f "$TARBALL" ] && [ "$(sha256 "$TARBALL" | cut -d' ' -f1)" = "$EXPECTED" ]; }
  for attempt in $(seq 20); do
    checked && break
    if curl -sSfL -C - --retry 3 --retry-all-errors --speed-limit 1024 --speed-time 30 -o "$TARBALL" "$URL"; then
      checked || rm -f "$TARBALL"
    else
      echo "download interrupted (attempt $attempt), resuming"
      sleep 10
    fi
  done
  checked || { echo "rustc-$VERSION-src.tar.xz: download failed or checksum mismatch" >&2; exit 1; }
  # Unpacked and patched in a staging directory, moved into place only when complete: an interrupted
  # run never leaves a $SRC that looks complete but is unpatched.
  WORK=$ROOT/build/src/partial
  rm -rf "$WORK"
  mkdir -p "$WORK"
  tar xJf "$TARBALL" -C "$WORK"
  PART=$WORK/rustc-$VERSION-src
  (cd "$PART/src/llvm-project" && patch -p1 < "$ROOT/patches/llvm-wasi.patch")
  (cd "$PART" && patch -p1 < "$ROOT/patches/rustc-wasm-host.patch")
  # syn 3 (for serde_derive, linked into rustc) isn't among rustc's vendored crates.
  CRATE=$(mktemp)
  curl -sSfL --retry 5 --retry-all-errors https://static.crates.io/crates/syn/syn-3.0.6.crate -o "$CRATE"
  tar xzf "$CRATE" -C "$PART/vendor"
  printf '{"files":{},"package":"%s"}' "$(sha256 "$CRATE" | cut -d' ' -f1)" > "$PART/vendor/syn-3.0.6/.cargo-checksum.json"
  rm "$CRATE"
  sed "s|@ROOT@|$ROOT|g; s|@HOME@|$HOME|g; s|@BUILD@|$BUILD_TRIPLE|g; s|@WASI_SDK@|$WASI_SDK_NAME|g" \
    "$ROOT/patches/bootstrap.toml" > "$PART/bootstrap.toml"
  mv "$PART" "$SRC"
  rmdir "$WORK"
fi

[ -x "$ROOT/build/llvm/wasm-prefix/bin/llvm-config" ] || "$ROOT/scripts/build-llvm-wasm.sh"

step "rustc for wasm32-wasip1-threads"
(cd "$SRC" && ./x build --stage 1 compiler/rustc)
cp "$SRC/build/wasm32-wasip1-threads/stage1/bin/rustc.wasm" "$ROOT/build/rustc.wasm"
ls -la "$ROOT/build/rustc.wasm"
step done
