// One thread of a WASI program: the main thread (`_start`) or one started by `wasi.thread-spawn`
// (`wasi_thread_start`). Every thread of a process runs its own instance on the process's shared
// memory. File calls go to the file-system worker; the rest is answered here.
//
// The page is told when the thread ends: "exit" (the program is done, by `proc_exit` or by `_start`
// returning), "thread-done", or "crash" (a trap, in any thread: the page then ends the whole process,
// rather than leaving the other threads waiting for one that is gone).
//
// A spawned thread's worker stays for the next thread of the process: the page sends it {type: "run",
// tid, startArg} and it calls `wasi_thread_start` again on its instance (which sets up the new thread's
// stack and TLS), instead of a new worker instantiating the module.

import { CTRL_ARGS, ERRNO, FS_CALLS, STATE } from "./wasi-defs.js";

class ProcExit extends Error {
  constructor(code) {
    super(`exit ${code}`);
    this.code = code;
  }
}

let startThread = null;

onmessage = (e) => {
  const job = e.data;
  try {
    if (job.type === "run") startThread(job.tid, job.startArg);
    else startThread = setUp(job);
  } catch (err) {
    if (err instanceof ProcExit) {
      postMessage({ type: "exit", code: err.code });
    } else {
      postMessage({ type: "crash", tid: job.tid, message: String(err), stack: err && err.stack });
    }
  }
};

