"""Bridge between Claude Code and the browser IDE.

One process does both jobs:

* It serves the IDE (``web/``) on http://127.0.0.1:8765 with the headers that let the page use
  ``SharedArrayBuffer`` (cross-origin isolation), which the in-browser toolchain needs. The IDE page
  connects back over a WebSocket (``/ws``).
* It is an MCP server on stdio: Claude Code starts it (see README) and calls its tools, which the
  bridge forwards to the IDE page and answers with the page's reply.

If another bridge already serves the port (say one started with ``--serve-only``), this one only does
MCP and forwards through that one's ``/rpc`` endpoint.

    python bridge/ide_bridge.py               # MCP on stdio + the IDE server (what Claude Code runs)
    python bridge/ide_bridge.py --serve-only  # just the IDE server
"""

from __future__ import annotations

import argparse
import asyncio
import itertools
import json
import logging
import mimetypes
import sys
from pathlib import Path

import aiohttp
from aiohttp import web

ROOT = Path(__file__).resolve().parent.parent
WEB = ROOT / "web"
HOST = "127.0.0.1"
PORT = 8765

log = logging.getLogger("ide-bridge")

mimetypes.add_type("application/wasm", ".wasm")
mimetypes.add_type("text/javascript", ".js")
mimetypes.add_type("text/javascript", ".mjs")


class PageLink:
    """The IDE page's WebSocket, and the calls waiting for its answers."""

    def __init__(self) -> None:
        self.ws: web.WebSocketResponse | None = None
        self.pending: dict[int, asyncio.Future] = {}
        self.ids = itertools.count(1)
        self.connected = asyncio.Event()

    async def call(self, method: str, params: dict | None = None, timeout: float = 600) -> object:
        if self.ws is None or self.ws.closed:
            try:
                await asyncio.wait_for(self.connected.wait(), 5)
            except TimeoutError:
                raise RuntimeError(
                    f"The IDE isn't open. Open http://{HOST}:{PORT}/ in Chrome (or another Chromium), "
                    "then try again."
                ) from None
        call_id = next(self.ids)
        future = asyncio.get_running_loop().create_future()
        self.pending[call_id] = future
        await self.ws.send_json({"id": call_id, "method": method, "params": params or {}})
        try:
            return await asyncio.wait_for(future, timeout)
        finally:
            self.pending.pop(call_id, None)

    def answer(self, msg: dict) -> None:
        future = self.pending.get(msg.get("id"))
        if future is None or future.done():
            return
        if "error" in msg:
            future.set_exception(RuntimeError(msg["error"]))
        else:
            future.set_result(msg.get("result"))


page = PageLink()


@web.middleware
async def isolation_headers(request: web.Request, handler):
    response = await handler(request)
    # SharedArrayBuffer (shared wasm memory, Atomics.wait) needs a cross-origin isolated page.
    response.headers["Cross-Origin-Opener-Policy"] = "same-origin"
    response.headers["Cross-Origin-Embedder-Policy"] = "require-corp"
    response.headers["Cross-Origin-Resource-Policy"] = "same-origin"
    return response


async def static(request: web.Request) -> web.StreamResponse:
    rel = request.match_info.get("path", "") or "index.html"
    path = (WEB / rel).resolve()
    if WEB not in path.parents and path != WEB:
        raise web.HTTPForbidden()
    if path.is_dir():
        path = path / "index.html"
    gz = path.with_name(path.name + ".gz")
    if not path.exists() and not gz.exists():
        raise web.HTTPNotFound()
    content_type = mimetypes.guess_type(path.name)[0] or "application/octet-stream"
    # The toolchain's big files are stored gzipped; the browser unpacks them as they arrive.
    if gz.exists() and "gzip" in request.headers.get("Accept-Encoding", ""):
        response = web.FileResponse(gz)
        response.headers["Content-Encoding"] = "gzip"
        response.headers["Content-Type"] = content_type
        size_file = path.with_name(path.name + ".size")
        if size_file.exists():
            response.headers["X-Decoded-Length"] = size_file.read_text().strip()
    else:
        response = web.FileResponse(path)
        response.headers["Content-Type"] = content_type
    # App files change while developing; the big toolchain files are revalidated by ETag.
    response.headers["Cache-Control"] = "no-cache"
    return response


async def websocket(request: web.Request) -> web.WebSocketResponse:
    ws = web.WebSocketResponse(max_msg_size=256 * 1024 * 1024, heartbeat=30)
    await ws.prepare(request)
    if page.ws is not None and not page.ws.closed:
        await page.ws.close(message=b"another IDE page connected")
    page.ws = ws
    page.connected.set()
    log.info("IDE page connected")
    try:
        async for msg in ws:
            if msg.type == aiohttp.WSMsgType.TEXT:
                page.answer(json.loads(msg.data))
    finally:
        if page.ws is ws:
            page.ws = None
            page.connected.clear()
            for future in page.pending.values():
                if not future.done():
                    future.set_exception(RuntimeError("the IDE page went away"))
        log.info("IDE page disconnected")
    return ws


async def rpc(request: web.Request) -> web.Response:
    """Lets a second bridge (another Claude Code session) reach the page through this one."""
    body = await request.json()
    try:
        result = await page.call(body["method"], body.get("params"), body.get("timeout", 600))
        return web.json_response({"result": result})
    except Exception as e:  # noqa: BLE001 - every failure goes back to the caller
        return web.json_response({"error": str(e)})


def make_app() -> web.Application:
    app = web.Application(middlewares=[isolation_headers], client_max_size=256 * 1024 * 1024)
    app.router.add_get("/ws", websocket)
    app.router.add_post("/rpc", rpc)
    app.router.add_get("/{path:.*}", static)
    return app


async def start_server() -> web.AppRunner | None:
    """Serves the IDE, or returns None if another bridge already does."""
    runner = web.AppRunner(make_app(), access_log=None)
    await runner.setup()
    try:
        await web.TCPSite(runner, HOST, PORT).start()
    except OSError:
        await runner.cleanup()
        return None
    log.info("IDE at http://%s:%s/", HOST, PORT)
    return runner


class Relay:
    """Calls the page through another bridge's /rpc."""

    async def call(self, method: str, params: dict | None = None, timeout: float = 600) -> object:
        async with aiohttp.ClientSession() as session:
            body = {"method": method, "params": params or {}, "timeout": timeout}
            async with session.post(f"http://{HOST}:{PORT}/rpc", json=body, timeout=timeout + 10) as r:
                reply = await r.json()
        if "error" in reply:
            raise RuntimeError(reply["error"])
        return reply["result"]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--serve-only", action="store_true", help="serve the IDE without MCP on stdio")
    args = parser.parse_args()
    # stdout is the MCP channel: logs go to stderr.
    logging.basicConfig(level=logging.INFO, stream=sys.stderr, format="%(name)s: %(message)s")

    if args.serve_only:

        async def serve() -> None:
            if await start_server() is None:
                sys.exit(f"port {PORT} is in use (is a bridge already running?)")
            await asyncio.Event().wait()

        asyncio.run(serve())
    else:
        from mcp_tools import run_mcp

        async def serve_and_mcp() -> None:
            runner = await start_server()
            link = page if runner is not None else Relay()
            if runner is None:
                log.info("another bridge serves port %s: relaying through it", PORT)
            await run_mcp(link)

        asyncio.run(serve_and_mcp())


if __name__ == "__main__":
    main()
