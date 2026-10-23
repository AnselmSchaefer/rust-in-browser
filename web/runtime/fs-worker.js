// The file system: one in-memory tree for every program the IDE runs (rustc, wasm-bindgen) and for
// the IDE itself. It also holds each process's open files, so all threads of a process share them.
//
// Threads call in synchronously: a thread fills its control block (see wasi-defs.js), pings this
// worker over its MessagePort and waits with Atomics.wait. The call reads and writes the process's
// shared WebAssembly memory directly, so file data is copied once, between the file and the caller.
// The page talks to this worker with ordinary async messages.

import { CTRL_ARGS, ERRNO, FDFLAGS, FILETYPE, FS_CALLS, OFLAGS, STATE, WHENCE } from "./wasi-defs.js";

let nextIno = 1n;
const nowNs = () => BigInt(Math.round((performance.timeOrigin + performance.now()) * 1e6));

class Dir {
  constructor(parent) {
    this.kind = "dir";
    this.children = new Map();
    this.parent = parent ?? this;
    this.ino = nextIno++;
    this.mtime = nowNs();
  }
}

class File {
  // `data` may view a mounted pack: such files copy their bytes before the first write.
  constructor(data = new Uint8Array(0), borrowed = false) {
    this.kind = "file";
    this.data = data;
    this.size = data.length;
    this.borrowed = borrowed;
    this.ino = nextIno++;
    this.mtime = nowNs();
  }
  reserve(size) {
    if (!this.borrowed && size <= this.data.length) return;
    const grown = new Uint8Array(Math.max(size, this.borrowed ? this.size : this.data.length * 2, 4096));
    grown.set(this.data.subarray(0, this.size));
    this.data = grown;
    this.borrowed = false;
  }
  write(pos, bytes) {
    this.reserve(pos + bytes.length);
    if (pos > this.size) this.data.fill(0, this.size, pos);
    this.data.set(bytes, pos);
    this.size = Math.max(this.size, pos + bytes.length);
    this.mtime = nowNs();
  }
  truncate(size) {
    this.reserve(size);
    if (size > this.size) this.data.fill(0, this.size, size);
    this.size = size;
    this.mtime = nowNs();
  }
  bytes() {
    return this.data.subarray(0, this.size);
  }
}

const root = new Dir();

// --- Paths ------------------------------------------------------------------------------------------

function parts(path) {
  return path.split("/").filter((p) => p !== "" && p !== ".");
}

// The node at `path` from `base` (a Dir), or an errno.
function lookup(base, path) {
  let node = path.startsWith("/") ? root : base;
  for (const name of parts(path)) {
    if (node.kind !== "dir") return ERRNO.NOTDIR;
    node = name === ".." ? node.parent : node.children.get(name);
    if (!node) return ERRNO.NOENT;
  }
  return node;
}

// The directory that holds the last part of `path`, and that part's name; or an errno.
function lookupParent(base, path) {
  const ps = parts(path);
  const name = ps.pop();
  if (name === undefined || name === "..") return ERRNO.INVAL;
  const dir = lookup(path.startsWith("/") ? root : base, ps.join("/") || ".");
  if (typeof dir === "number") return dir;
  if (dir.kind !== "dir") return ERRNO.NOTDIR;
  return { dir, name };
}

function mkdirp(path) {
  let node = root;
  for (const name of parts(path)) {
    let next = name === ".." ? node.parent : node.children.get(name);
    if (!next) {
      next = new Dir(node);
      node.children.set(name, next);
    }
    if (next.kind !== "dir") throw new Error(`not a directory: ${path}`);
    node = next;
  }
  return node;
}

function writeFileAt(path, bytes, borrowed = false) {
  const ps = parts(path);
  const name = ps.pop();
  const dir = mkdirp(ps.join("/"));
  const file = new File(bytes, borrowed);
  dir.children.set(name, file);
  return file;
}

// --- Processes and their open files -----------------------------------------------------------------

const procs = new Map();

