// The IDE: Monaco on the left, the app on the right, the toolchain in this tab.
//
// Build & Run: the project's files are already in the in-tab file system (every edit is written there,
// and kept in IndexedDB across reloads); rustc 1.98.0, compiled to WebAssembly, compiles Caprice for
// wasm32-unknown-unknown against prebuilt dependencies; wasm-bindgen (also WebAssembly) makes the JS
// glue; the preview frame starts the result. The MCP bridge reaches all of this over a WebSocket.

import { Toolchain } from "./runtime/toolchain.js";

const $ = (id) => document.getElementById(id);
const ROOT = "/project";
const params = new URLSearchParams(location.search);

const state = {
  tc: null,
  build: null,
  monaco: null,
  editor: null,
  models: new Map(), // path -> monaco model
  tabs: [], // open paths
  active: null,
  files: [], // project file paths
  edited: new Set(), // paths that differ from the project as shipped
  pristine: new Map(), // path -> shipped text (for edited files)
  building: null, // the running build's promise
  lastBuild: null,
  diagnostics: [],
  app: null, // { js, wasm } of the last good build
  appRun: 0,
  appState: "idle",
  appLogs: [],
  previewWaiters: new Map(),
  nextPreviewId: 1,
};

// --- Small helpers --------------------------------------------------------------------------------

const normalize = (path) => {
  path = String(path || "").trim();
  if (!path.startsWith("/")) path = `${ROOT}/${path}`;
  return path.replace(/\/+/g, "/").replace(/\/$/, "");
};
const relative = (path) => (path.startsWith(`${ROOT}/`) ? path.slice(ROOT.length + 1) : path);
const isText = (path) => /\.(rs|toml|lock|md|txt|json|svg|ron|css|html|js|yml|yaml)$/i.test(path);
const escapeHtml = (s) => s.replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c]);
const languageOf = (path) =>
  path.endsWith(".rs") ? "rust" : /\.(toml|lock)$/.test(path) ? "ini" : path.endsWith(".json") ? "json" :
  path.endsWith(".md") ? "markdown" : path.endsWith(".svg") ? "xml" : "plaintext";

function setStatus(html) {
  $("status").innerHTML = html;
}

function overlay(title, detail = "", fraction = null) {
  $("app-overlay").classList.remove("hidden");
  $("overlay-title").textContent = title;
  $("overlay-detail").textContent = detail;
  const bar = $("progress-bar");
  bar.parentElement.classList.toggle("indeterminate", fraction === null);
  bar.style.width = fraction === null ? "" : `${Math.round(fraction * 100)}%`;
}
const hideOverlay = () => $("app-overlay").classList.add("hidden");

// --- Output panel (with rustc's ANSI colours) ------------------------------------------------------

const ANSI = { 31: "red", 32: "green", 33: "yellow", 34: "blue", 35: "magenta", 36: "cyan", 91: "red", 92: "green", 93: "yellow", 94: "blue", 95: "magenta", 96: "cyan" };

