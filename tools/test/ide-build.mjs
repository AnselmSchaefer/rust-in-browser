// node ide-build.mjs [url] [shot.png]: opens the IDE headless, waits for the first build, prints Output
// and the result, and screenshots the page.
import { chromium } from "playwright-core";
const [url = "http://127.0.0.1:8765/", shot = "ide.png"] = process.argv.slice(2);
const browser = await chromium.launch({ executablePath: "/usr/bin/chromium", args: ["--enable-unsafe-swiftshader", "--js-flags=--max-old-space-size=8192"] });
const page = await browser.newPage({ viewport: { width: 1600, height: 950 } });
page.on("console", (m) => { if (m.type() === "error" || process.env.ALL) console.log(`[${m.type()}]`, m.text().slice(0, 500)); });
page.on("pageerror", (e) => console.log("[pageerror]", e.message));
const t0 = Date.now();
await page.goto(url);
await page.waitForFunction(() => window.ide?.state?.lastBuild, null, { timeout: 15 * 60 * 1000, polling: 1000 });
console.log(`first build done after ${((Date.now() - t0) / 1000).toFixed(1)} s`);
console.log(await page.evaluate(() => document.getElementById("output").innerText.slice(-6000)));
console.log(JSON.stringify(await page.evaluate(() => window.ide.state.lastBuild)));
await page.waitForTimeout(8000);
console.log("app:", await page.evaluate(() => window.ide.state.appState));
await page.screenshot({ path: shot });
await browser.close();