class Proc {
  constructor(pid, memory) {
    this.pid = pid;
    this.memory = memory;
    // 0-2: stdin (empty), stdout, stderr. 3: "/" preopened, through which libc opens every path.
    this.fds = new Map([
      [0, { stdio: 0 }],
      [1, { stdio: 1 }],
      [2, { stdio: 2 }],
      [3, { node: root, pos: 0, preopen: "/" }],
    ]);
    this.decoders = { 1: new TextDecoder(), 2: new TextDecoder() };
  }
  u8() {
    return new Uint8Array(this.memory.buffer);
  }
  dv() {
    return new DataView(this.memory.buffer);
  }
  str(ptr, len) {
    return new TextDecoder().decode(this.u8().slice(ptr, ptr + len));
  }
  addFd(entry) {
    let fd = 4;
    while (this.fds.has(fd)) fd++;
    this.fds.set(fd, entry);
    return fd;
  }
  // The iovecs at `iovs` as (pointer, length) pairs.
  iovecs(iovs, count) {
    const dv = this.dv();
    const list = [];
    for (let i = 0; i < count; i++) {
      list.push([dv.getUint32(iovs + i * 8, true), dv.getUint32(iovs + i * 8 + 4, true)]);
    }
    return list;
  }
  emit(fd, bytes) {
    const text = this.decoders[fd].decode(bytes, { stream: true });
    if (text) postMessage({ type: "output", pid: this.pid, fd, text });
  }
}

function fileType(entry) {
  if (entry.stdio !== undefined) return FILETYPE.CHARACTER_DEVICE;
  return entry.node.kind === "dir" ? FILETYPE.DIRECTORY : FILETYPE.REGULAR_FILE;
}

function writeFilestat(dv, ptr, node, type) {
  dv.setBigUint64(ptr, 0n, true); // dev
  dv.setBigUint64(ptr + 8, node ? node.ino : 0n, true);
  dv.setUint8(ptr + 16, type);
  dv.setBigUint64(ptr + 24, 1n, true); // nlink
  dv.setBigUint64(ptr + 32, BigInt(node && node.kind === "file" ? node.size : 0), true);
  const t = node ? node.mtime : 0n;
  dv.setBigUint64(ptr + 40, t, true);
  dv.setBigUint64(ptr + 48, t, true);
  dv.setBigUint64(ptr + 56, t, true);
}

function nodeType(node) {
  return node.kind === "dir" ? FILETYPE.DIRECTORY : FILETYPE.REGULAR_FILE;
}

// The directory an `*at` call resolves its path from.
function dirOf(p, fd) {
  const entry = p.fds.get(fd);
  if (!entry || !entry.node) return ERRNO.BADF;
  if (entry.node.kind !== "dir") return ERRNO.NOTDIR;
  return entry.node;
}

function readInto(p, iovs, count, file, pos) {
  const u8 = p.u8();
  let total = 0;
  for (const [ptr, len] of p.iovecs(iovs, count)) {
    const n = Math.max(0, Math.min(len, file.size - pos));
    u8.set(file.data.subarray(pos, pos + n), ptr);
    pos += n;
    total += n;
    if (n < len) break;
  }
  return total;
}

function gather(p, iovs, count) {
  const u8 = p.u8();
  const chunks = p.iovecs(iovs, count).map(([ptr, len]) => u8.slice(ptr, ptr + len));
  const total = chunks.reduce((n, c) => n + c.length, 0);
  const all = new Uint8Array(total);
  let at = 0;
  for (const c of chunks) {
    all.set(c, at);
    at += c.length;
  }
  return all;
}

// --- The calls ----------------------------------------------------------------------------------------
// Each gets the process and an accessor for its arguments: `a(i)` as an unsigned 32-bit number,
// `b(i)` as a BigInt (64-bit arguments).

