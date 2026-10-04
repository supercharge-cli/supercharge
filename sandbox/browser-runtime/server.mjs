import { chromium } from "playwright";
import { createServer } from "node:http";
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import { BrowserLease } from "./lease.mjs";
import { Desktop } from "./desktop.mjs";
import { Handoff } from "./handoff.mjs";

const host = "127.0.0.1";
const port = Number.parseInt(process.env.SUPERAGENT_BROWSER_SIDECAR_PORT || "9224", 10);
const mediaDir = process.env.SUPERAGENT_MEDIA_ROOT || "/home/user/workspace/.superagent-media";
const maxBodyBytes = 128 * 1024;
const refs = new Map();
const lease = new BrowserLease();
const desktop = new Desktop();
const handoff = new Handoff({ path: process.env.SUPERAGENT_DESKTOP_HANDOFF_STATE || "/home/user/.supercharge/state/desktop-handoff.json" });
let serial = Promise.resolve();
let authority = Promise.resolve();
let currentAction;
let context;
let page;
let browser;
let shuttingDown = false;

function authorityAction(action) {
  const pending = authority.then(action, action);
  authority = pending.catch(() => {});
  return pending;
}

function json(res, status, body) {
  if (res.destroyed) return;
  res.writeHead(status, { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" });
  res.end(JSON.stringify(body));
}

async function body(req) {
  const chunks = [];
  let size = 0;
  for await (const chunk of req) {
    size += chunk.length;
    if (size > maxBodyBytes) throw new Error("request body is too large");
    chunks.push(chunk);
  }
  if (!chunks.length) return {};
  const value = JSON.parse(Buffer.concat(chunks).toString("utf8"));
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("request body must be an object");
  return value;
}

function requireString(value, label, max = 16_000) {
  if (typeof value !== "string" || !value.trim() || Buffer.byteLength(value) > max || value.includes("\0")) throw new Error(`${label} is missing, invalid, or too long`);
  return value;
}

function requireUrl(value) {
  const parsed = new URL(requireString(value, "url", 2048));
  if (!["http:", "https:"].includes(parsed.protocol)) throw new Error("only http and https URLs are supported");
  return parsed.toString();
}

function locatorFor(ref) {
  const entry = refs.get(requireString(ref, "ref", 64));
  if (!entry) throw new Error("Unknown or stale browser reference; request browser_snapshot again");
  return entry;
}

function invalidate() { refs.clear(); desktop.invalidate(); }

async function reconcilePage() {
  const pages = context.pages().filter((candidate) => !candidate.isClosed());
  const focused = [];
  for (const candidate of pages) {
    try { if (await candidate.evaluate(() => document.visibilityState === "visible")) focused.push(candidate); } catch { /* A tab can be navigating or closing. */ }
  }
  let next;
  if (focused.length === 1) next = focused[0];
  else if (focused.length > 1) {
    const active = (await desktop.windows()).find((window) => window.active);
    for (const candidate of focused) {
      const title = await candidate.title().catch(() => "");
      if (title && active?.title.includes(title)) { if (next) throw new Error("Multiple browser windows match; focus the intended window with desktop_action"); next = candidate; }
    }
    if (!next) throw new Error("Multiple browser windows are visible; focus the intended window with desktop_action");
  } else if (pages.length === 1) next = pages[0];
  else throw new Error("Cannot identify the current browser tab; inspect and focus it with desktop tools");
  if (page !== next) { page = next; refs.clear(); }
  page.setDefaultTimeout(5000);
  page.setDefaultNavigationTimeout(8000);
}

async function snapshot() {
  const generation = randomUUID().slice(0, 8);
  refs.clear();
  const nodes = await page.locator("a,button,input,textarea,select,[role],summary").evaluateAll((elements, generation) => elements.slice(0, 250).map((element, index) => {
    const text = (element.getAttribute("aria-label") || element.getAttribute("title") || element.innerText || element.value || "").replace(/\s+/g, " ").trim().slice(0, 240);
    const role = element.getAttribute("role") || element.tagName.toLowerCase();
    const id = `${generation}-e${index + 1}`;
    element.setAttribute("data-supercharge-ref", id);
    return { ref: id, role, name: text, disabled: "disabled" in element && Boolean(element.disabled) };
  }), generation);
  for (const node of nodes) refs.set(node.ref, page.locator(`[data-supercharge-ref="${node.ref}"]`));
  let aria;
  try { aria = await page.locator("body").ariaSnapshot({ timeout: 3000 }); }
  catch { aria = nodes.map((node) => `${node.ref} ${node.role}${node.name ? `: ${node.name}` : ""}`).join("\n"); }
  return { ok: true, url: page.url(), title: await page.title(), snapshot: aria.slice(0, 50_000), elements: nodes, ...handoff.publicState() };
}

async function screenshot(owner, passive) {
  const result = await desktop.observe(owner, lease.generation, { passive });
  if (!passive) {
    await mkdir(mediaDir, { recursive: true });
    result.path = join(mediaDir, `desktop-${result.frameId}.jpg`);
    await writeFile(result.path, Buffer.from(result.jpeg, "base64"), { mode: 0o600 });
  }
  return { ...result, url: page?.isClosed() ? "" : page?.url(), ...handoff.publicState() };
}

async function expire() {
  if (handoff.due()) {
    // Keep the handoff active until held input is released, so queued agents cannot slip in.
    await desktop.resetInput();
    await handoff.expire();
    lease.reset(); invalidate();
  }
  if (lease.expire()) {
    currentAction?.controller.abort();
    await desktop.releaseInput(); invalidate();
  }
}

async function control(path, input, owner, observer) {
  return authorityAction(async () => {
    await expire();
    if ((path === "/handoff/request" || path === "/handoff/start") && (input.botId || "main") !== (process.env.SUPERAGENT_DESKTOP_BOT_ID || "main")) throw new Error("Handoff bot does not match this desktop");
    if (path === "/handoff/request" || path === "/handoff/cancel") {
      if (observer) throw new Error("Human-help requests belong to the originating agent");
      if (lease.acquire(owner, handoff.active())) invalidate();
      return path.endsWith("request") ? handoff.request(owner, input) : handoff.cancelRequest(owner);
    }
    if (!observer) throw new Error("An authenticated daemon handoff is required");
    if (path === "/handoff/start") {
      if (currentAction) throw new Error("An agent input operation is still running; wait for it to finish");
      if (!handoff.active()) { await desktop.resetInput(); lease.reset(); invalidate(); }
      return handoff.start(input);
    }
    if (path === "/handoff/stop") {
      if (currentAction) throw new Error("Desktop input is still settling; retry returning control");
      if (input.handoffId !== handoff.state.handoffId) throw new Error("Stale handoff: this viewer does not own current control");
      await desktop.resetInput();
      const result = await handoff.stop(input.handoffId);
      lease.reset(); invalidate();
      return result;
    }
    if (path === "/lease/release") {
      const target = requireString(input.owner, "owner", 128);
      if (currentAction?.owner === target) currentAction.controller.abort();
      if (lease.release(target)) { await desktop.releaseInput(); invalidate(); }
      return { ok: true };
    }
    if (path === "/lease/reset") {
      if (handoff.active()) throw new Error("Cannot reset input while a user controls the desktop");
      currentAction?.controller.abort();
      await desktop.resetInput(); lease.reset(); invalidate();
      return { ok: true };
    }
    throw new Error("Unknown control endpoint");
  });
}

async function browserAction(path, input) {
  await reconcilePage();
  if (path === "/snapshot") return snapshot();
  desktop.invalidate();
  if (path === "/navigate") { refs.clear(); await page.goto(requireUrl(input.url), { waitUntil: "domcontentloaded" }); }
  else if (path === "/click") await locatorFor(input.ref).click();
  else if (path === "/fill") await locatorFor(input.ref).fill(requireString(input.text, "text"));
  else if (path === "/press") await (input.ref ? locatorFor(input.ref) : page.locator("body")).press(requireString(input.key, "key", 80));
  else if (path === "/control") {
    const action = requireString(input.action, "action", 32);
    if (action === "navigate") await page.goto(requireUrl(input.url), { waitUntil: "domcontentloaded" });
    else if (action === "click") await page.locator(requireString(input.selector, "selector", 512)).click();
    else if (action === "fill") await page.locator(requireString(input.selector, "selector", 512)).fill(requireString(input.text, "text"));
    else if (action === "type") await page.locator(requireString(input.selector, "selector", 512)).fill(requireString(input.text, "text"));
    else if (action === "press") await page.keyboard.press(requireString(input.key, "key", 80));
    else if (action === "select") await page.locator(requireString(input.selector, "selector", 512)).selectOption(requireString(input.value, "value"));
    else if (action === "scroll") {
      const pixels = Math.min(Math.max(Number(input.pixels) || 700, 1), 3000);
      const offsets = { up: [0, -pixels], down: [0, pixels], left: [-pixels, 0], right: [pixels, 0] };
      if (!offsets[input.direction]) throw new Error("Invalid scroll direction");
      await page.mouse.wheel(...offsets[input.direction]);
    } else if (action === "wait") await page.waitForTimeout(Math.min(Math.max(Number(input.milliseconds) || 500, 0), 5000));
    else if (action === "back") await page.goBack({ waitUntil: "domcontentloaded" });
    else if (action === "forward") await page.goForward({ waitUntil: "domcontentloaded" });
    else if (action === "reload") await page.reload({ waitUntil: "domcontentloaded" });
    else throw new Error("Unsupported browser action");
  } else throw new Error("Unknown browser endpoint");
  return { ok: true, url: page.url() };
}

await handoff.load();
await desktop.resetInput();
await desktop.image();
browser = await chromium.connectOverCDP(`http://127.0.0.1:${process.env.SUPERAGENT_BROWSER_CDP_PORT || "9222"}`);
context = browser.contexts()[0];
if (!context) throw new Error("The persistent browser context is unavailable");
page = context.pages()[0] || await context.newPage();
const server = createServer(async (req, res) => {
  try {
    const path = new URL(req.url || "/", `http://${host}:${port}`).pathname;
    const owner = typeof req.headers["x-vectra-browser-owner"] === "string" ? req.headers["x-vectra-browser-owner"] : "";
    const observer = req.headers["x-vectra-browser-observer"] === "1";
    if (req.method === "GET" && ["/health", "/status"].includes(path)) {
      await authorityAction(expire);
      return json(res, 200, { ok: true, browser: browser.isConnected(), desktop: true, desktopProtocol: 1, handoffProtocol: 2, display: process.env.DISPLAY || ":99", botId: process.env.SUPERAGENT_DESKTOP_BOT_ID || "main", url: page?.isClosed() ? "" : page?.url(), vncPort: Number(process.env.SUPERAGENT_BROWSER_NOVNC_PORT || 6080), owner: lease.owner, generation: lease.generation, ...handoff.publicState(), desktopBotId: process.env.SUPERAGENT_DESKTOP_BOT_ID || "main" });
    }
    const input = req.method === "POST" ? await body(req) : {};
    if (req.method === "POST" && (path.startsWith("/lease/") || path.startsWith("/handoff/"))) return json(res, 200, await control(path, input, owner, observer));
    if (req.method === "GET" && path === "/screenshot" && observer) return json(res, 200, await screenshot(null, true));
    const allowed = req.method === "GET" ? ["/screenshot", "/desktop/observe", "/snapshot"] : req.method === "POST" ? ["/desktop/action", "/navigate", "/click", "/fill", "/press", "/control"] : [];
    if (!allowed.includes(path)) return json(res, 404, { ok: false, error: "not found" });
    const acceptedGeneration = lease.generation;
    const queuedOwner = lease.owner;
    const action = async () => {
      if (res.destroyed || shuttingDown) return;
      const controller = new AbortController();
      let generation;
      const close = () => { if (!res.writableEnded) controller.abort(); };
      res.on("close", close);
      try {
        await authorityAction(async () => {
          await expire();
          if (observer) {
            if (path !== "/control" || !handoff.active() || input.handoffId !== handoff.state.handoffId || handoff.due()) throw new Error("Take control with a current handoff before sending input");
          } else {
            if (queuedOwner === owner && acceptedGeneration !== lease.generation) throw new Error("Input ownership changed while this request waited; observe again");
            if (lease.acquire(owner, handoff.active())) { await desktop.releaseInput(); invalidate(); }
          }
          generation = lease.generation;
          currentAction = { owner, controller };
        });
        const check = () => {
          if (observer ? (!handoff.active() || handoff.due() || input.handoffId !== handoff.state.handoffId) : !lease.valid(owner, generation) || handoff.active()) throw new Error("Desktop ownership changed; observe again");
        };
        controller.signal.throwIfAborted(); check();
        let result;
        if (["/screenshot", "/desktop/observe"].includes(path)) result = await screenshot(owner, false);
        else if (path === "/desktop/action") { refs.clear(); result = await desktop.act(input, owner, generation, controller.signal, check); }
        else result = await browserAction(path, input);
        controller.signal.throwIfAborted(); check();
        json(res, 200, result);
      } finally {
        if (controller.signal.aborted) { await desktop.releaseInput(); lease.release(owner); invalidate(); }
        if (currentAction?.controller === controller) currentAction = null;
        res.off("close", close);
      }
    };
    const pending = serial.then(action, action);
    serial = pending.catch(() => {});
    await pending;
  } catch (error) {
    if (!res.headersSent) json(res, 409, { ok: false, error: error instanceof Error ? error.message : String(error) });
  }
});
const watchdog = setInterval(() => { void authorityAction(expire).catch((error) => console.error("desktop watchdog:", error.message)); }, 200);
watchdog.unref();
server.listen(port, host, () => console.log(`supercharge desktop sidecar listening on ${host}:${port}`));

async function shutdown() {
  if (shuttingDown) return;
  shuttingDown = true;
  clearInterval(watchdog);
  server.close(); currentAction?.controller.abort();
  await desktop.resetInput().catch(() => {});
  desktop.capture.close();
  // Chromium belongs to start.sh, not to this connection. Keep tabs and unsaved work alive.
  process.exit(0);
}
process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);