function ansiToHtml(text) {
  let html = "";
  let classes = [];
  const parts = text.split(/\x1b\[([0-9;]*)m/);
  for (let i = 0; i < parts.length; i++) {
    if (i % 2 === 0) {
      const t = escapeHtml(parts[i]);
      html += classes.length && t ? `<span class="${classes.join(" ")}">${t}</span>` : t;
    } else {
      const codes = parts[i].split(";").map(Number);
      for (let j = 0; j < codes.length; j++) {
        const c = codes[j];
        if (c === 0 || parts[i] === "") classes = [];
        else if (c === 1) classes.push("ansi-bold");
        else if (ANSI[c]) classes = classes.filter((x) => !x.startsWith("ansi-") || x === "ansi-bold").concat(`ansi-${ANSI[c]}`);
        else if (c === 38 && codes[j + 1] === 5) {
          const n = codes[j + 2];
          if (ANSI[30 + (n % 8)]) classes.push(`ansi-${ANSI[30 + (n % 8)]}`);
          j += 2;
        } else if (c === 39) classes = classes.filter((x) => x === "ansi-bold");
      }
    }
  }
  return html;
}

const out = {
  clear() {
    $("output").innerHTML = "";
  },
  html(h) {
    const el = $("output");
    const atBottom = el.scrollTop + el.clientHeight >= el.scrollHeight - 20;
    el.insertAdjacentHTML("beforeend", h);
    if (atBottom) el.scrollTop = el.scrollHeight;
  },
  step(text) {
    this.html(`<span class="out-step">› ${escapeHtml(text)}</span>\n`);
  },
  ansi(text) {
    this.html(ansiToHtml(text));
  },
  result(ok, text) {
    this.html(`<span class="${ok ? "out-ok" : "out-err"}">${escapeHtml(text)}</span>\n`);
  },
};

function showPanel(name) {
  for (const el of document.querySelectorAll("[data-panel]")) el.classList.toggle("active", el.dataset.panel === name);
}

// --- Persistence of edits (IndexedDB) -------------------------------------------------------------

const db = {
  open() {
    if (this.handle) return this.handle;
    this.handle = new Promise((resolve, reject) => {
      const req = indexedDB.open("rust-browser-ide", 1);
      req.onupgradeneeded = () => req.result.createObjectStore("edits");
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
    });
    return this.handle;
  },
  async tx(mode, fn) {
    const d = await this.open();
    return new Promise((resolve, reject) => {
      const t = d.transaction("edits", mode);
      const result = fn(t.objectStore("edits"));
      t.oncomplete = () => resolve(result?.result);
      t.onerror = () => reject(t.error);
    });
  },
  all() {
    return this.tx("readonly", (s) => s.getAll()).then(async (values) => {
      const keys = await this.tx("readonly", (s) => s.getAllKeys());
      return keys.map((k, i) => [k, values[i]]);
    });
  },
  put: (path, text) => db.tx("readwrite", (s) => s.put(text, path)),
  delete: (path) => db.tx("readwrite", (s) => s.delete(path)),
  clear: () => db.tx("readwrite", (s) => s.clear()),
};

// --- Monaco ---------------------------------------------------------------------------------------

function loadMonaco() {
  return new Promise((resolve) => {
    window.MonacoEnvironment = { getWorkerUrl: () => "vendor/monaco/vs/base/worker/workerMain.js" };
    window.require.config({ paths: { vs: "vendor/monaco/vs" } });
    window.require(["vs/editor/editor.main"], () => resolve(window.monaco));
  });
}

function setupEditor(monaco) {
  monaco.editor.defineTheme("ide", {
    base: "vs-dark",
    inherit: true,
    rules: [],
    colors: { "editor.background": "#16181d", "editorGutter.background": "#16181d", "editor.lineHighlightBackground": "#1d2129" },
  });
  const editor = monaco.editor.create($("editor"), {
    theme: "ide",
    automaticLayout: true,
    fontSize: 13,
    fontFamily: "'JetBrains Mono', 'Fira Code', ui-monospace, Menlo, monospace",
    minimap: { enabled: true, scale: 1, renderCharacters: false },
    rulers: [130],
    scrollBeyondLastLine: false,
    smoothScrolling: true,
    tabSize: 4,
    renderWhitespace: "selection",
  });
  editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter, () => buildAndRun());
  document.addEventListener("keydown", (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
      e.preventDefault();
      buildAndRun();
    }
  });
  editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => flushSaves().then(() => setStatus("Saved in this tab")));
  return editor;
}

const saveTimers = new Map();

async function modelFor(path) {
  if (state.models.has(path)) return state.models.get(path);
  const text = await state.tc.readFile(path, true);
  const model = state.monaco.editor.createModel(text, languageOf(path), state.monaco.Uri.file(path));
  model.onDidChangeContent(() => {
    clearTimeout(saveTimers.get(path));
    saveTimers.set(path, setTimeout(() => saveModel(path), 400));
  });
  state.models.set(path, model);
  return model;
}

async function saveModel(path) {
  saveTimers.delete(path);
  const model = state.models.get(path);
  if (!model) return;
  const text = model.getValue();
  await state.tc.writeFile(path, text);
  await markEdited(path, text);
}

async function markEdited(path, text) {
  if (!state.pristine.has(path) && !state.edited.has(path)) {
    // The first change: what it was as shipped is in the project pack, unchanged until now.
    state.pristine.set(path, state.shipped.get(path) ?? null);
  }
  const shipped = state.pristine.get(path);
  if (text === shipped) {
    state.edited.delete(path);
    await db.delete(path);
  } else {
    state.edited.add(path);
    await db.put(path, text);
  }
  renderTree();
}