const calls = {
  fd_write(p, a) {
    const [fd, iovs, count, out] = [a(0), a(1), a(2), a(3)];
    const entry = p.fds.get(fd);
    if (!entry) return ERRNO.BADF;
    const bytes = gather(p, iovs, count);
    if (entry.stdio === 1 || entry.stdio === 2) {
      p.emit(entry.stdio, bytes);
    } else if (entry.stdio === 0) {
      return ERRNO.BADF;
    } else {
      if (entry.node.kind !== "file") return ERRNO.ISDIR;
      if (entry.append) entry.pos = entry.node.size;
      entry.node.write(entry.pos, bytes);
      entry.pos += bytes.length;
    }
    p.dv().setUint32(out, bytes.length, true);
    return ERRNO.SUCCESS;
  },
  fd_read(p, a) {
    const [fd, iovs, count, out] = [a(0), a(1), a(2), a(3)];
    const entry = p.fds.get(fd);
    if (!entry) return ERRNO.BADF;
    let n = 0;
    if (entry.stdio === undefined) {
      if (entry.node.kind !== "file") return ERRNO.ISDIR;
      n = readInto(p, iovs, count, entry.node, entry.pos);
      entry.pos += n;
    } else if (entry.stdio !== 0) {
      return ERRNO.BADF;
    }
    p.dv().setUint32(out, n, true);
    return ERRNO.SUCCESS;
  },
  fd_pread(p, a, b) {
    const [fd, iovs, count, offset, out] = [a(0), a(1), a(2), Number(b(3)), a(4)];
    const entry = p.fds.get(fd);
    if (!entry || !entry.node) return ERRNO.BADF;
    if (entry.node.kind !== "file") return ERRNO.ISDIR;
    p.dv().setUint32(out, readInto(p, iovs, count, entry.node, offset), true);
    return ERRNO.SUCCESS;
  },
  fd_pwrite(p, a, b) {
    const [fd, iovs, count, offset, out] = [a(0), a(1), a(2), Number(b(3)), a(4)];
    const entry = p.fds.get(fd);
    if (!entry || !entry.node) return ERRNO.BADF;
    if (entry.node.kind !== "file") return ERRNO.ISDIR;
    const bytes = gather(p, iovs, count);
    entry.node.write(offset, bytes);
    p.dv().setUint32(out, bytes.length, true);
    return ERRNO.SUCCESS;
  },
  fd_seek(p, a, b) {
    const [fd, offset, whence, out] = [a(0), Number(BigInt.asIntN(64, b(1))), a(2), a(3)];
    const entry = p.fds.get(fd);
    if (!entry) return ERRNO.BADF;
    if (!entry.node) return ERRNO.SPIPE;
    const size = entry.node.kind === "file" ? entry.node.size : 0;
    const base = whence === WHENCE.SET ? 0 : whence === WHENCE.CUR ? entry.pos : whence === WHENCE.END ? size : NaN;
    const pos = base + offset;
    if (!(pos >= 0)) return ERRNO.INVAL;
    entry.pos = pos;
    p.dv().setBigUint64(out, BigInt(pos), true);
    return ERRNO.SUCCESS;
  },
  fd_tell(p, a) {
    const entry = p.fds.get(a(0));
    if (!entry) return ERRNO.BADF;
    if (!entry.node) return ERRNO.SPIPE;
    p.dv().setBigUint64(a(1), BigInt(entry.pos), true);
    return ERRNO.SUCCESS;
  },
  fd_close(p, a) {
    const fd = a(0);
    if (!p.fds.has(fd)) return ERRNO.BADF;
    p.fds.delete(fd);
    return ERRNO.SUCCESS;
  },
  fd_fdstat_get(p, a) {
    const [fd, ptr] = [a(0), a(1)];
    const entry = p.fds.get(fd);
    if (!entry) return ERRNO.BADF;
    const dv = p.dv();
    dv.setUint8(ptr, fileType(entry));
    dv.setUint16(ptr + 2, entry.append ? FDFLAGS.APPEND : 0, true);
    dv.setBigUint64(ptr + 8, 0xffffffffffffffffn, true);
    dv.setBigUint64(ptr + 16, 0xffffffffffffffffn, true);
    return ERRNO.SUCCESS;
  },
  fd_fdstat_set_flags(p, a) {
    const entry = p.fds.get(a(0));
    if (!entry) return ERRNO.BADF;
    entry.append = (a(1) & FDFLAGS.APPEND) !== 0;
    return ERRNO.SUCCESS;
  },
  fd_filestat_get(p, a) {
    const [fd, ptr] = [a(0), a(1)];
    const entry = p.fds.get(fd);
    if (!entry) return ERRNO.BADF;
    writeFilestat(p.dv(), ptr, entry.node, fileType(entry));
    return ERRNO.SUCCESS;
  },
  fd_filestat_set_size(p, a, b) {
    const entry = p.fds.get(a(0));
    if (!entry || !entry.node) return ERRNO.BADF;
    if (entry.node.kind !== "file") return ERRNO.ISDIR;
    entry.node.truncate(Number(b(1)));
    return ERRNO.SUCCESS;
  },
  fd_filestat_set_times(p, a) {
    return p.fds.has(a(0)) ? ERRNO.SUCCESS : ERRNO.BADF;
  },
  fd_prestat_get(p, a) {
    const entry = p.fds.get(a(0));
    if (!entry || !entry.preopen) return ERRNO.BADF;
    const dv = p.dv();
    dv.setUint8(a(1), 0);
    dv.setUint32(a(1) + 4, new TextEncoder().encode(entry.preopen).length, true);
    return ERRNO.SUCCESS;
  },
  fd_prestat_dir_name(p, a) {
    const entry = p.fds.get(a(0));
    if (!entry || !entry.preopen) return ERRNO.BADF;
    const name = new TextEncoder().encode(entry.preopen);
    p.u8().set(name.subarray(0, a(2)), a(1));
    return ERRNO.SUCCESS;
  },
  fd_readdir(p, a, b) {
    const [fd, buf, bufLen, cookie, out] = [a(0), a(1), a(2), Number(b(3)), a(4)];
    const entry = p.fds.get(fd);
    if (!entry || !entry.node) return ERRNO.BADF;
    if (entry.node.kind !== "dir") return ERRNO.NOTDIR;
    const dir = entry.node;
    const names = [[".", dir], ["..", dir.parent], ...dir.children];
    const u8 = p.u8();
    let used = 0;
    for (let i = cookie; i < names.length && used < bufLen; i++) {
      const [name, node] = names[i];
      const nameBytes = new TextEncoder().encode(name);
      const rec = new Uint8Array(24 + nameBytes.length);
      const dv = new DataView(rec.buffer);
      dv.setBigUint64(0, BigInt(i + 1), true);
      dv.setBigUint64(8, node.ino, true);
      dv.setUint32(16, nameBytes.length, true);
      dv.setUint8(20, nodeType(node));
      rec.set(nameBytes, 24);
      const n = Math.min(rec.length, bufLen - used);
      u8.set(rec.subarray(0, n), buf + used);
      used += n;
    }
    p.dv().setUint32(out, used, true);
    return ERRNO.SUCCESS;
  },
  fd_sync: (p, a) => (p.fds.has(a(0)) ? ERRNO.SUCCESS : ERRNO.BADF),
  fd_datasync: (p, a) => (p.fds.has(a(0)) ? ERRNO.SUCCESS : ERRNO.BADF),
  fd_advise: (p, a) => (p.fds.has(a(0)) ? ERRNO.SUCCESS : ERRNO.BADF),
  fd_allocate(p, a, b) {
    const entry = p.fds.get(a(0));
    if (!entry || !entry.node || entry.node.kind !== "file") return ERRNO.BADF;
    const end = Number(b(1) + b(2));
    if (end > entry.node.size) entry.node.truncate(end);
    return ERRNO.SUCCESS;
  },
  fd_renumber(p, a) {
    const [from, to] = [a(0), a(1)];
    const entry = p.fds.get(from);
    if (!entry || !p.fds.has(to)) return ERRNO.BADF;
    p.fds.set(to, entry);
    p.fds.delete(from);
    return ERRNO.SUCCESS;
  },
  path_open(p, a) {
    const [dirfd, path, oflags, fdflags, out] = [a(0), p.str(a(2), a(3)), a(4), a(7), a(8)];
    const base = dirOf(p, dirfd);
    if (typeof base === "number") return base;
    let node = lookup(base, path);
    if (node === ERRNO.NOENT && oflags & OFLAGS.CREAT) {
      const parent = lookupParent(base, path);
      if (typeof parent === "number") return parent;
      node = new File();
      parent.dir.children.set(parent.name, node);
      parent.dir.mtime = nowNs();
    } else if (typeof node === "number") {
      return node;
    } else if (oflags & OFLAGS.CREAT && oflags & OFLAGS.EXCL) {
      return ERRNO.EXIST;
    }
    if (oflags & OFLAGS.DIRECTORY && node.kind !== "dir") return ERRNO.NOTDIR;
    if (oflags & OFLAGS.TRUNC) {
      if (node.kind !== "file") return ERRNO.ISDIR;
      node.truncate(0);
    }
    const fd = p.addFd({ node, pos: 0, append: (fdflags & FDFLAGS.APPEND) !== 0 });
    p.dv().setUint32(out, fd, true);
    return ERRNO.SUCCESS;
  },
  path_filestat_get(p, a) {
    const base = dirOf(p, a(0));
    if (typeof base === "number") return base;
    const node = lookup(base, p.str(a(2), a(3)));
    if (typeof node === "number") return node;
    writeFilestat(p.dv(), a(4), node, nodeType(node));
    return ERRNO.SUCCESS;
  },
  path_filestat_set_times(p, a) {
    const base = dirOf(p, a(0));
    if (typeof base === "number") return base;
    const node = lookup(base, p.str(a(2), a(3)));
    return typeof node === "number" ? node : ERRNO.SUCCESS;
  },
  path_create_directory(p, a) {
    const base = dirOf(p, a(0));
    if (typeof base === "number") return base;
    const parent = lookupParent(base, p.str(a(1), a(2)));
    if (typeof parent === "number") return parent;
    if (parent.dir.children.has(parent.name)) return ERRNO.EXIST;
    parent.dir.children.set(parent.name, new Dir(parent.dir));
    parent.dir.mtime = nowNs();
    return ERRNO.SUCCESS;
  },
  path_remove_directory(p, a) {
    const base = dirOf(p, a(0));
    if (typeof base === "number") return base;
    const parent = lookupParent(base, p.str(a(1), a(2)));
    if (typeof parent === "number") return parent;
    const node = parent.dir.children.get(parent.name);
    if (!node) return ERRNO.NOENT;
    if (node.kind !== "dir") return ERRNO.NOTDIR;
    if (node.children.size) return ERRNO.NOTEMPTY;
    parent.dir.children.delete(parent.name);
    return ERRNO.SUCCESS;
  },
  path_unlink_file(p, a) {
    const base = dirOf(p, a(0));
    if (typeof base === "number") return base;
    const parent = lookupParent(base, p.str(a(1), a(2)));
    if (typeof parent === "number") return parent;
    const node = parent.dir.children.get(parent.name);
    if (!node) return ERRNO.NOENT;
    if (node.kind === "dir") return ERRNO.ISDIR;
    parent.dir.children.delete(parent.name);
    return ERRNO.SUCCESS;
  },
  path_rename(p, a) {
    const from = dirOf(p, a(0));
    const to = dirOf(p, a(3));
    if (typeof from === "number") return from;
    if (typeof to === "number") return to;
    const src = lookupParent(from, p.str(a(1), a(2)));
    const dst = lookupParent(to, p.str(a(4), a(5)));
    if (typeof src === "number") return src;
    if (typeof dst === "number") return dst;
    const node = src.dir.children.get(src.name);
    if (!node) return ERRNO.NOENT;
    const old = dst.dir.children.get(dst.name);
    if (old && old.kind === "dir" && (node.kind !== "dir" || old.children.size)) return ERRNO.NOTEMPTY;
    if (old && old.kind !== "dir" && node.kind === "dir") return ERRNO.NOTDIR;
    src.dir.children.delete(src.name);
    dst.dir.children.set(dst.name, node);
    if (node.kind === "dir") node.parent = dst.dir;
    return ERRNO.SUCCESS;
  },
  path_readlink: () => ERRNO.INVAL,
  path_symlink: () => ERRNO.NOTSUP,
  // A hard link is another entry for the same file (rustc links its incremental cache's files).
  path_link(p, a) {
    const from = dirOf(p, a(0));
    const to = dirOf(p, a(4));
    if (typeof from === "number") return from;
    if (typeof to === "number") return to;
    const node = lookup(from, p.str(a(2), a(3)));
    if (typeof node === "number") return node;
    if (node.kind === "dir") return ERRNO.PERM;
    const dst = lookupParent(to, p.str(a(5), a(6)));
    if (typeof dst === "number") return dst;
    if (dst.dir.children.has(dst.name)) return ERRNO.EXIST;
    dst.dir.children.set(dst.name, node);
    return ERRNO.SUCCESS;
  },
};

