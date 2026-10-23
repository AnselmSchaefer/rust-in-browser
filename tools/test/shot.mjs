// node shot.mjs URL OUT.png [waitMs]: opens URL in headless Chromium, prints console errors, saves a screenshot.
import { chromium } from "playwright-core";
const [url, out, wait = "8000"] = process.argv.slice(2);
const browser = await chromium.launch({ executablePath: "/usr/bin/chromium", args: ["--enable-unsafe-swiftshader", "--use-angle=swiftshader", "--enable-webgl"] });
const page = await browser.newPage({ viewport: { width: 1280, height: 860 } });
page.on("console", (m) => { if (["error", "warning"].includes(m.type()) || process.env.ALL) console.log(`[${m.type()}]`, m.text().slice(0, 400)); });
page.on("pageerror", (e) => console.log("[pageerror]", e.message));
await page.goto(url);
await page.waitForTimeout(Number(wait));
await page.screenshot({ path: out });
await browser.close();
