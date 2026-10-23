// node wasm-meta.mjs FILE.wasm: prints {"memoryMin", "memoryMax", "shared"} of the imported memory.
import { readFileSync } from "node:fs";

export function memoryLimits(bytes) {
  let at = 8;
  const u32 = () => {
    let result = 0, shift = 0, b;
    do {
      b = bytes[at++];
      result += (b & 0x7f) * 2 ** shift;
      shift += 7;
    } while (b & 0x80);
    return result;
  };
  const name = () => {
    const n = u32();
    at += n;
  };
  while (at < bytes.length) {
    const id = bytes[at++];
    const size = u32();
    const end = at + size;
    if (id === 2) {
      for (let count = u32(); count > 0; count--) {
        name();
        name();
        const kind = bytes[at++];
        if (kind === 0) u32();
        else if (kind === 1) { at++; const f = bytes[at++]; u32(); if (f & 1) u32(); }
        else if (kind === 3) at += 2;
        else if (kind === 4) { at++; u32(); }
        else if (kind === 2) {
          const flags = bytes[at++];
          const min = u32();
          const max = flags & 1 ? u32() : 65536;
          return { memoryMin: min, memoryMax: max, shared: (flags & 2) !== 0 };
        }
      }
    }
    at = end;
  }
  return null;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  console.log(JSON.stringify(memoryLimits(readFileSync(process.argv[2]))));
}
