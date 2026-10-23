// node keep-open.mjs [minutes]: opens the IDE in headless Chromium and keeps it open (for MCP tests).
import { chromium } from "playwright-core";
const minutes = Number(process.argv[2] ?? 15);
const browser = await chromium.launch({ executablePath: "/usr/bin/chromium", args: ["--enable-unsafe-swiftshader"] });
const page = await browser.newPage({ viewport: { width: 1600, height: 950 } });
page.on("pageerror", (e) => console.log("[pageerror]", e.message));
page.on("console", (m) => { if (m.type() === "error") console.log("[error]", m.text().slice(0, 300)); });
await page.goto("http://127.0.0.1:8765/");
console.log("IDE open");
await page.waitForTimeout(minutes * 60000);
await browser.close();
