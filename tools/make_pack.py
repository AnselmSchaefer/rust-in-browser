"""Packs files into one download for the browser's file system (mounted by fs-worker.js).

Format: b"RPK1", u32 index length (little endian), the JSON index [[path, offset, size], ...], then
the files' bytes, which the offsets count from. Next to OUT it writes OUT.gz (what the bridge serves)
and OUT.size (the unpacked length, for the download's progress).

    python make_pack.py OUT SRC_DIR[:PREFIX[:GLOB]] ...
    python make_pack.py web/toolchain/sysroot.pack ~/.rustup/.../lib/rustlib/wasm32-unknown-unknown/lib:lib/rustlib/wasm32-unknown-unknown/lib:*.rlib
"""

from __future__ import annotations

import gzip
import json
import shutil
import struct
import sys
from pathlib import Path


def collect(spec: str) -> list[tuple[str, Path]]:
    src, _, rest = spec.partition(":")
    prefix, _, pattern = rest.partition(":")
    root = Path(src).expanduser()
    files = sorted(p for p in root.rglob(pattern or "*") if p.is_file())
    return [(str(Path(prefix) / p.relative_to(root)) if prefix else str(p.relative_to(root)), p) for p in files]


def write_pack(out: Path, entries: list[tuple[str, Path | bytes]]) -> int:
    index, blobs, offset = [], [], 0
    for name, source in entries:
        data = source if isinstance(source, bytes) else source.read_bytes()
        index.append([name, offset, len(data)])
        blobs.append(data)
        offset += len(data)
    header = json.dumps(index, separators=(",", ":")).encode()
    out.parent.mkdir(parents=True, exist_ok=True)
    with out.open("wb") as f:
        f.write(b"RPK1" + struct.pack("<I", len(header)) + header)
        for blob in blobs:
            f.write(blob)
    size = out.stat().st_size
    with out.open("rb") as src, gzip.open(out.with_name(out.name + ".gz"), "wb", compresslevel=6) as dst:
        shutil.copyfileobj(src, dst, 1 << 20)
    out.with_name(out.name + ".size").write_text(str(size))
    return size


def main() -> None:
    out, *specs = sys.argv[1:]
    entries = [entry for spec in specs for entry in collect(spec)]
    size = write_pack(Path(out), entries)
    gz = Path(out + ".gz").stat().st_size
    print(f"{out}: {len(entries)} files, {size / 2**20:.1f} MiB ({gz / 2**20:.1f} MiB gzipped)")


if __name__ == "__main__":
    main()
