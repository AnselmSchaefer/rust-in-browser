# Third-party notices

This repository's own code (the IDE, the runtime, the bridge, the scripts and tools, and Caprice in
`caprice-web/`) is under the MIT license (`LICENSE`). These parts come from others and keep their licenses:

| Part | Source | License | License text |
|---|---|---|---|
| `web/vendor/monaco/` | [Monaco Editor](https://github.com/microsoft/monaco-editor) 0.52.2, unmodified | MIT; bundled components as listed in its notices | `web/vendor/monaco/LICENSE`, `web/vendor/monaco/ThirdPartyNotices.txt` |
| `patches/llvm-wasi.patch` | Changes to LLVM 22: [YoWASP's WASI patch](https://github.com/YoWASP/llvm-project) (commit 97196c8), and two fixes | Apache-2.0 WITH LLVM-exception (LLVM's license) | `licenses/LLVM-LICENSE.txt` |
| `patches/rustc-wasm-host.patch` | Changes to [rustc](https://github.com/rust-lang/rust) 1.98.0, partly ported from [oligamiq/rust](https://github.com/oligamiq/rust) (work by bjorn3 and oligamiq); used under the MIT option of rustc's MIT OR Apache-2.0 | MIT | `licenses/RUST-LICENSE-MIT.txt` |
| `compiler/rustc_static_proc_macros/serde_derive/` inside `patches/rustc-wasm-host.patch` | [serde_derive](https://github.com/serde-rs/serde) 1.0.229, two attributes removed; used under the MIT option of MIT OR Apache-2.0 | MIT | `LICENSE-MIT` inside the patch |
| `patches/bootstrap.toml` | Configuration for rustc's bootstrap | MIT (like rustc) | `licenses/RUST-LICENSE-MIT.txt` |

The build downloads further software that is not part of this repository: the rustc source, wasi-sdk,
Caprice's crate dependencies (from crates.io) and the official Rust standard library. The toolchain that
`tools/build_assets.py` assembles (`web/toolchain/`, not committed) contains compiled code of all of them;
anyone distributing it must include their licenses too.
