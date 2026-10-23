"""Drives the bridge as Claude Code would: starts it over stdio and calls its tools."""

import asyncio
import base64
import sys
import time
from pathlib import Path

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

ROOT = Path(__file__).resolve().parents[2]
SHOTS = Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp")


def text(result) -> str:
    return "\n".join(c.text for c in result.content if getattr(c, "type", "") == "text")


async def main() -> None:
    params = StdioServerParameters(command=str(ROOT / ".venv/bin/python"), args=[str(ROOT / "bridge/ide_bridge.py")], cwd=str(ROOT))
    async with stdio_client(params) as (read, write), ClientSession(read, write) as s:
        await s.initialize()
        tools = await s.list_tools()
        print("tools:", ", ".join(t.name for t in tools.tools))

        async def call(name, **args):
            t0 = time.time()
            r = await s.call_tool(name, args, read_timeout_seconds=1800)
            print(f"\n## {name} {args if len(str(args)) < 120 else ''} ({time.time() - t0:.1f} s){' ERROR' if r.is_error else ''}")
            return r

        # Wait for the IDE's first build to finish and the app to run.
        for _ in range(120):
            st = text(await s.call_tool("ide_status", {}))
            if '"app": "running"' in st:
                break
            await asyncio.sleep(2)
        print(st)

        print(text(await call("read_file", path="src/start.rs", start_line=424, end_line=433)))
        print(text(await call("edit_file", path="src/start.rs", old_text='18..=22 => "Good evening.",', new_text='18..=22 => "Good evening, from rustc in the browser.",')))
        print(text(await call("build")))
        r = await call("app_screenshot")
        img = next(c for c in r.content if c.type == "image")
        (SHOTS / "mcp-after-edit.png").write_bytes(base64.b64decode(img.data))
        print("screenshot saved", len(img.data))
        print(text(await call("app_size")))

        # A deliberate error: diagnostics come back from the in-browser rustc.
        print(text(await call("edit_file", path="src/start.rs", old_text='"Good evening, from rustc in the browser."', new_text='"Good evening, from rustc in the browser.".len()')))
        print(text(await call("build"))[:3000])
        print(text(await call("edit_file", path="src/start.rs", old_text='"Good evening, from rustc in the browser.".len()', new_text='"Good evening, from rustc in the browser."')))
        print(text(await call("build")))
        print(text(await call("app_console", last=20)))


asyncio.run(main())
