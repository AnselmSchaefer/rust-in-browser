"""Assembles what the browser IDE downloads, under web/toolchain/:

* rustc.wasm and wasm-bindgen.wasm (gzipped), with their memory limits in manifest.json;
* sysroot.pack: the official 1.98.0 std for wasm32-unknown-unknown, at /sysroot;
* deps.pack: Caprice's dependencies, built natively by cargo for wasm32-unknown-unknown (rlibs at
  /deps/wasm32) and its proc macros for the build machine (at /deps/host; the wasm rustc reads their
  metadata, and runs the ones linked into it);
* project.pack: the Caprice project (caprice-web/), at /project;
* build.json: the rustc command cargo would run for Caprice, with paths in the browser's file system.

    python tools/build_assets.py [--rustc build/rustc.wasm]
"""

from __future__ import annotations

import argparse
import gzip
import json
import os
import shlex
import shutil
import subprocess
from pathlib import Path

from make_pack import collect, write_pack

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "web" / "toolchain"
PROJECT = ROOT / "caprice-web"
TARGET_DIR = ROOT / "build" / "caprice-target"
# The browser's rustc is 1.98.0: the std and the dependencies must come from the same compiler.
TOOLCHAIN = Path.home() / ".rustup" / "toolchains" / "1.98.0-x86_64-unknown-linux-gnu"
STD = TOOLCHAIN / "lib" / "rustlib" / "wasm32-unknown-unknown" / "lib"


def memory_limits(wasm: bytes) -> dict:
    """The imported memory's limits (pages), from the module's import section."""
    at = 8

    def u32() -> int:
        nonlocal at
        result = shift = 0
        while True:
            b = wasm[at]
            at += 1
            result |= (b & 0x7F) << shift
            shift += 7
            if not b & 0x80:
                return result

    def skip_name() -> None:
        nonlocal at
        n = u32()
        at += n

    while at < len(wasm):
        section, size = wasm[at], None
        at += 1
        size = u32()
        end = at + size
        if section == 2:
            for _ in range(u32()):
                skip_name()
                skip_name()
                kind = wasm[at]
                at += 1
                if kind == 0:
                    u32()
                elif kind == 1:
                    at += 1
                    flags = wasm[at]
                    at += 1
                    u32()
                    if flags & 1:
                        u32()
                elif kind == 3:
                    at += 2
                elif kind == 4:
                    at += 1
                    u32()
                elif kind == 2:
                    flags = wasm[at]
                    at += 1
                    low = u32()
                    high = u32() if flags & 1 else 65536
                    return {"memoryMin": low, "memoryMax": high}
        at = end
    raise SystemExit("the module imports no memory (is it built for wasm32-wasip1-threads?)")


def add_program(manifest: dict, name: str, wasm_path: Path, max_threads: int) -> None:
    data = wasm_path.read_bytes()
    out = OUT / f"{name}.wasm"
    out.write_bytes(data)
    with gzip.open(out.with_name(out.name + ".gz"), "wb", compresslevel=6) as f:
        f.write(data)
    out.with_name(out.name + ".size").write_text(str(len(data)))
    manifest["programs"][name] = {"url": f"toolchain/{name}.wasm", **memory_limits(data), "maxThreads": max_threads}
    print(f"{name}: {len(data) / 2**20:.1f} MiB")


def cargo_build() -> tuple[list[str], list[dict]]:
    """Builds Caprice for wasm32-unknown-unknown; returns cargo's rustc command for it and the artifacts."""
    (PROJECT / "src" / "main.rs").touch()
    env = {**os.environ, "CARGO_TARGET_DIR": str(TARGET_DIR), "RUSTUP_TOOLCHAIN": "1.98.0"}
    run = subprocess.run(
        ["cargo", "build", "-v", "--target", "wasm32-unknown-unknown", "--message-format=json"],
        cwd=PROJECT, env=env, capture_output=True, text=True,
    )
    if run.returncode != 0:
        raise SystemExit(run.stderr[-4000:])
    command = next(
        line.strip()[len("Running `"):-1]
        for line in run.stderr.splitlines()
        if line.strip().startswith("Running `") and "--crate-name caprice " in line
    )
    artifacts = [json.loads(l) for l in run.stdout.splitlines() if l.startswith("{")]
    artifacts = [a for a in artifacts if a.get("reason") == "compiler-artifact"]
    return shlex.split(command), artifacts