async function flushSaves() {
  await Promise.all([...saveTimers.keys()].map((p) => {
    clearTimeout(saveTimers.get(p));
    return saveModel(p);
  }));
}

async function openFile(path, line = null, column = 1) {
  path = normalize(path);
  if (!isText(path)) {
    setStatus(`${escapeHtml(relative(path))} isn't a text file`);
    return;
  }
  const model = await modelFor(path);
  if (!state.tabs.includes(path)) state.tabs.push(path);
  state.active = path;
  state.editor.setModel(model);
  if (line) {
    state.editor.revealLineInCenter(line);
    state.editor.setPosition({ lineNumber: line, column });
    state.editor.focus();
  }
  renderTabs();
  renderTree();
}

function closeTab(path) {
  const i = state.tabs.indexOf(path);
  if (i < 0) return;
  state.tabs.splice(i, 1);
  if (state.active === path) {
    const next = state.tabs[Math.min(i, state.tabs.length - 1)];
    if (next) openFile(next);
    else {
      state.active = null;
      state.editor.setModel(null);
    }
  }
  renderTabs();
}

function renderTabs() {
  const tabs = $("tabs");
  tabs.innerHTML = "";
  for (const path of state.tabs) {
    const tab = document.createElement("div");
    tab.className = `tab${path === state.active ? " active" : ""}${state.edited.has(path) ? " dirty" : ""}`;
    tab.title = relative(path);
    tab.innerHTML = `<span>${escapeHtml(path.split("/").pop())}</span><span class="close"></span>`;
    tab.onclick = (e) => (e.target.classList.contains("close") ? closeTab(path) : openFile(path));
    tabs.appendChild(tab);
  }
}

// --- File tree ------------------------------------------------------------------------------------

const closedDirs = new Set(["assets"]);

function renderTree() {
  const tree = {};
  for (const path of state.files) {
    let node = tree;
    const parts = relative(path).split("/");
    parts.forEach((part, i) => {
      if (i === parts.length - 1) node[part] = path;
      else node = node[part] ??= {};
    });
  }
  const errors = new Set(state.diagnostics.filter((d) => d.level === "error" && d.file).map((d) => d.file));
  const render = (node, depth, prefix) => {
    const entries = Object.entries(node).sort(([a, x], [b, y]) =>
      (typeof x === "string") - (typeof y === "string") || a.localeCompare(b));
    let html = "";
    for (const [name, value] of entries) {
      const pad = `style="padding-left:${8 + depth * 12}px"`;
      if (typeof value === "string") {
        const badge = errors.has(value) ? "err" : state.edited.has(value) ? "edited" : "";
        html += `<div class="row${value === state.active ? " active" : ""}" data-path="${value}" ${pad}>
          <span class="twist"></span><span class="name">${escapeHtml(name)}</span>${badge ? `<span class="badge ${badge}"></span>` : ""}</div>`;
      } else {
        const dir = prefix ? `${prefix}/${name}` : name;
        const closed = closedDirs.has(dir);
        html += `<div class="dir${closed ? " closed" : ""}"><div class="row" data-dir="${dir}" ${pad}>
          <span class="twist">${closed ? "▶" : "▼"}</span><span class="name">${escapeHtml(name)}</span></div>
          <div class="children">${render(value, depth + 1, dir)}</div></div>`;
      }
    }
    return html;
  };
  $("tree").innerHTML = render(tree, 0, "");
}

$("tree").addEventListener("click", (e) => {
  const row = e.target.closest(".row");
  if (!row) return;
  if (row.dataset.path) openFile(row.dataset.path);
  else if (row.dataset.dir) {
    const dir = row.dataset.dir;
    if (closedDirs.has(dir)) closedDirs.delete(dir);
    else closedDirs.add(dir);
    renderTree();
  }
});

// --- Diagnostics ----------------------------------------------------------------------------------

function diagnosticOf(json) {
  const primary = (json.spans || []).find((s) => s.is_primary) || (json.spans || [])[0];
  return {
    level: json.level,
    message: json.message,
    code: json.code?.code ?? null,
    file: primary ? normalize(primary.file_name) : null,
    line: primary?.line_start ?? null,
    column: primary?.column_start ?? null,
    endLine: primary?.line_end ?? null,
    endColumn: primary?.column_end ?? null,
    rendered: json.rendered ?? json.message,
  };
}

