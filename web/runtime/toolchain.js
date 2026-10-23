// Runs WASI programs (wasm32-wasip1-threads) in the page: rustc, wasm-bindgen. Each run is a process
// with its own shared memory (up to the module's maximum, 4 GiB for rustc) and a Web Worker per thread.
// The file system lives in one worker that every process and the page share.
//
// A process ends when it exits, or when any of its threads traps: then all its workers are stopped
// and the run reports the crash, instead of the other threads waiting on the one that is gone.

import { CTRL_BYTES } from "./wasi-defs.js";

const here = (path) => new URL(path, import.meta.url);

export class Toolchain {
  constructor() {
    this.fs = new Worker(here("./fs-worker.js"), { type: "module" });
    this.replies = new Map();
    this.outputs = new Map();
    this.nextId = 1;
    this.nextPid = 1;
    this.modules = new Map();
    this.fs.onmessage = (e) => {
      const msg = e.data;
      if (msg.type === "reply") {
        const r = this.replies.get(msg.id);
        this.replies.delete(msg.id);
        if (msg.error !== undefined) r.reject(new Error(msg.error));
        else r.resolve(msg.result);
      } else if (msg.type === "output") {
        this.outputs.get(msg.pid)?.(msg.fd, msg.text);
      }
    };
  }

  // A request to the file system (see `pageRequests` in fs-worker.js).
  request(op, args = {}, transfer = []) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.replies.set(id, { resolve, reject });
      this.fs.postMessage({ id, op, ...args }, transfer);
    });
  }

  writeFile(path, data) {
    return this.request("writeFile", { path, data });
  }
  readFile(path, text = false) {
    return this.request("readFile", { path, text });
  }

  // Downloads a pack of read-only files and mounts it at `at`.
  async mountPack(url, at, onProgress) {
    const buffer = await download(url, onProgress);
    return this.request("mountPack", { buffer, at }, [buffer]);
  }

  // Compiles a program once; `info` gives its memory limits (pages) and thread limit.
  async load(name, url, info) {
    if (!this.modules.has(name)) {
      const module = WebAssembly.compileStreaming(fetch(url));
      this.modules.set(name, { module, info });
    }
    const entry = this.modules.get(name);
    return { module: await entry.module, info: entry.info };
  }

  // Runs a loaded program. Resolves with { code, crash, stdout, stderr, ms }.
  async run(name, { args = [], env = {}, onOutput = () => {} } = {}) {
    const { module, info } = await this.load(name);
    const pid = this.nextPid++;
    const memory = new WebAssembly.Memory({ initial: info.memoryMin, maximum: info.memoryMax, shared: true });
    const counter = new SharedArrayBuffer(8); // [next thread id, live threads]
    const workers = new Map();
    const idle = []; // workers whose thread has ended, kept for the next one
    const started = performance.now();
    let stdout = "";
    let stderr = "";
    this.outputs.set(pid, (fd, text) => {
      if (fd === 1) stdout += text;
      else stderr += text;
      onOutput(fd, text);
    });
    await this.request("procStart", { pid, memory });

    let finish;
    const done = new Promise((resolve) => (finish = resolve));
    let ended = false;
    const end = async (result) => {
      if (ended) return;
      ended = true;
      for (const w of workers.values()) w.terminate();
      for (const w of idle) w.terminate();
      workers.clear();
      idle.length = 0;
      await this.request("procEnd", { pid });
      this.outputs.delete(pid);
      finish({ ...result, stdout, stderr, ms: Math.round(performance.now() - started) });
    };

    const startThread = (tid, startArg) => {
      const reused = tid !== 0 && idle.pop();
      if (reused) {
        workers.set(tid, reused);
        reused.tid = tid;
        reused.postMessage({ type: "run", tid, startArg });
        return;
      }
      const worker = new Worker(here("./thread-worker.js"), { type: "module", name: `${name}#${tid}` });
      worker.tid = tid;
      workers.set(tid, worker);
      const channel = new MessageChannel();
      const ctrl = new SharedArrayBuffer(CTRL_BYTES);
      this.request("attach", { pid, ctrl, port: channel.port1 }, [channel.port1]);
      worker.onmessage = (e) => {
        const msg = e.data;
        if (msg.type === "spawn") startThread(msg.tid, msg.startArg);
        else if (msg.type === "exit") end({ code: msg.code });
        else if (msg.type === "crash") end({ code: null, crash: `thread ${msg.tid}: ${msg.message}`, stack: msg.stack });
        else if (msg.type === "thread-done") {
          workers.delete(msg.tid);
          if (!ended) idle.push(worker);
        }
      };
      worker.onerror = (e) => end({ code: null, crash: `thread ${worker.tid}: ${e.message}` });
      worker.postMessage(
        {
          module, memory, tid, startArg, ctrl, port: channel.port2, counter,
          args: [name, ...args], env, maxThreads: info.maxThreads ?? 128,
        },
        [channel.port2],
      );
    };
    startThread(0, 0);
    return done;
  }
}

// Fetches `url` into an ArrayBuffer, reporting (received, total) bytes as they come.
async function download(url, onProgress = () => {}) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${url}: ${response.status}`);
  const total = Number(response.headers.get("x-decoded-length") || response.headers.get("content-length") || 0);
  const reader = response.body.getReader();
  const chunks = [];
  let received = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
    received += value.length;
    onProgress(received, total);
  }
  const out = new Uint8Array(received);
  let at = 0;
  for (const c of chunks) {
    out.set(c, at);
    at += c.length;
  }
  return out.buffer;
}
