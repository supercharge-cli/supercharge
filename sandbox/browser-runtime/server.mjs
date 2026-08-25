import { chromium } from "playwright";
import { createServer } from "node:http";
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";

const host = "127.0.0.1";
const port = Number.parseInt(process.env.SUPERAGENT_BROWSER_SIDECAR_PORT || "9224", 10);
const profileDir = process.env.SUPERAGENT_BROWSER_PROFILE_DIR || "/home/user/.supercharge/browser-profile";
const mediaDir = process.env.SUPERAGENT_MEDIA_ROOT || "/home/user/workspace/.superagent-media";
const maxBodyBytes = 128 * 1024;
const maxTextChars = 16_000;
const refs = new Map();
let handoff = { active: false, expiresAt: 0 };
let context;
let page;

function now() {
  return Date.now();
}

function handoffActive() {
  if (handoff.active && handoff.expiresAt <= now()) handoff = { active: false, expiresAt: 0 };
  return handoff.active;
}

function json(res, status, body) {
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

function requireString(value, label, max = maxTextChars) {
  if (typeof value !== "string" || !value.trim() || value.length > max || value.includes("\0")) {
    throw new Error(`${label} is missing, invalid, or too long`);
  }
  return value.trim();
}

function requireUrl(value) {
  const raw = requireString(value, "url", 2_048);
  const parsed = new URL(raw);
  if (!["http:", "https:"].includes(parsed.protocol)) throw new Error("only http and https URLs are supported");
  return parsed.toString();
}

function locatorFor(ref) {
  const entry = refs.get(requireString(ref, "ref", 64));
  if (!entry) throw new Error("unknown browser reference; request browser_snapshot and use one of its refs");
  return entry;
}

async function snapshot() {
  refs.clear();
  const nodes = await page.locator("a,button,input,textarea,select,[role],summary").evaluateAll((elements) => elements.slice(0, 250).map((element, index) => {
    const text = (element.getAttribute("aria-label") || element.getAttribute("title") || element.innerText || element.value || "").replace(/\s+/g, " ").trim().slice(0, 240);
    const role = element.getAttribute("role") || element.tagName.toLowerCase();
    const id = `e${index + 1}`;
    element.setAttribute("data-supercharge-ref", id);
    return { ref: id, role, name: text, disabled: "disabled" in element && Boolean(element.disabled) };
  }));
  for (const node of nodes) refs.set(node.ref, page.locator(`[data-supercharge-ref="${node.ref}"]`));
  let aria = "";
  try {
    aria = await page.locator("body").ariaSnapshot({ timeout: 5_000 });
  } catch {
    aria = nodes.map((node) => `${node.ref} ${node.role}${node.name ? `: ${node.name}` : ""}`).join("\n");
  }
  return { ok: true, url: page.url(), title: await page.title(), snapshot: aria.slice(0, 50_000), elements: nodes, handoff: handoffActive() };
}

async function screenshot() {
  await mkdir(mediaDir, { recursive: true });
  const filename = `browser-${now()}.jpg`;
  const path = join(mediaDir, filename);
  const image = await page.screenshot({ type: "jpeg", quality: 72, path, fullPage: false });
  return { ok: true, path, jpeg: image.toString("base64"), url: page.url(), handoff: handoffActive() };
}

async function ensureBrowser() {
  await mkdir(profileDir, { recursive: true });
  await mkdir(mediaDir, { recursive: true });
  context = await chromium.launchPersistentContext(profileDir, {
    headless: false,
    viewport: { width: 1280, height: 800 },
    args: ["--no-first-run", "--no-default-browser-check", "--disable-dev-shm-usage"],
  });
  page = context.pages()[0] || await context.newPage();
  page.setDefaultTimeout(15_000);
}

async function route(req, res) {
  try {
    const url = new URL(req.url || "/", `http://${host}:${port}`);
    if (req.method === "GET" && url.pathname === "/health") return json(res, 200, { ok: true, browser: Boolean(page), handoff: handoffActive() });
    if (req.method === "GET" && url.pathname === "/status") return json(res, 200, { ok: true, url: page.url(), handoff: handoffActive(), vncPort: 6080 });
    if (req.method === "GET" && url.pathname === "/snapshot") return json(res, 200, await snapshot());
    if (req.method === "GET" && url.pathname === "/screenshot") return json(res, 200, await screenshot());
    if (req.method !== "POST") return json(res, 404, { error: "not found" });
    const input = await body(req);
    if (url.pathname === "/navigate") {
      const destination = requireUrl(input.url);
      await page.goto(destination, { waitUntil: "domcontentloaded" });
      return json(res, 200, { ok: true, url: page.url(), title: await page.title() });
    }
    if (url.pathname === "/click") {
      await locatorFor(input.ref).click();
      return json(res, 200, { ok: true, url: page.url() });
    }
    if (url.pathname === "/fill") {
      await locatorFor(input.ref).fill(requireString(input.text, "text"));
      return json(res, 200, { ok: true });
    }
    if (url.pathname === "/press") {
      const target = input.ref ? locatorFor(input.ref) : page.locator("body");
      await target.press(requireString(input.key, "key", 80));
      return json(res, 200, { ok: true });
    }
    if (url.pathname === "/handoff/start") {
      const minutes = Math.min(Math.max(Number(input.minutes) || 10, 1), 30);
      handoff = { active: true, expiresAt: now() + minutes * 60_000 };
      return json(res, 200, { ok: true, handoff: handoffActive(), expiresAt: handoff.expiresAt, vncPort: 6080 });
    }
    if (url.pathname === "/handoff/stop") {
      handoff = { active: false, expiresAt: 0 };
      return json(res, 200, { ok: true, handoff: false });
    }
    if (url.pathname === "/control") {
      const action = requireString(input.action, "action", 32);
      const selector = typeof input.selector === "string" ? requireString(input.selector, "selector", 512) : "";
      const text = typeof input.text === "string" ? requireString(input.text, "text") : "";
      if (action === "navigate") {
        await page.goto(requireUrl(input.url), { waitUntil: "domcontentloaded" });
      } else if (action === "click") {
        await page.locator(selector).click();
      } else if (action === "fill") {
        await page.locator(selector).fill(text);
      } else if (action === "type") {
        await page.locator(selector).pressSequentially(text);
      } else if (action === "press") {
        await page.keyboard.press(requireString(input.key, "key", 80));
      } else if (action === "select") {
        await page.locator(selector).selectOption(requireString(input.value, "value"));
      } else if (action === "scroll") {
        const direction = requireString(input.direction, "direction", 16).toLowerCase();
        const pixels = Math.min(Math.max(Number(input.pixels) || 700, 1), 3000);
        const offsets = { up: [0, -pixels], down: [0, pixels], left: [-pixels, 0], right: [pixels, 0] };
        if (!offsets[direction]) throw new Error("scroll direction must be up, down, left, or right");
        await page.mouse.wheel(...offsets[direction]);
      } else if (action === "wait") {
        await page.waitForTimeout(Math.min(Math.max(Number(input.milliseconds) || 500, 0), 30000));
      } else if (action === "back") {
        await page.goBack({ waitUntil: "domcontentloaded" });
      } else if (action === "forward") {
        await page.goForward({ waitUntil: "domcontentloaded" });
      } else if (action === "reload") {
        await page.reload({ waitUntil: "domcontentloaded" });
      } else {
        throw new Error("unsupported browser action");
      }
      return json(res, 200, { ok: true, action, url: page.url() });
    }
    return json(res, 404, { error: "not found" });
  } catch (error) {
    return json(res, 400, { ok: false, error: error instanceof Error ? error.message : String(error) });
  }
}

await ensureBrowser();
const server = createServer(route);
server.listen(port, host, () => console.log(`supercharge-browser sidecar listening on ${host}:${port}`));

async function shutdown() {
  server.close();
  await context?.close();
  process.exit(0);
}
process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);