def browser_command(argv: list[str], deps: dict[str, str]) -> list[str]:
    """cargo's rustc arguments for Caprice, for the browser: paths in its file system, a cdylib (the page
    starts it through `caprice_start`), JSON diagnostics for the editor, incremental builds."""
    args, i = [], 1
    while i < len(argv):
        a = argv[i]
        nxt = argv[i + 1] if i + 1 < len(argv) else None
        if a == "src/main.rs":
            args.append("/project/src/main.rs")
        elif a == "--crate-type":
            args += ["--crate-type", "cdylib"]
            i += 1
        elif a.startswith("--emit="):
            args.append("--emit=link")
        elif a.startswith("--json="):
            args.append("--json=diagnostic-rendered-ansi")
        elif a == "--out-dir":
            args += ["--out-dir", "/out"]
            i += 1
        elif a == "-C" and nxt and nxt.startswith("incremental="):
            args += ["-C", "incremental=/incremental"]
            i += 1
        elif a == "-L" and nxt and nxt.startswith("dependency="):
            i += 1
        elif a == "-C" and nxt and nxt.startswith("extra-filename="):
            i += 1  # the output is /out/caprice.wasm
        elif a == "--extern":
            name, _, path = nxt.partition("=")
            args += ["--extern", f"{name}={deps[Path(path).name]}"]
            i += 1
        else:
            args.append(a)
        i += 1
    return args + [
        "--sysroot", "/sysroot",
        "-L", "dependency=/deps/wasm32",
        "-L", "dependency=/deps/host",
    ]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--rustc", type=Path, default=ROOT / "build" / "rustc.wasm")
    parser.add_argument("--wasm-bindgen", type=Path, default=ROOT / "tools" / "wasm-bindgen-wasi" / "target"
                        / "wasm32-wasip1-threads" / "release" / "wasm-bindgen-wasi.wasm")
    parser.add_argument("--skip-packs", action="store_true", help="only the programs and the manifest")
    args = parser.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)

    manifest = {"programs": {}, "packs": []}
    if args.rustc.exists():
        add_program(manifest, "rustc", args.rustc, max_threads=64)
    else:
        print(f"no {args.rustc} yet: the manifest has no rustc")
    add_program(manifest, "wasm-bindgen", args.wasm_bindgen, max_threads=16)

    argv, artifacts = cargo_build()
    # Dependency rlibs for the target, and proc macros for the build machine (named as the wasm host's
    # dylibs: `name-hash.wasm`, no `lib`).
    deps: dict[str, str] = {}
    files: list[tuple[str, Path]] = []
    for artifact in artifacts:
        if artifact["target"]["name"] == "caprice":
            continue
        for f in map(Path, artifact["filenames"]):
            # cargo's rlibs hold their whole metadata, so their .rmeta files aren't needed.
            if f.suffix == ".rlib" and "wasm32-unknown-unknown" in f.parts:
                deps[f.name] = f"/deps/wasm32/{f.name}"
                files.append((f"wasm32/{f.name}", f))
            elif f.suffix == ".so" and "proc-macro" in artifact["target"]["kind"]:
                name = f.name.removeprefix("lib").removesuffix(".so") + ".wasm"
                deps[f.name] = f"/deps/host/{name}"
                files.append((f"host/{name}", f))
    build = {
        "rustc": browser_command(argv, deps),
        "output": "/out/caprice.wasm",
        "bindgen": ["/out/caprice.wasm", "/out/pkg", "caprice"],
        "js": "/out/pkg/caprice.js",
        "wasm": "/out/pkg/caprice_bg.wasm",
    }
    (OUT / "build.json").write_text(json.dumps(build, indent=1))

    if not args.skip_packs:
        # Since 1.89 the std's rlibs hold only a stub of their metadata; the rest is in the .rmeta files.
        sysroot = collect(f"{STD}:lib/rustlib/wasm32-unknown-unknown/lib:*.rlib")
        sysroot += collect(f"{STD}:lib/rustlib/wasm32-unknown-unknown/lib:*.rmeta")
        print(f"sysroot.pack: {write_pack(OUT / 'sysroot.pack', sysroot) / 2**20:.1f} MiB")
        print(f"deps.pack: {write_pack(OUT / 'deps.pack', files) / 2**20:.1f} MiB, {len(files)} files")
        project = [
            (str(p.relative_to(PROJECT)), p)
            for p in sorted(PROJECT.rglob("*"))
            if p.is_file() and p.relative_to(PROJECT).parts[0] in ("src", "assets", "Cargo.toml", "Cargo.lock")
        ]
        print(f"project.pack: {write_pack(OUT / 'project.pack', project) / 2**20:.1f} MiB, {len(project)} files")
    for name, at in [("sysroot", "/sysroot"), ("deps", "/deps"), ("project", "/project")]:
        manifest["packs"].append({"url": f"toolchain/{name}.pack", "at": at})
    (OUT / "manifest.json").write_text(json.dumps(manifest, indent=1))
    # The uncompressed copies are only for building: the bridge serves the .gz files.
    for p in OUT.glob("*.pack"):
        p.unlink()
    for p in OUT.glob("*.wasm"):
        p.unlink()
    shutil.rmtree(OUT / "__pycache__", ignore_errors=True)


if __name__ == "__main__":
    main()
