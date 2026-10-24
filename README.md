# Rust Browser IDE: Caprice, compiled in the tab

A browser IDE with Monaco 0.52.2 on the left and [Caprice](../caprice) running on the right. The whole
toolchain runs in the tab as WebAssembly: **rustc 1.98.0** compiles Caprice for `wasm32-unknown-unknown`,
**wasm-bindgen** makes its JS glue, and the result starts in the app pane. A **Python MCP bridge** lets
Claude Code on this machine read and edit the code, build it, look at the app and use it.

```
┌──────────────────────────── browser tab (cross-origin isolated) ─────────────────────────────┐
│  Monaco 0.52.2 · file tree · Output/Problems   │   Caprice (eframe, WebGL) in an iframe      │
│              │ Build & Run                     │          ▲ caprice.js + caprice_bg.wasm     │
│              ▼                                 │          │                                  │
│  rustc.wasm (1.98.0, LLVM 22, lld in-process) ──► wasm-bindgen.wasm                          │
│  one Web Worker per wasm thread, shared memory per process (≤ 4 GiB)                         │
│  file-system worker: /sysroot (official std) /deps (prebuilt rlibs) /project /out /incremental│
└───────────────────────────────▲──────────────────────────────────────────────────────────────┘
                                │ WebSocket
                 bridge/ide_bridge.py (serves the IDE, MCP over stdio) ◄── Claude Code
```

## Run it

```sh
python3 -m venv .venv && .venv/bin/pip install -r bridge/requirements.txt
.venv/bin/python bridge/ide_bridge.py --serve-only      # or let Claude Code start it, below
```

Open <http://127.0.0.1:8765/> in Chrome or another Chromium (the page needs `SharedArrayBuffer`, which
the bridge's headers allow). The IDE downloads the toolchain (about 180 MB, gzipped), then builds Caprice
and starts it. **Ctrl+Enter** builds and runs again; edits are kept in the browser (IndexedDB) across
reloads; *Reset project* goes back to Caprice as shipped.

Measured here (12 cores, headless Chromium): the toolchain loads in about 3 s from the local bridge; a
full build of Caprice (23,000 lines) takes **14–15 s in rustc** plus 3.4 s in wasm-bindgen; after an
edit, the incremental build takes **3.5 s in rustc** (about 7 s with wasm-bindgen). Native rustc needs
10.5 s for the same full build. Errors come back as rustc renders them, in Problems and as editor
markers.

### From Claude Code

`.mcp.json` in this directory registers the bridge, so `claude` started here offers it (approve
`rust-browser-ide`). Elsewhere: `claude mcp add rust-browser-ide -- /path/to/.venv/bin/python /path/to/bridge/ide_bridge.py`.
The bridge serves the IDE itself; if one already runs (`--serve-only`), it relays through that one.

Tools: `ide_status`, `list_files`, `read_file`, `write_file`, `edit_file`, `delete_file`, `search`,
`build`, `get_diagnostics`, `open_in_editor`, `app_screenshot`, `app_console`, `app_size`, `app_click`,
`app_drag`, `app_scroll`, `app_key`, `app_type`, `restart_app`, `reset_project`, `sync_to_disk`,
`load_from_disk`.

## Performance

Measured 2026-10-23 on an i5-11400H laptop (6 cores / 12 threads, 16 GB RAM), Chromium 155, fresh profile.

| | AC power | Battery |
|---|--:|--:|
| Toolchain load (local bridge) | 1.9 s | 3.0 s |
| Cold build: rustc + wasm-bindgen | **15.7 s** (12.6 + 3.1) | 23–25 s (18–21 + 4.4) |
| Rebuild after an edit: rustc + wasm-bindgen | **4.5–4.8 s** (~2.0 + 2.6) | 7.3–8.2 s (~3.4 + 4.1) |
| Build that fails with a type error | – | 1.4 s |
| First build after a page reload | – | 20.9 s (full, not incremental) |

- **Memory** (PSS of the whole Chromium process tree): ~1.4 GiB idle with the app running, **~3.0–3.2 GiB at
  the peak of a build**, and up to 3.5 GiB when builds run back-to-back, because a build's memory is freed only
  at the next GC. About 92 % of it is the one renderer process. The JS + wasm heap stays at ~740 MiB after each
  incremental build (no leak across 8 builds).
- **CPU:** rustc averages ~2 cores. It peaks near 8 cores only during a cold build's codegen; incremental builds
  use ~1.8 cores. The IDE costs ~0.5 cores at rest with the app running.