// `ctrl` (CTRL_BYTES, shared) and `port` connect this thread to the file-system worker, which holds
// the same block and the port's other end. `counter` (shared Int32 [next thread id, live threads])
// gives out thread ids and caps the threads running at once. Returns the function that starts a thread.
function setUp({ module, memory, tid, startArg, args, env, ctrl, port, counter, maxThreads }) {
  const ctrlI32 = new Int32Array(ctrl, 0, 4);
  const ctrlArgs = new BigInt64Array(ctrl, 16, CTRL_ARGS);

  const fsCall = (n) => (...xs) => {
    for (let i = 0; i < xs.length; i++) ctrlArgs[i] = typeof xs[i] === "bigint" ? xs[i] : BigInt(xs[i] >>> 0);
    ctrlI32[1] = n;
    Atomics.store(ctrlI32, 0, STATE.REQUEST);
    port.postMessage(0);
    Atomics.wait(ctrlI32, 0, STATE.REQUEST);
    const result = ctrlI32[1];
    Atomics.store(ctrlI32, 0, STATE.IDLE);
    return result;
  };

  const enc = new TextEncoder();
  const argv = args.map((a) => enc.encode(`${a}\0`));
  const envv = Object.entries(env).map(([k, v]) => enc.encode(`${k}=${v}\0`));
  const u8 = () => new Uint8Array(memory.buffer);
  const dv = () => new DataView(memory.buffer);

  const putList = (list, ptrs, buf) => {
    const view = dv();
    const bytes = u8();
    let at = buf >>> 0;
    list.forEach((item, i) => {
      view.setUint32((ptrs >>> 0) + i * 4, at, true);
      bytes.set(item, at);
      at += item.length;
    });
    return ERRNO.SUCCESS;
  };
  const putSizes = (list, countPtr, sizePtr) => {
    const view = dv();
    view.setUint32(countPtr >>> 0, list.length, true);
    view.setUint32(sizePtr >>> 0, list.reduce((n, x) => n + x.length, 0), true);
    return ERRNO.SUCCESS;
  };

  const nowNs = () => BigInt(Math.round((performance.timeOrigin + performance.now()) * 1e6));
  const sleeper = new Int32Array(new SharedArrayBuffer(4));

  const wasi = {
    args_get: (ptrs, buf) => putList(argv, ptrs, buf),
    args_sizes_get: (c, s) => putSizes(argv, c, s),
    environ_get: (ptrs, buf) => putList(envv, ptrs, buf),
    environ_sizes_get: (c, s) => putSizes(envv, c, s),
    clock_res_get(_id, out) {
      dv().setBigUint64(out >>> 0, 1000n, true);
      return ERRNO.SUCCESS;
    },
    clock_time_get(id, _precision, out) {
      // 0 realtime, 1 monotonic, 2/3 process and thread CPU time (the clock will do).
      const t = id === 0 ? nowNs() : BigInt(Math.round(performance.now() * 1e6));
      dv().setBigUint64(out >>> 0, t, true);
      return ERRNO.SUCCESS;
    },
    random_get(buf, len) {
      // getRandomValues refuses shared memory, and takes at most 64 KiB at a time.
      const tmp = new Uint8Array(len >>> 0);
      for (let i = 0; i < tmp.length; i += 65536) crypto.getRandomValues(tmp.subarray(i, i + 65536));
      u8().set(tmp, buf >>> 0);
      return ERRNO.SUCCESS;
    },
    poll_oneoff(inPtr, outPtr, count, neventsPtr) {
      // Clock subscriptions only (sleeps): wait for the earliest, then report every clock.
      const view = dv();
      let timeoutNs = null;
      const events = [];
      for (let i = 0; i < count; i++) {
        const sub = (inPtr >>> 0) + i * 48;
        const userdata = view.getBigUint64(sub, true);
        const tag = view.getUint8(sub + 8);
        if (tag === 0) {
          const clock = view.getUint32(sub + 16, true);
          let t = view.getBigUint64(sub + 24, true);
          const absolute = view.getUint16(sub + 40, true) & 1;
          if (absolute) t -= clock === 0 ? nowNs() : BigInt(Math.round(performance.now() * 1e6));
          const ns = Number(t < 0n ? 0n : t);
          timeoutNs = timeoutNs === null ? ns : Math.min(timeoutNs, ns);
          events.push([userdata, 0]);
        } else {
          events.push([userdata, tag, ERRNO.NOSYS]);
        }
      }
      if (timeoutNs !== null) Atomics.wait(sleeper, 0, 0, timeoutNs / 1e6);
      events.forEach(([userdata, type, error = 0], i) => {
        const ev = (outPtr >>> 0) + i * 32;
        view.setBigUint64(ev, userdata, true);
        view.setUint16(ev + 8, error, true);
        view.setUint8(ev + 10, type);
      });
      view.setUint32(neventsPtr >>> 0, events.length, true);
      return ERRNO.SUCCESS;
    },
    proc_exit(code) {
      throw new ProcExit(code);
    },
    proc_raise: () => ERRNO.NOSYS,
    sched_yield: () => ERRNO.SUCCESS,
    sock_accept: () => ERRNO.NOSYS,
    sock_recv: () => ERRNO.NOSYS,
    sock_send: () => ERRNO.NOSYS,
    sock_shutdown: () => ERRNO.NOSYS,
  };
  FS_CALLS.forEach((name, n) => (wasi[name] = fsCall(n)));

  const counterI32 = new Int32Array(counter);
  const imports = {
    env: { memory },
    wasi_snapshot_preview1: wasi,
    wasi: {
      // A new thread's id; the page starts its worker. The id must be positive and below 2^29.
      "thread-spawn": (arg) => {
        if (Atomics.add(counterI32, 1, 1) >= maxThreads) {
          Atomics.sub(counterI32, 1, 1);
          return -ERRNO.AGAIN;
        }
        const id = (Atomics.add(counterI32, 0, 1) % 0x1ffffffe) + 1;
        postMessage({ type: "spawn", tid: id, startArg: arg });
        return id;
      },
    },
  };

  const instance = new WebAssembly.Instance(module, imports);
  const start = (id, arg) => {
    instance.exports.wasi_thread_start(id, arg);
    Atomics.sub(counterI32, 1, 1);
    postMessage({ type: "thread-done", tid: id });
  };
  if (tid === 0) {
    instance.exports._start();
    postMessage({ type: "exit", code: 0 });
  } else {
    start(tid, startArg);
  }
  return start;
}
