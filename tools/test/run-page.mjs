// node run-page.mjs URL [timeoutMs]: opens URL headless, waits for window.result, prints page text + result.
import { chromium } from "playwright-core";
const [url, timeout = "60000"] = process.argv.slice(2);
const browser = await chromium.launch({ executablePath: "/usr/bin/chromium" });
const page = await browser.newPage();
page.on("console", (m) => { if (m.type() === "error" || process.env.ALL) console.log(`[${m.type()}]`, m.text().slice(0, 600)); });
page.on("pageerror", (e) => console.log("[pageerror]", e.message));
await page.goto(url);
try {
  await page.waitForFunction(() => window.result !== undefined, null, { timeout: Number(timeout) });
} catch (e) { console.log("timeout waiting for window.result"); }
console.log(await page.evaluate(() => document.body.innerText));
await browser.close();
