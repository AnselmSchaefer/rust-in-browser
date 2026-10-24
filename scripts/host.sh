# The machine the toolchain is built on (sourced by the build scripts): Linux x86_64 or macOS arm64.
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)
    BUILD_TRIPLE=x86_64-unknown-linux-gnu
    WASI_SDK_NAME=wasi-sdk-34.0-x86_64-linux
    ;;
  Darwin-arm64)
    BUILD_TRIPLE=aarch64-apple-darwin
    WASI_SDK_NAME=wasi-sdk-34.0-arm64-macos
    ;;
  *)
    echo "unsupported build machine: $(uname -s) $(uname -m)" >&2
    exit 1
    ;;
esac
cpu_count() { nproc 2>/dev/null || sysctl -n hw.ncpu; }
sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi; }