function showDiagnostics() {
  const { monaco } = state;
  const byFile = new Map();
  for (const d of state.diagnostics) {
    if (!d.file || !d.line) continue;
    if (!byFile.has(d.file)) byFile.set(d.file, []);
    byFile.get(d.file).push({
      severity: d.level === "error" ? monaco.MarkerSeverity.Error : d.level === "warning" ? monaco.MarkerSeverity.Warning : monaco.MarkerSeverity.Info,
      message: d.message + (d.code ? ` [${d.code}]` : ""),
      startLineNumber: d.line, startColumn: d.column, endLineNumber: d.endLine, endColumn: d.endColumn,
    });
  }
  for (const model of monaco.editor.getModels()) {
    monaco.editor.setModelMarkers(model, "rustc", byFile.get(model.uri.path) ?? []);
  }
  state.pendingMarkers = byFile;
  const list = state.diagnostics.filter((d) => d.level === "error" || d.level === "warning");
  $("problem-count").textContent = list.length || "";
  $("problems").innerHTML = list.length
    ? list.map((d, i) => `<div class="problem" data-i="${i}"><span class="sev ${d.level}">${d.level}</span>
        <span>${escapeHtml(d.message)}</span><span class="where">${d.file ? `${escapeHtml(relative(d.file))}:${d.line}` : ""}</span></div>`).join("")
    : `<div class="empty">No problems.</div>`;
  $("problems").onclick = (e) => {
    const row = e.target.closest(".problem");
    if (!row) return;
    const d = list[Number(row.dataset.i)];
    if (d.file) openFile(d.file, d.line, d.column);
  };
  renderTree();
}

// --- Build & run ----------------------------------------------------------------------------------

// Threads' stacks are memory rustc allocates; on wasm, running past one corrupts the heap instead of
// trapping, and LLVM's codegen threads recurse deeply. 16 MiB each, like a native main thread.
const RUSTC_ENV = { RUST_BACKTRACE: "0", RUST_MIN_STACK: String(16 << 20), CARGO_PKG_NAME: "caprice", CARGO_CRATE_NAME: "caprice" };

// Builds, and starts the app if `start`. One build at a time: a second request waits for the first.
function buildAndRun({ start = true } = {}) {
  if (state.building) return state.building;
  state.building = runBuild({ start }).finally(() => {
    state.building = null;
    $("run").disabled = false;
    $("run").classList.remove("busy");
    $("run").querySelector("span").textContent = "Build & Run";
  });
  return state.building;
}