- **Findings:** wasm-bindgen now takes longer than rustc in an incremental build (it reprocesses the 27 MB debug
  wasm every time). What you edit barely matters: a leaf literal and a const used across the crate rebuild in
  the same time. A no-op rebuild still costs a full incremental build. A reload drops the in-memory incremental
  cache. One full build out of 16 failed without errors; it did not happen again in 12 further cold starts. Battery power makes
  everything ~1.5× slower.

## How it works

**rustc for WebAssembly.** rubrc's prebuilt rustc is 1.83; Caprice's eframe 0.36 needs Rust 1.95. So
rustc **1.98.0** is built from the official source for `wasm32-wasip1-threads`
(`scripts/build-rustc-wasm.sh`):

- LLVM 22.1.8 with [YoWASP's WASI patch](https://github.com/YoWASP/llvm-project) (`patches/llvm-wasi.patch`,
  two small fixes for rustc's LLVM), built with wasi-sdk 34 (`scripts/build-llvm-wasm.sh`);
- the rustc side ported from [oligamiq/rust](https://github.com/oligamiq/rust) (bjorn3's work) to 1.98
  (`patches/rustc-wasm-host.patch`): **lld runs in-process** (WASI can't spawn the linker),
  **serde_derive is linked into rustc** (WASI can't `dlopen` a proc macro: `rustc_static_proc_macros`
  builds the same client table the dylib would have; the native `.so` is still read for its metadata),
  no-op file locks so incremental builds work, no `process::id`, `/sysroot` as the default sysroot;
- built with the official 1.98.0 as stage0 (`local-rebuild`) and the tarball's version info, so it reports
  `1.98.0 (88d9e12ae 2026-08-18)`: it reads the **official std** for `wasm32-unknown-unknown` and the
  dependency rlibs that native cargo built — the browser compiles only Caprice itself.

**The host** (`web/runtime/`) follows the design measured in
[rubrc#9](https://github.com/oligamiq/rubrc/issues/9): every process gets its own shared
`WebAssembly.Memory` (up to the module's 4 GiB, so the fixed 256 MiB region of
[rubrc#8](https://github.com/oligamiq/rubrc/issues/8) doesn't exist here), each wasm thread is a Web
Worker, and file calls go to one file-system worker through a per-thread `SharedArrayBuffer` block and
`Atomics.wait`; that worker reads and writes the caller's memory directly. **A trap in any thread ends
the whole process with an error** (#8's second suggestion), instead of leaving the others waiting.

**Caprice on the web** (`caprice-web/`, a copy of your working tree): a `web.rs` that stands in for
threads (work runs at once), the clock, pids and file dialogs (none yet), and `caprice_start`, which the
page calls. The browser builds it as a `cdylib`.

## Rebuilding the toolchain

```sh
scripts/build-rustc-wasm.sh        # LLVM + rustc for wasm32-wasip1-threads → build/rustc.wasm (~1 h)
(cd tools/wasm-bindgen-wasi && RUSTFLAGS=-Clink-arg=--max-memory=4294967296 \
   cargo build --release --target wasm32-wasip1-threads)
.venv/bin/python tools/build_assets.py   # packs, programs, build.json → web/toolchain/
```

`tools/build_assets.py` runs cargo for Caprice (with the pinned `1.98.0` toolchain), packs the official
std, the dependencies it built and the project, and turns cargo's rustc command into `build.json`.

## Tests

`tools/test/` drives headless Chromium (`npm install` there; uses `/usr/bin/chromium`):
`node ide-build.mjs` loads the IDE and waits for its first build; `run-page.mjs` runs `web/test/*.html`
(`smoke.html`: the WASI host; `rustc.html?src=…&args=…`: rustc on a snippet); `mcp_e2e.py` and
`mcp_use_app.py` call the bridge like Claude Code does (edit, build, a deliberate error, screenshots,
clicking and typing in Caprice).

## Limits

- Dependencies are prebuilt: changing `Cargo.toml` in the browser doesn't add crates.
- Proc macros run in the browser only if linked into rustc: today serde_derive (what Caprice uses).
  Other proc macro crates in the dependency graph load, but using one of their macros gives a compile
  error that says so. Adding one means adding it to `compiler/rustc_static_proc_macros` and rebuilding
  rustc (a few minutes once the first build is done).
- The browser has no system fonts: Caprice draws with egui's default font there.
- File dialogs, printing and the Claude features of Caprice don't work in the browser yet.
- Tested in Chromium. Firefox has the needed APIs (shared memory, module workers) but isn't tested.
- rustc is built with `panic=abort`: a compile error ends it with a trap after the diagnostics, which the
  IDE reports as a failed build, not a crash.

## License

MIT (see `LICENSE`). The parts from others keep their licenses: see `THIRD-PARTY-NOTICES.md`.
