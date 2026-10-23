"""Uses the running app through the bridge: click, type, screenshot."""

import asyncio
import base64
import sys
from pathlib import Path

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

ROOT = Path(__file__).resolve().parents[2]
SHOTS = Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp")


async def main() -> None:
    params = StdioServerParameters(command=str(ROOT / ".venv/bin/python"), args=[str(ROOT / "bridge/ide_bridge.py")], cwd=str(ROOT))
    async with stdio_client(params) as (read, write), ClientSession(read, write) as s:
        await s.initialize()

        async def call(name, **args):
            r = await s.call_tool(name, args, read_timeout_seconds=600)
            out = "\n".join(c.text for c in r.content if c.type == "text")
            print(f"## {name} {args}: {'ERROR ' if r.is_error else ''}{out[:300]}")
            return r

        async def shot(name):
            r = await call("app_screenshot")
            img = next(c for c in r.content if c.type == "image")
            (SHOTS / name).write_bytes(base64.b64decode(img.data))

        await call("app_click", x=112, y=330)
        await asyncio.sleep(2.5)
        await shot("mcp-blank-page.png")
        await call("app_type", text="Written by Claude through the MCP bridge, in an app compiled in the browser.")
        await asyncio.sleep(1)
        await shot("mcp-typed.png")
        await call("app_console", last=20)


asyncio.run(main())
