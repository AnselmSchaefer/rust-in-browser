"""The MCP tools: each forwards to the IDE page (``tools`` in web/ide.js) and shapes its answer for Claude."""

from __future__ import annotations

import base64
import json
from pathlib import Path
from typing import Any, Protocol

from mcp.server.mcpserver import Image, MCPServer

INSTRUCTIONS = """\
A Rust IDE running in the user's browser (Monaco on the left, the app on the right). Its whole
toolchain runs in the tab as WebAssembly: rustc 1.98.0 compiles the project for wasm32-unknown-unknown
against prebuilt dependencies, wasm-bindgen makes the JS glue, and the app (Caprice, an egui/eframe word
processor) starts in the right pane.

The project's files live in the browser (paths like "src/main.rs", "Cargo.toml"); edit them with
write_file / edit_file, then build to compile and restart the app. Dependencies are fixed (prebuilt):
Cargo.toml changes don't add crates. A first build takes a minute or two; later ones are incremental.
Use app_screenshot to see the app, app_click / app_type / app_key to use it (coordinates are CSS pixels
of the app pane, see app_size), and app_console for its log. sync_to_disk / load_from_disk copy the
project between the browser and a local folder."""


TEXT_SUFFIXES = {".rs", ".toml", ".lock", ".md", ".txt", ".json", ".svg"}


class Link(Protocol):
    async def call(self, method: str, params: dict | None = None, timeout: float = 600) -> Any: ...


def format_build(result: dict) -> str:
    lines = []
    if result.get("ok"):
        lines.append(
            f"Build succeeded in {result['ms'] / 1000:.1f} s (rustc {result['rustcMs'] / 1000:.1f} s, "
            f"wasm-bindgen {(result.get('bindgenMs') or 0) / 1000:.1f} s); "
            f"{result['warnings']} warning(s). "
            + ("The app was restarted." if result.get("app") == "running" else f"The app is {result.get('app', 'not started')}.")
        )
    elif result.get("crash"):
        lines.append(f"Build crashed: {result['crash']}")
    else:
        lines.append(f"Build failed: {result['errors']} error(s), {result['warnings']} warning(s).")
    for d in result.get("diagnostics", []):
        lines.append("")
        lines.append(d["rendered"].rstrip())
    return "\n".join(lines)