async function runBuild({ start = true } = {}) {
  const { tc, build } = state;
  $("run").disabled = true;
  $("run").classList.add("busy");
  $("run").querySelector("span").textContent = "Building…";
  await flushSaves();
  out.clear();
  showPanel("output");
  const t0 = performance.now();
  const ticker = setInterval(() => setStatus(`Compiling caprice… ${Math.round((performance.now() - t0) / 1000)} s`), 500);
  setStatus("Compiling caprice…");
  out.step(`rustc ${build.rustc.slice(0, 6).join(" ")} …  (rustc 1.98.0, running as WebAssembly in this tab)`);

  const diagnostics = [];
  let pending = "";
  const onOutput = (fd, text) => {
    if (fd !== 2) return out.ansi(text);
    pending += text;
    const lines = pending.split("\n");
    pending = lines.pop();
    for (const line of lines) {
      if (line.startsWith("{")) {
        try {
          const json = JSON.parse(line);
          if (json.$message_type === "diagnostic" || json.level) {
            diagnostics.push(diagnosticOf(json));
            out.ansi(json.rendered ?? `${json.level}: ${json.message}\n`);
            continue;
          }
        } catch {}
      }
      out.ansi(`${line}\n`);
    }
  };

  let rustc;
  try {
    rustc = await tc.run("rustc", { args: build.rustc, env: RUSTC_ENV, onOutput });
  } finally {
    clearInterval(ticker);
  }
  if (pending) onOutput(2, "\n");
  state.diagnostics = diagnostics;
  showDiagnostics();
  const errors = diagnostics.filter((d) => d.level === "error").length;
  const warnings = diagnostics.filter((d) => d.level === "warning").length;
  const rustcMs = rustc.ms;

  const result = { ok: false, errors, warnings, rustcMs, bindgenMs: null, ms: null, crash: null };
  // rustc ends with a trap after reporting errors (it's built with panic=abort); that is a failed
  // build, not a crash. Without errors, a trap is a crash.
  if (rustc.code !== 0) {
    result.crash = errors ? null : rustc.crash ?? `exit code ${rustc.code}`;
    out.result(false, errors ? `error: could not compile caprice (${errors} error${errors > 1 ? "s" : ""}, ${warnings} warnings)` : `rustc crashed: ${result.crash}`);
    if (errors) showPanel("problems");
    setStatus(`<span class="err">Build failed</span> · ${errors} error${errors === 1 ? "" : "s"}, ${warnings} warning${warnings === 1 ? "" : "s"}`);
    return finishBuild(result, t0);
  }
  out.result(true, `compiled in ${(rustcMs / 1000).toFixed(1)} s${warnings ? ` with ${warnings} warnings` : ""}`);

  setStatus("Generating JS bindings…");
  out.step(`wasm-bindgen ${build.bindgen.join(" ")}`);
  const bindgen = await tc.run("wasm-bindgen", { args: build.bindgen, onOutput: (_fd, t) => out.ansi(t) });
  result.bindgenMs = bindgen.ms;
  if (bindgen.code !== 0) {
    result.crash = `wasm-bindgen: ${bindgen.crash ?? `exit code ${bindgen.code}`}`;
    out.result(false, result.crash);
    setStatus(`<span class="err">wasm-bindgen failed</span>`);
    return finishBuild(result, t0);
  }
  const [js, wasm] = await Promise.all([tc.readFile(build.js, true), tc.readFile(build.wasm)]);
  result.ok = true;
  result.wasmBytes = wasm.length;
  out.result(true, `bindings in ${(bindgen.ms / 1000).toFixed(1)} s · caprice_bg.wasm ${(wasm.length / 2 ** 20).toFixed(1)} MiB`);
  state.app = { js, wasm };
  if (start) startApp();
  setStatus(`<span class="ok">Built</span> in ${((performance.now() - t0) / 1000).toFixed(1)} s · ${warnings} warning${warnings === 1 ? "" : "s"}`);
  return finishBuild(result, t0);
}

function finishBuild(result, t0) {
  result.ms = Math.round(performance.now() - t0);
  state.lastBuild = { ...result, at: new Date().toISOString() };
  return state.lastBuild;
}

// --- The app's frame ------------------------------------------------------------------------------

function startApp() {
  if (!state.app) return;
  state.appRun++;
  state.appLogs = [];
  $("console").textContent = "";
  $("console-count").textContent = "";
  setAppState("starting");
  $("preview").src = `preview.html?run=${state.appRun}`;
  $("restart").disabled = false;
}

function setAppState(s) {
  state.appState = s;
  $("app-state").textContent = { starting: "starting…", running: "running in this tab", crashed: "crashed — see App console", idle: "not built yet" }[s] ?? s;
}

// Resolves once the app runs (or has crashed, or doesn't start within `ms`).
async function appSettled(ms = 20000) {
  const until = performance.now() + ms;
  while (state.appState === "starting" && performance.now() < until) await new Promise((r) => setTimeout(r, 100));
  return state.appState;
}

async function previewRequest(method, params = {}, timeout = 15000) {
  await appSettled();
  if (state.appState !== "running") throw new Error(`the app isn't running (it is ${state.appState}); build it first`);
  const id = state.nextPreviewId++;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      state.previewWaiters.delete(id);
      reject(new Error(`the app didn't answer ${method}`));
    }, timeout);
    state.previewWaiters.set(id, { resolve, reject, timer });
    $("preview").contentWindow.postMessage({ type: "request", id, method, params }, location.origin);
  });
}