function serve(pid, ctrl) {
  const i32 = new Int32Array(ctrl, 0, 4);
  const args = new BigInt64Array(ctrl, 16, CTRL_ARGS);
  const a = (i) => Number(BigInt.asUintN(32, args[i]));
  const b = (i) => BigInt.asUintN(64, args[i]);
  const name = FS_CALLS[i32[1]];
  let result;
  try {
    const p = procs.get(pid);
    result = p ? calls[name](p, a, b) : ERRNO.BADF;
  } catch (e) {
    console.error(`fs: ${name} failed`, e);
    result = ERRNO.IO;
  }
  i32[1] = result;
  Atomics.store(i32, 0, STATE.DONE);
  Atomics.notify(i32, 0);
}

// --- Packs: many read-only files in one download ---------------------------------------------------
// "RPK1", u32 index length, JSON index [[path, offset, size], ...], then the data the offsets count in.

function mountPack(buffer, at) {
  const dv = new DataView(buffer);
  const magic = new TextDecoder().decode(new Uint8Array(buffer, 0, 4));
  if (magic !== "RPK1") throw new Error("not a pack");
  const indexLen = dv.getUint32(4, true);
  const index = JSON.parse(new TextDecoder().decode(new Uint8Array(buffer, 8, indexLen)));
  const dataStart = 8 + indexLen;
  for (const [path, offset, size] of index) {
    writeFileAt(`${at}/${path}`, new Uint8Array(buffer, dataStart + offset, size), true);
  }
  return index.length;
}

