// Opens a stream the way a viewer does (headless Chromium, the stream's
// login) and screenshots it once the desktop is drawn. Used by try-stream.sh.
//
//   node apps/wadd/dev/stream-probe.mjs URL USER PASSWORD OUT.png [HOST]
//
// HOST, when given, is sent as the Host header (the proxy routes on it), so
// a local URL can stand in for https://ws-<guid>.wadspaces.com.
// playwright-core comes from .build/uitools (npm i playwright-core there);
// the browser is $CHROMIUM, else the newest Chromium in Playwright's cache.
import { existsSync, readdirSync } from "node:fs";
import { createRequire } from "node:module";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "../../..");
const require = createRequire(join(root, ".build/uitools/package.json"));
const { chromium } = require("playwright-core");

const [url, user, password, out, host] = process.argv.slice(2);
if (!out) {
  console.error("usage: stream-probe.mjs URL USER PASSWORD OUT.png [HOST]");
  process.exit(2);
}
const cache = join(homedir(), ".cache/ms-playwright");
const executablePath =
  process.env.CHROMIUM ||
  (existsSync(cache) ? readdirSync(cache) : [])
    .filter((d) => /^chromium-\d+$/.test(d))
    .sort((a, b) => Number(b.split("-")[1]) - Number(a.split("-")[1]))
    .map((d) => join(cache, d, "chrome-linux/chrome"))
    .find((p) => existsSync(p));
const browser = await chromium.launch(executablePath ? { executablePath } : {});
try {
  const context = await browser.newContext({
    viewport: { width: 1280, height: 800 },
    httpCredentials: { username: user, password },
    ...(host ? { extraHTTPHeaders: { Host: host } } : {}),
  });
  const page = await context.newPage();
  const sockets = [];
  page.on("websocket", (ws) => sockets.push(ws.url()));
  const res = await page.goto(url, { waitUntil: "load", timeout: 30000 });
  console.log(`page ${res.status()}`);
  // The stream draws on a canvas once frames arrive; give it time to.
  await page.waitForSelector("canvas", { timeout: 30000 });
  await page.waitForTimeout(8000);
  await page.screenshot({ path: out });
  console.log(`websockets ${sockets.length}: ${sockets.join(" ")}`);
  console.log(`screenshot ${out}`);
} finally {
  await browser.close();
}