addEventListener("message", (e) => {
  if (e.origin !== location.origin || e.data?.source !== "preview") return;
  if (e.source !== $("preview").contentWindow) return;
  const msg = e.data;
  if (msg.type === "ready" && state.app) {
    const wasm = state.app.wasm.slice();
    $("preview").contentWindow.postMessage({ type: "start", js: state.app.js, wasm }, location.origin, [wasm.buffer]);
  } else if (msg.type === "started") {
    setAppState("running");
  } else if (msg.type === "crashed") {
    setAppState("crashed");
  } else if (msg.type === "console") {
    const entry = { level: msg.level, text: msg.text, at: Date.now() };
    state.appLogs.push(entry);
    if (state.appLogs.length > 2000) state.appLogs.shift();
    const cls = msg.level === "error" ? "log-error" : msg.level === "warn" ? "log-warn" : "";
    $("console").insertAdjacentHTML("beforeend", `<span class="${cls}">${escapeHtml(msg.text)}</span>\n`);
    $("console-count").textContent = state.appLogs.length;
  } else if (msg.type === "reply") {
    const w = state.previewWaiters.get(msg.id);
    if (!w) return;
    clearTimeout(w.timer);
    state.previewWaiters.delete(msg.id);
    if (msg.error) w.reject(new Error(msg.error));
    else w.resolve(msg.result);
  }
});

// --- Project files --------------------------------------------------------------------------------

async function loadProject() {
  const listing = await state.tc.request("walk", { path: ROOT });
  state.files = listing.map((f) => f.path).sort();
  state.shipped = new Map();
  for (const path of state.files.filter(isText)) state.shipped.set(path, await state.tc.readFile(path, true));
  // Edits from earlier visits.
  for (const [path, text] of await db.all()) {
    state.pristine.set(path, state.shipped.get(path) ?? null);
    if (text === null) continue;
    await state.tc.writeFile(path, text);
    state.edited.add(path);
    if (!state.files.includes(path)) state.files.push(path);
  }
  state.files.sort();
}

async function resetProject() {
  for (const path of state.edited) {
    const shipped = state.shipped.get(path);
    if (shipped === undefined) {
      await state.tc.request("remove", { path });
      state.files = state.files.filter((p) => p !== path);
    } else {
      await state.tc.writeFile(path, shipped);
    }
    const model = state.models.get(path);
    if (model) {
      if (shipped === undefined) {
        closeTab(path);
        model.dispose();
        state.models.delete(path);
      } else {
        model.setValue(shipped);
      }
    }
  }
  await flushSaves();
  state.edited.clear();
  state.pristine.clear();
  await db.clear();
  renderTree();
  renderTabs();
}

// --- The MCP bridge -------------------------------------------------------------------------------

