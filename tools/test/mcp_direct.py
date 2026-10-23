"""The bridge Claude Code starts serves the IDE itself: the open page reconnects to it."""

import asyncio
from pathlib import Path

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

ROOT = Path(__file__).resolve().parents[2]


async def main() -> None:
    params = StdioServerParameters(command=str(ROOT / ".venv/bin/python"), args=[str(ROOT / "bridge/ide_bridge.py")], cwd=str(ROOT))
    async with stdio_client(params) as (read, write), ClientSession(read, write) as s:
        await s.initialize()
        for name, args in [("ide_status", {}), ("search", {"pattern": "fn greeting"}), ("get_diagnostics", {})]:
            r = await s.call_tool(name, args, read_timeout_seconds=120)
            print(f"## {name}: {'ERROR ' if r.is_error else ''}" + "\n".join(c.text for c in r.content if c.type == "text")[:400])


asyncio.run(main())
