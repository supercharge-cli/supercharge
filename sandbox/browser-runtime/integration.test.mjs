import assert from "node:assert/strict";
import { chromium } from "playwright";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { execFileSync } from "node:child_process";

const base = "http://127.0.0.1:9224";
const owner = "integration-a";
async function call(path, data, { observer = false, as = owner, ok = true } = {}) {
  const response = await fetch(base + path, { method: data ? "POST" : "GET", headers: { "content-type": "application/json", ...(observer ? { "x-vectra-browser-observer": "1" } : { "x-vectra-browser-owner": as }) }, ...(data ? { body: JSON.stringify(data) } : {}) });
  const value = await response.json();
  if (ok) assert.equal(response.status, 200, `${path}: ${JSON.stringify(value)}`); else assert.notEqual(response.status, 200, path);
  return value;
}
const site = createServer((req, res) => {
  res.setHeader("Content-Type", "text/html; charset=utf-8");
  res.end('<title>Desktop integration</title><style>body{margin:0}textarea{position:absolute;left:100px;top:80px;width:400px;height:180px}#drag{position:absolute;left:650px;top:140px;width:160px;height:120px;background:teal;color:white}</style><textarea aria-label="Fixture"></textarea><div id="drag">Drag here</div><script>window.moves=[];document.addEventListener("pointermove",e=>{if(e.buttons)window.moves.push([e.clientX,e.clientY])});</script>');
});
await new Promise((resolve) => site.listen(18677, "127.0.0.1", resolve));
const browser = await chromium.connectOverCDP("http://127.0.0.1:9222");
try {
  await call("/lease/reset", {}, { observer: true });
  await call("/navigate", { url: "http://127.0.0.1:18677" });
  const page = browser.contexts()[0].pages().find((page) => page.url().includes("18677"));
  await page.bringToFront();
  const browserWindow = (await call("/desktop/observe")).windows.find((window) => window.title.includes("Desktop integration"));
  assert.ok(browserWindow);
  let frame = await call("/desktop/observe");
  await call("/desktop/action", { frameId: frame.frameId, actions: [{ action: "focus", windowId: browserWindow.id }, { action: "key", key: "Tab" }] });
  // Focus via DOM only sets up this local input fixture; the characters must come from real X11 input.
  await page.locator("textarea").focus();
  frame = await call("/desktop/observe");
  assert.equal(frame.displayWidth, 1280);
  assert.equal(frame.imageWidth, 1280);
  assert.ok(frame.jpeg.length > 1000);
  const text = "Desktop ✓ café 中文\nsecond line";
  await call("/desktop/action", { frameId: frame.frameId, actions: [{ action: "type", text }] });
  assert.equal(await page.locator("textarea").inputValue(), text);
  await call("/desktop/action", { frameId: frame.frameId, actions: [{ action: "move", x: 1, y: 1 }] }, { ok: false });

  frame = await call("/desktop/observe");
  await call("/desktop/action", { frameId: frame.frameId, actions: [{ action: "key", key: "Ctrl+A" }, { action: "key", key: "Backspace" }, { action: "type", text: "replaced" }] });
  assert.equal(await page.locator("textarea").inputValue(), "replaced");

  const geometry = await page.evaluate(() => ({ left: screenX, top: screenY + outerHeight - innerHeight }));
  frame = await call("/desktop/observe");
  await call("/desktop/action", { frameId: frame.frameId, actions: [{ action: "drag", x: geometry.left + 670, y: geometry.top + 160, toX: geometry.left + 750, toY: geometry.top + 210, durationMs: 200 }] });
  assert.ok(await page.evaluate(() => window.moves.length) > 0);

  frame = await call("/desktop/observe");
  await call("/desktop/action", { frameId: frame.frameId, actions: [{ action: "launch", application: "terminal" }, { action: "launch", application: "files" }] });
  await new Promise((resolve) => setTimeout(resolve, 700));
  frame = await call("/desktop/observe");
  assert.ok(frame.windows.some((window) => window.app.toLowerCase().includes("terminal")));
  assert.ok(frame.windows.some((window) => window.app.toLowerCase().includes("thunar")));
  await call("/desktop/observe", undefined, { as: "integration-b", ok: false });

  const help = await call("/handoff/request", { minutes: 1, botId: "main", sessionId: "fixture-dm" });
  assert.equal(help.handoff, false);
  assert.equal(help.helpRequested, true);
  const handoffId = "integration-viewer-123456";
  await call("/handoff/start", { handoffId, minutes: 1 }, { observer: true });
  await call("/desktop/observe", undefined, { ok: false });
  await call("/snapshot", undefined, { ok: false });
  await call("/screenshot", undefined, { observer: true });
  await call("/handoff/cancel", {}, { ok: false });
  await call("/handoff/stop", { handoffId: "wrong-viewer-123456" }, { observer: true, ok: false });
  await call("/handoff/stop", { handoffId }, { observer: true });
  await call("/desktop/observe", undefined, { as: "integration-b" });

  await call("/lease/release", { owner: "integration-b" }, { observer: true });
  const pid = await readFile("/tmp/supercharge-browser/chromium.pid", "utf8");
  execFileSync("bash", ["/opt/vectra/browser-runtime/start.sh"]);
  assert.equal(await readFile("/tmp/supercharge-browser/chromium.pid", "utf8"), pid);
  assert.ok(browser.contexts()[0].pages().some((page) => page.url().includes("18677")));
  for (const bot of ["research", "design"]) {
    execFileSync("bash", ["/opt/vectra/browser-runtime/start.sh"], { env: { ...process.env, SUPERAGENT_DESKTOP_BOT_ID: bot } });
  }
  const slots = await Promise.all(["research", "design"].map(async (bot) => Number((await readFile(`/tmp/supercharge-browser/bots/${bot}/slot`, "utf8")).trim())));
  assert.notEqual(slots[0], slots[1]);
  for (const [index, bot] of ["research", "design"].entries()) {
    const status = await (await fetch(`http://127.0.0.1:${13000 + slots[index]}/status`)).json();
    assert.equal(status.desktopBotId, bot);
    assert.equal(status.display, `:${99 + slots[index]}`);
    assert.equal(status.vncPort, 11000 + slots[index]);
    const observation = await (await fetch(`http://127.0.0.1:${13000 + slots[index]}/desktop/observe`, { headers: { "x-vectra-browser-owner": `integration-${bot}` } })).json();
    assert.equal(observation.display, status.display);
    assert.ok(observation.jpeg.length > 1000);
    const isolated = await chromium.connectOverCDP(`http://127.0.0.1:${12000 + slots[index]}`);
    assert.notEqual(isolated.contexts()[0].pages()[0]?.url(), "http://127.0.0.1:18677/");
    await isolated.close();
  }
  const botPid = await readFile("/tmp/supercharge-browser/bots/research/chromium.pid", "utf8");
  execFileSync("bash", ["/opt/vectra/browser-runtime/start.sh"], { env: { ...process.env, SUPERAGENT_DESKTOP_BOT_ID: "research" } });
  assert.equal(await readFile("/tmp/supercharge-browser/bots/research/chromium.pid", "utf8"), botPid);
  assert.equal((await call("/status")).desktopBotId, "main");
  console.log("PASS: desktop input, handoff, restart and two isolated bot displays/CDP/sidecars");
} finally {
  await call("/lease/reset", {}, { observer: true }).catch(() => {});
  site.close();
  // Disconnect this test client without terminating the persistent browser.
  process.exitCode = 0;
}
process.exit(0);