const tools = {
  async status() {
    return {
      ready: !!state.build,
      building: !!state.building,
      app: state.appState,
      lastBuild: state.lastBuild,
      openFile: state.active && relative(state.active),
      editedFiles: [...state.edited].map(relative),
      fileCount: state.files.length,
    };
  },
  async list_files({ path = "" }) {
    const prefix = normalize(path || ROOT);
    return state.files.filter((p) => p === prefix || p.startsWith(`${prefix}/`)).map((p) => ({
      path: relative(p), edited: state.edited.has(p),
    }));
  },
  async read_file({ path, start_line, end_line }) {
    path = normalize(path);
    const model = state.models.get(path);
    const text = model ? model.getValue() : await state.tc.readFile(path, true);
    if (!start_line && !end_line) return { path: relative(path), text, lines: text.split("\n").length };
    const lines = text.split("\n");
    const from = Math.max(1, start_line || 1);
    const to = Math.min(lines.length, end_line || lines.length);
    return { path: relative(path), from, to, lines: lines.length, text: lines.slice(from - 1, to).join("\n") };
  },
  async write_file({ path, content }) {
    path = normalize(path);
    if (!path.startsWith(`${ROOT}/`)) throw new Error("files live under the project (src/…, Cargo.toml)");
    const isNew = !state.files.includes(path);
    if (isNew) {
      state.files.push(path);
      state.files.sort();
    }
    const model = state.models.get(path);
    if (model) {
      if (model.getValue() !== content) model.pushEditOperations([], [{ range: model.getFullModelRange(), text: content }], () => null);
      clearTimeout(saveTimers.get(path));
      saveTimers.delete(path);
    }
    await state.tc.writeFile(path, content);
    await markEdited(path, content);
    await openFile(path);
    return { path: relative(path), created: isNew, bytes: new TextEncoder().encode(content).length };
  },
  async edit_file({ path, old_text, new_text, replace_all = false }) {
    const { text } = await tools.read_file({ path });
    const count = text.split(old_text).length - 1;
    if (count === 0) throw new Error(`old_text not found in ${path}`);
    if (count > 1 && !replace_all) throw new Error(`old_text occurs ${count} times in ${path}; make it unique or set replace_all`);
    const updated = replace_all ? text.split(old_text).join(new_text) : text.replace(old_text, () => new_text);
    await tools.write_file({ path, content: updated });
    const line = text.slice(0, text.indexOf(old_text)).split("\n").length;
    await openFile(normalize(path), line);
    return { path, replaced: replace_all ? count : 1, line };
  },
  async delete_file({ path }) {
    path = normalize(path);
    if (!state.files.includes(path)) throw new Error(`no such file: ${relative(path)}`);
    await state.tc.request("remove", { path });
    state.files = state.files.filter((p) => p !== path);
    closeTab(path);
    state.models.get(path)?.dispose();
    state.models.delete(path);
    if (state.shipped.has(path)) {
      state.pristine.set(path, state.shipped.get(path));
      state.edited.add(path);
      await db.put(path, null);
    } else {
      state.edited.delete(path);
      await db.delete(path);
    }
    renderTree();
    return { deleted: relative(path) };
  },
  async search({ pattern, regex = false, path = "" }) {
    const re = regex ? new RegExp(pattern) : null;
    const hits = [];
    for (const p of (await tools.list_files({ path })).map((f) => normalize(f.path)).filter(isText)) {
      const { text } = await tools.read_file({ path: p });
      text.split("\n").forEach((line, i) => {
        if (re ? re.test(line) : line.includes(pattern)) hits.push({ path: relative(p), line: i + 1, text: line.trim().slice(0, 200) });
      });
      if (hits.length > 500) break;
    }
    return { hits: hits.slice(0, 500), truncated: hits.length > 500 };
  },
  async build({ run = true }) {
    const result = await buildAndRun({ start: run });
    if (run && result.ok) result.app = await appSettled();
    return { ...result, diagnostics: tools.get_diagnostics_list() };
  },
  get_diagnostics_list() {
    return state.diagnostics
      .filter((d) => d.level === "error" || d.level === "warning")
      .map((d) => ({ level: d.level, code: d.code, message: d.message, file: d.file && relative(d.file), line: d.line, column: d.column, rendered: d.rendered.replace(/\x1b\[[0-9;]*m/g, "") }));
  },
  async get_diagnostics() {
    return { lastBuild: state.lastBuild, diagnostics: tools.get_diagnostics_list() };
  },
  async open_file({ path, line }) {
    await openFile(path, line);
    return { open: relative(normalize(path)) };
  },
  async screenshot() {
    return previewRequest("screenshot");
  },
  async app_console({ clear = false, last = 200 }) {
    const logs = state.appLogs.slice(-last);
    if (clear) {
      state.appLogs = [];
      $("console").textContent = "";
      $("console-count").textContent = "";
    }
    return { state: state.appState, logs };
  },
  app_size: () => previewRequest("size"),
  app_click: (p) => previewRequest("click", p),
  app_drag: (p) => previewRequest("drag", p),
  app_scroll: (p) => previewRequest("scroll", p),
  app_key: (p) => previewRequest("key", p),
  app_type: (p) => previewRequest("type", p, 60000),
  async restart_app() {
    if (!state.app) throw new Error("nothing built yet");
    startApp();
    return { app: await appSettled() };
  },
  async reset_project() {
    const edited = [...state.edited].map(relative);
    await resetProject();
    return { reset: edited };
  },
};

function connectBridge() {
  const el = $("bridge");
  let ws;
  try {
    ws = new WebSocket(`ws://${location.host}/ws`);
  } catch {
    setTimeout(connectBridge, 3000);
    return;
  }
  ws.onopen = () => {
    el.classList.add("on");
    el.querySelector(".label").textContent = "bridge connected";
  };
  ws.onclose = () => {
    el.classList.remove("on", "active");
    el.querySelector(".label").textContent = "bridge";
    setTimeout(connectBridge, 2000);
  };
  ws.onmessage = async (e) => {
    const { id, method, params } = JSON.parse(e.data);
    el.classList.add("active");
    try {
      if (!tools[method] || method.endsWith("_list")) throw new Error(`unknown method ${method}`);
      if (!state.build && method !== "status") throw new Error("the IDE is still loading its toolchain");
      const result = await tools[method](params ?? {});
      ws.send(JSON.stringify({ id, result }));
    } catch (err) {
      ws.send(JSON.stringify({ id, error: String(err?.message ?? err) }));
    } finally {
      el.classList.remove("active");
    }
  };
}

// --- Layout ---------------------------------------------------------------------------------------

function draggable(handle, onMove) {
  handle.addEventListener("pointerdown", (e) => {
    handle.setPointerCapture(e.pointerId);
    handle.classList.add("dragging");
    $("preview").style.pointerEvents = "none";
    const move = (ev) => onMove(ev);
    const up = () => {
      handle.classList.remove("dragging");
      $("preview").style.pointerEvents = "";
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
  });
}
draggable($("split"), (e) => {
  const w = Math.min(Math.max(e.clientX, 380), innerWidth - 320);
  $("code").style.flexBasis = `${w}px`;
});
draggable($("panel-resizer"), (e) => {
  const col = document.querySelector(".editor-column").getBoundingClientRect();
  const h = Math.min(Math.max(col.bottom - e.clientY, 60), col.height - 120);
  $("panel").style.flexBasis = `${h}px`;
});
for (const b of document.querySelectorAll(".panel-tabs button")) b.onclick = () => showPanel(b.dataset.panel);
$("run").onclick = () => buildAndRun();
$("restart").onclick = () => startApp();
$("reset").onclick = async () => {
  if (!state.edited.size) return setStatus("Nothing to reset: the project is as shipped");
  if (confirm(`Discard your edits to ${state.edited.size} file(s) and go back to Caprice as shipped?`)) {
    await resetProject();
    setStatus("Project reset");
  }
};

// --- Start ----------------------------------------------------------------------------------------

async function boot() {
  if (!crossOriginIsolated) {
    overlay("This page needs cross-origin isolation", "Open it through the bridge: python bridge/ide_bridge.py --serve-only, then http://127.0.0.1:8765/", 0);
    setStatus(`<span class="err">Not cross-origin isolated</span>`);
    return;
  }
  overlay("Loading the toolchain", "rustc 1.98.0 for WebAssembly, the std for wasm32-unknown-unknown, Caprice's dependencies");
  connectBridge();
  const monacoReady = loadMonaco();
  const tc = (state.tc = new Toolchain());
  const [manifest, build] = await Promise.all([
    fetch("toolchain/manifest.json").then((r) => r.json()),
    fetch("toolchain/build.json").then((r) => r.json()),
  ]);
  const programs = Object.entries(manifest.programs).map(([name, p]) => tc.load(name, p.url, p));
  const progress = new Map();
  const report = () => {
    let got = 0;
    let total = 0;
    for (const [r, t] of progress.values()) {
      got += r;
      total += t;
    }
    const mb = (n) => (n / 2 ** 20).toFixed(0);
    overlay("Loading the toolchain", `${mb(got)} of ${mb(total)} MiB · compiling rustc.wasm`, total ? got / total : null);
  };
  await Promise.all([
    ...programs,
    ...manifest.packs.map((p) => tc.mountPack(p.url, p.at, (r, t) => {
      progress.set(p.url, [r, t]);
      report();
    })),
  ]);
  if (!manifest.programs.rustc) throw new Error("the toolchain has no rustc.wasm yet (run tools/build_assets.py)");
  overlay("Opening the project");
  state.monaco = await monacoReady;
  state.editor = setupEditor(state.monaco);
  await loadProject();
  renderTree();
  await openFile(`${ROOT}/src/main.rs`);
  state.build = build;
  $("run").disabled = false;
  setStatus("Ready · Ctrl+Enter builds and runs");
  overlay("Ready", "Press Build & Run (Ctrl+Enter) to compile Caprice in this tab", 1);
  if (!params.has("noautorun")) {
    overlay("Building Caprice", "rustc is compiling it in this tab; the first build takes a minute or two", null);
    const result = await buildAndRun();
    if (result.ok) hideOverlay();
    else overlay("The build failed", "See Output and Problems on the left", 1);
  }
}

// When the app frame starts, hide the overlay.
new MutationObserver(() => {
  if (state.appState === "running") hideOverlay();
}).observe($("app-state"), { childList: true, characterData: true, subtree: true });

boot().catch((err) => {
  console.error(err);
  overlay("The IDE couldn't start", String(err?.message ?? err), 0);
  setStatus(`<span class="err">${escapeHtml(String(err?.message ?? err))}</span>`);
});

window.ide = { state, tools, buildAndRun };