def build_server(link: Link) -> MCPServer:
    server = MCPServer(name="rust-browser-ide", instructions=INSTRUCTIONS)

    async def page(method: str, timeout: float = 600, **params: Any) -> Any:
        return await link.call(method, {k: v for k, v in params.items() if v is not None}, timeout)

    @server.tool()
    async def ide_status() -> str:
        """Whether the IDE is loaded, building, and whether the app runs; the last build; edited files."""
        return json.dumps(await page("status", timeout=30), indent=1)

    @server.tool()
    async def list_files(path: str = "") -> str:
        """Lists the project's files (optionally under a directory such as "src"). `edited` marks files
        changed from the project as shipped."""
        files = await page("list_files", path=path)
        return "\n".join(f"{f['path']}{'  (edited)' if f['edited'] else ''}" for f in files)

    @server.tool()
    async def read_file(path: str, start_line: int | None = None, end_line: int | None = None) -> str:
        """Reads a project file as the editor has it (with unsaved changes), optionally a range of lines
        (1-based, inclusive). Lines are numbered in the output."""
        r = await page("read_file", path=path, start_line=start_line, end_line=end_line)
        first = r.get("from", 1)
        body = "\n".join(f"{first + i:5}\t{line}" for i, line in enumerate(r["text"].split("\n")))
        return f"{r['path']} ({r['lines']} lines)\n{body}"

    @server.tool()
    async def write_file(path: str, content: str) -> str:
        """Writes a whole project file (creating it if new) and shows it in the editor."""
        r = await page("write_file", path=path, content=content)
        return f"{'Created' if r['created'] else 'Wrote'} {r['path']} ({r['bytes']} bytes)"

    @server.tool()
    async def edit_file(path: str, old_text: str, new_text: str, replace_all: bool = False) -> str:
        """Replaces `old_text` with `new_text` in a project file. `old_text` must occur exactly once,
        unless `replace_all` is set. The editor shows the change."""
        r = await page("edit_file", path=path, old_text=old_text, new_text=new_text, replace_all=replace_all)
        return f"Replaced {r['replaced']} occurrence(s) in {r['path']} (at line {r['line']})"

    @server.tool()
    async def delete_file(path: str) -> str:
        """Deletes a project file."""
        r = await page("delete_file", path=path)
        return f"Deleted {r['deleted']}"

    @server.tool()
    async def search(pattern: str, regex: bool = False, path: str = "") -> str:
        """Searches the project's text files for a string (or a JavaScript regular expression if `regex`)."""
        r = await page("search", pattern=pattern, regex=regex, path=path)
        lines = [f"{h['path']}:{h['line']}: {h['text']}" for h in r["hits"]]
        if r["truncated"]:
            lines.append("… (more matches; narrow the search)")
        return "\n".join(lines) or "No matches."

    @server.tool()
    async def build(run: bool = True) -> str:
        """Compiles the project in the browser (rustc, then wasm-bindgen) and, if `run`, restarts the app
        with the result. Returns rustc's errors and warnings as rendered by rustc."""
        return format_build(await page("build", timeout=1800, run=run))

    @server.tool()
    async def get_diagnostics() -> str:
        """The errors and warnings of the last build, without building again."""
        r = await page("get_diagnostics", timeout=30)
        if not r["lastBuild"]:
            return "No build yet."
        return format_build({**r["lastBuild"], "diagnostics": r["diagnostics"]})

    @server.tool()
    async def open_in_editor(path: str, line: int | None = None) -> str:
        """Shows a file (at a line) in the IDE's editor, for the user to look at."""
        r = await page("open_file", path=path, line=line, timeout=30)
        return f"Showing {r['open']}"

    @server.tool()
    async def app_screenshot() -> Image:
        """A PNG of the running app (the right pane)."""
        r = await page("screenshot", timeout=30)
        return Image(data=base64.b64decode(r["png"]), format="png")

    @server.tool()
    async def app_console(clear: bool = False, last: int = 200) -> str:
        """The app's console output (logs, warnings, panics) since it started; `clear` empties it."""
        r = await page("app_console", clear=clear, last=last, timeout=30)
        logs = "\n".join(f"[{e['level']}] {e['text']}" for e in r["logs"])
        return f"App: {r['state']}\n{logs or '(no output)'}"

    @server.tool()
    async def app_size() -> str:
        """The app pane's size in CSS pixels (the coordinates app_click and friends take) and the
        device pixel ratio (screenshots are that many times larger)."""
        return json.dumps(await page("app_size", timeout=30))

    @server.tool()
    async def app_click(x: float, y: float, button: int = 0, double: bool = False) -> str:
        """Clicks the app at (x, y) in CSS pixels from its top-left. button: 0 left, 1 middle, 2 right."""
        await page("app_click", x=x, y=y, button=button, double=double, timeout=30)
        return f"Clicked at ({x}, {y})"

    @server.tool()
    async def app_drag(x: float, y: float, to_x: float, to_y: float) -> str:
        """Drags with the left button from (x, y) to (to_x, to_y)."""
        await page("app_drag", x=x, y=y, toX=to_x, toY=to_y, timeout=30)
        return f"Dragged from ({x}, {y}) to ({to_x}, {to_y})"

    @server.tool()
    async def app_scroll(x: float, y: float, dy: float, dx: float = 0) -> str:
        """Scrolls the app at (x, y) by (dx, dy) pixels (positive dy scrolls down)."""
        await page("app_scroll", x=x, y=y, dy=dy, dx=dx, timeout=30)
        return f"Scrolled by ({dx}, {dy})"

    @server.tool()
    async def app_key(key: str, ctrl: bool = False, shift: bool = False, alt: bool = False, meta: bool = False) -> str:
        """Presses one key in the app: a character or a key name such as "Enter", "Backspace",
        "ArrowLeft", "Escape", "Tab", with modifiers."""
        await page("app_key", key=key, ctrl=ctrl, shift=shift, alt=alt, meta=meta, timeout=30)
        return f"Pressed {key}"

    @server.tool()
    async def app_type(text: str) -> str:
        """Types text into the app (into whatever has focus there); newlines press Enter."""
        await page("app_type", text=text, timeout=120)
        return f"Typed {len(text)} characters"

    @server.tool()
    async def restart_app() -> str:
        """Starts the last build of the app again, from scratch."""
        r = await page("restart_app", timeout=60)
        return f"App: {r['app']}"

    @server.tool()
    async def reset_project() -> str:
        """Discards every edit in the browser and goes back to the project as shipped."""
        r = await page("reset_project", timeout=60)
        return f"Reset {len(r['reset'])} file(s): {', '.join(r['reset']) or 'none were edited'}"

    @server.tool()
    async def sync_to_disk(directory: str) -> str:
        """Writes the browser's project files (text files, as edited) into a local directory, e.g. to
        review them with git or to keep them."""
        root = Path(directory).expanduser().resolve()
        files = [f["path"] for f in await page("list_files")]
        written = 0
        for rel in files:
            if Path(rel).suffix not in TEXT_SUFFIXES:
                continue  # pictures and other binary files stay as shipped
            r = await page("read_file", path=rel)
            target = root / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(r["text"])
            written += 1
        return f"Wrote {written} file(s) to {root}"

    @server.tool()
    async def load_from_disk(directory: str, only_changed: bool = True) -> str:
        """Copies a local project's text files (src/**, Cargo.toml) into the browser IDE, e.g. after
        editing them on disk. With `only_changed`, files equal to the browser's are skipped."""
        root = Path(directory).expanduser().resolve()
        candidates = [p for p in root.rglob("*") if p.is_file() and p.suffix in (".rs", ".toml", ".lock")]
        candidates = [p for p in candidates if "target" not in p.relative_to(root).parts]
        loaded = []
        for p in sorted(candidates):
            rel = str(p.relative_to(root))
            text = p.read_text()
            if only_changed:
                try:
                    if (await page("read_file", path=rel))["text"] == text:
                        continue
                except RuntimeError:
                    pass
            await page("write_file", path=rel, content=text)
            loaded.append(rel)
        return f"Loaded {len(loaded)} file(s): {', '.join(loaded) or 'nothing changed'}"

    return server


async def run_mcp(link: Link) -> None:
    await build_server(link).run_stdio_async()