// --- The page's requests -----------------------------------------------------------------------------

function list(path) {
  const node = lookup(root, path);
  if (typeof node === "number") throw new Error(`no such directory: ${path}`);
  if (node.kind !== "dir") throw new Error(`not a directory: ${path}`);
  return [...node.children].map(([name, n]) => ({ name, kind: n.kind, size: n.kind === "file" ? n.size : 0 }));
}

function walk(path) {
  const out = [];
  const visit = (node, at) => {
    for (const [name, n] of node.children) {
      const p = `${at}/${name}`;
      if (n.kind === "dir") visit(n, p);
      else out.push({ path: p, size: n.size });
    }
  };
  const node = lookup(root, path);
  if (typeof node !== "number" && node.kind === "dir") visit(node, path === "/" ? "" : path.replace(/\/$/, ""));
  return out;
}

const pageRequests = {
  writeFile({ path, data }) {
    const bytes = typeof data === "string" ? new TextEncoder().encode(data) : new Uint8Array(data);
    writeFileAt(path, bytes);
  },
  readFile({ path, text }) {
    const node = lookup(root, path);
    if (typeof node === "number" || node.kind !== "file") throw new Error(`no such file: ${path}`);
    return text ? new TextDecoder().decode(node.bytes()) : node.bytes().slice();
  },
  exists({ path }) {
    return typeof lookup(root, path) !== "number";
  },
  mkdirp({ path }) {
    mkdirp(path);
  },
  list: ({ path }) => list(path),
  walk: ({ path }) => walk(path),
  remove({ path }) {
    const parent = lookupParent(root, path);
    if (typeof parent !== "number") parent.dir.children.delete(parent.name);
  },
  mountPack: ({ buffer, at }) => mountPack(buffer, at),
  procStart({ pid, memory }) {
    procs.set(pid, new Proc(pid, memory));
  },
  procEnd({ pid }) {
    const p = procs.get(pid);
    if (!p) return;
    for (const fd of [1, 2]) {
      const rest = p.decoders[fd].decode();
      if (rest) postMessage({ type: "output", pid, fd, text: rest });
    }
    procs.delete(pid);
  },
  attach({ pid, ctrl, port }) {
    port.onmessage = () => serve(pid, ctrl);
  },
};

onmessage = (e) => {
  const { id, op, ...args } = e.data;
  try {
    const result = pageRequests[op](args);
    const transfer = result instanceof Uint8Array ? [result.buffer] : [];
    postMessage({ type: "reply", id, result }, transfer);
  } catch (err) {
    postMessage({ type: "reply", id, error: String(err && err.message ? err.message : err) });
  }
};
