import { execFile, spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { readFile, readdir, stat } from "node:fs/promises";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

const buttons = { left: 1, middle: 2, right: 3 };
const scrollButtons = { up: 4, down: 5, left: 6, right: 7 };
const keyAliases = { Ctrl: "Control_L", Control: "Control_L", Alt: "Alt_L", Shift: "Shift_L", Super: "Super_L", Meta: "Super_L", Enter: "Return", Backspace: "BackSpace", Esc: "Escape", Space: "space", PageUp: "Prior", PageDown: "Next" };
const applicationDirectories = ["/usr/share/applications", "/usr/local/share/applications", "/home/user/.local/share/applications"];
const excludedCategories = new Set(["Settings", "System"]);

function applicationId(value) {
  if (typeof value !== "string" || !/^[a-zA-Z0-9][a-zA-Z0-9._+-]{0,127}\.desktop$/.test(value)) throw new Error("Use an application ID returned by desktop_observe, such as discord.desktop");
  return value;
}

function desktopField(contents, key) {
  const section = contents.match(/(?:^|\n)\[Desktop Entry\]([^]*?)(?=\n\[|$)/)?.[1] || "";
  return section.match(new RegExp(`(?:^|\\n)${key}=([^\\r\\n]*)`))?.[1]?.trim() || "";
}

export function integer(value, label, min, max) {
  if (!Number.isSafeInteger(value) || value < min || value > max) throw new Error(`${label} must be an integer from ${min} to ${max}`);
  return value;
}

export function keyChord(value) {
  if (typeof value !== "string" || value.length > 100) throw new Error("Invalid key chord");
  const keys = value.split("+").map((key) => keyAliases[key] || (/^[A-Z]$/.test(key) ? key.toLowerCase() : key));
  if (!keys.length || keys.length > 5 || keys.some((key) => !/^(?:[a-zA-Z0-9]|F(?:[1-9]|1[0-2])|Control_[LR]|Alt_[LR]|Shift_[LR]|Super_[LR]|Return|Tab|Escape|BackSpace|Delete|Insert|Home|End|Left|Right|Up|Down|Prior|Next|space|minus|equal|plus|comma|period|slash|semicolon|apostrophe|bracketleft|bracketright|backslash|grave)$/.test(key))) {
    throw new Error("Unsupported key; use a named key or a chord such as Ctrl+L");
  }
  return keys;
}

function command(file, args, { signal, input, timeout = 5000 } = {}) {
  return new Promise((resolve, reject) => {
    const child = execFile(file, args, { env: { ...process.env, DISPLAY: process.env.DISPLAY || ":99" }, signal, timeout, maxBuffer: 1024 * 1024, encoding: "utf8" }, (error, stdout) => error ? reject(new Error(`${file} failed: ${error.message}`)) : resolve(stdout));
    child.stdin.on("error", () => {});
    child.stdin.end(input);
  });
}

export class DisplayCapture {
  constructor() {
    this.child = null;
    this.pending = new Map();
    this.next = 0;
  }

  start() {
    if (this.child) return;
    const child = spawn("python3", ["-u", fileURLToPath(new URL("./capture.py", import.meta.url))], { env: { ...process.env, DISPLAY: process.env.DISPLAY || ":99" }, stdio: ["pipe", "pipe", "inherit"] });
    this.child = child;
    const fail = (error) => {
      if (this.child !== child) return;
      this.child = null;
      for (const request of this.pending.values()) { clearTimeout(request.timer); request.reject(error); }
      this.pending.clear();
      child.kill();
    };
    child.on("error", fail);
    child.on("exit", () => fail(new Error("Desktop capture stopped")));
    child.stdin.on("error", fail);
    createInterface({ input: child.stdout }).on("line", (line) => {
      try {
        const result = JSON.parse(line);
        const request = this.pending.get(result.id);
        if (!request) return;
        this.pending.delete(result.id);
        clearTimeout(request.timer);
        if (result.error) request.reject(new Error(result.error)); else request.resolve(result);
      } catch { fail(new Error("Invalid desktop capture response")); }
    });
  }

  capture() {
    this.start();
    return new Promise((resolve, reject) => {
      const id = ++this.next;
      const timer = setTimeout(() => { this.close(); reject(new Error("Desktop capture timed out")); }, 5000);
      this.pending.set(id, { resolve, reject, timer });
      this.child.stdin.write(`${JSON.stringify({ id })}\n`);
    });
  }

  close() {
    const child = this.child;
    this.child = null;
    for (const request of this.pending.values()) { clearTimeout(request.timer); request.reject(new Error("Desktop capture closed")); }
    this.pending.clear();
    child?.kill();
  }
}

export class Desktop {
  constructor({ run = command, capture = new DisplayCapture(), now = Date.now } = {}) {
    this.run = run;
    this.capture = capture;
    this.now = now;
    this.keys = new Set();
    this.buttons = new Set();
    this.frames = new Map();
    this.generation = randomUUID();
    this.geometry = "";
    this.revision = 0;
    this.pendingCapture = null;
    this.cachedCapture = null;
  }

  invalidate() {
    this.revision += 1;
    this.frames.clear();
    this.cachedCapture = null;
  }

  async image() {
    if (this.cachedCapture && this.now() - this.cachedCapture.at < 100) return this.cachedCapture.image;
    if (!this.pendingCapture) {
      this.pendingCapture = this.capture.capture().then((image) => {
        const geometry = `${image.displayWidth}x${image.displayHeight}`;
        if (geometry !== this.geometry) { this.geometry = geometry; this.generation = randomUUID(); this.invalidate(); }
        this.cachedCapture = { at: this.now(), image };
        return image;
      }).finally(() => { this.pendingCapture = null; });
    }
    return this.pendingCapture;
  }

  async observe(owner, leaseGeneration, { passive = false } = {}) {
    const image = await this.image();
    const frameId = randomUUID();
    const metadata = { frameId, displayGeneration: this.generation, leaseGeneration, display: process.env.DISPLAY || ":99", displayWidth: image.displayWidth, displayHeight: image.displayHeight, imageWidth: image.imageWidth, imageHeight: image.imageHeight, scaleX: image.displayWidth / image.imageWidth, scaleY: image.displayHeight / image.imageHeight, originX: 0, originY: 0 };
    if (!passive) {
      for (const [id, frame] of this.frames) if (this.now() - frame.at > 45_000) this.frames.delete(id);
      while (this.frames.size >= 32) this.frames.delete(this.frames.keys().next().value);
      this.frames.set(frameId, { ...metadata, owner, at: this.now(), revision: this.revision });
    }
    return { ok: true, ...metadata, jpeg: image.jpeg, ...(passive ? {} : { windows: await this.windows(), pointer: await this.pointer(), applications: await this.applications() }) };
  }

  async windows(signal) {
    const [output, active] = await Promise.all([
      this.run("wmctrl", ["-lpGx"], { signal }),
      this.run("xdotool", ["getactivewindow"], { signal }).catch(() => "0"),
    ]);
    return output.trim().split("\n").filter(Boolean).slice(0, 100).map((line) => {
      const match = line.match(/^(0x[0-9a-f]+)\s+(-?\d+)\s+(\d+)\s+(-?\d+)\s+(-?\d+)\s+(\d+)\s+(\d+)\s+(\S+)\s+\S+\s*(.*)$/i);
      return match ? { id: match[1], desktop: Number(match[2]), pid: Number(match[3]), x: Number(match[4]), y: Number(match[5]), width: Number(match[6]), height: Number(match[7]), app: match[8], title: match[9].slice(0, 500), active: parseInt(match[1], 16) === Number(active.trim()) } : null;
    }).filter(Boolean);
  }

  async pointer(signal) {
    const output = await this.run("xdotool", ["getmouselocation", "--shell"], { signal });
    const fields = Object.fromEntries(output.trim().split("\n").map((line) => line.split("=")));
    return { x: Number(fields.X), y: Number(fields.Y) };
  }

  async applications() {
    const apps = new Map();
    for (const directory of applicationDirectories) {
      let entries;
      try { entries = await readdir(directory); } catch (error) { if (error.code === "ENOENT") continue; throw error; }
      for (const id of entries.sort().filter((name) => /^[a-zA-Z0-9][a-zA-Z0-9._+-]{0,127}\.desktop$/.test(name))) {
        if (apps.has(id)) continue;
        const path = join(directory, id);
        try {
          const details = await stat(path);
          if (!details.isFile() || details.size > 64 * 1024) continue;
          const contents = await readFile(path, "utf8");
          const name = desktopField(contents, "Name");
          const categories = desktopField(contents, "Categories").split(";");
          if (!name || desktopField(contents, "NoDisplay") === "true" || desktopField(contents, "Hidden") === "true" || categories.some((category) => excludedCategories.has(category))) continue;
          apps.set(id, { id, name: name.slice(0, 100) });
        } catch { /* An application can be removed while the directory is being read. */ }
      }
    }
    return [...apps.values()].slice(0, 150);
  }

  coordinates(frame, x, y) {
    integer(x, "x", 0, frame.imageWidth - 1);
    integer(y, "y", 0, frame.imageHeight - 1);
    return [String(Math.min(frame.displayWidth - 1, Math.floor(x * frame.scaleX))), String(Math.min(frame.displayHeight - 1, Math.floor(y * frame.scaleY)))];
  }

  async act(input, owner, leaseGeneration, signal, check = () => {}) {
    if (!Array.isArray(input.actions) || !input.actions.length || input.actions.length > 24) throw new Error("actions must contain 1-24 desktop actions");
    const frame = this.frames.get(input.frameId);
    if (!frame || frame.owner !== owner || frame.leaseGeneration !== leaseGeneration || frame.displayGeneration !== this.generation || frame.revision !== this.revision || this.now() - frame.at > 45_000) throw new Error("Stale desktop frame; observe the desktop again before acting");
    const geometry = (await this.run("xdotool", ["getdisplaygeometry"], { signal })).trim().split(/\s+/).map(Number);
    if (geometry[0] !== frame.displayWidth || geometry[1] !== frame.displayHeight) { this.invalidate(); throw new Error("Display size changed; observe the desktop again"); }
    // Validate the whole batch before injecting its first event.
    for (const action of input.actions) this.validateAction(action, frame);
    this.invalidate();
    try {
      for (const action of input.actions) {
        signal?.throwIfAborted(); check();
        await this.action(action, frame, signal, check);
        signal?.throwIfAborted(); check();
      }
    } catch (error) { await this.releaseInput(); throw error; }
    return { ok: true, performed: input.actions.length, message: "Observe the desktop again before the next action sequence." };
  }

  validateAction(action, frame) {
    if (!action || typeof action !== "object") throw new Error("Invalid desktop action");
    const button = action.button || "left";
    switch (action.action) {
      case "move": this.coordinates(frame, action.x, action.y); break;
      case "click":
        this.coordinates(frame, action.x, action.y);
        if (!buttons[button]) throw new Error("Invalid mouse button");
        integer(action.count ?? 1, "count", 1, 2); break;
      case "drag":
        this.coordinates(frame, action.x, action.y); this.coordinates(frame, action.toX, action.toY);
        integer(action.durationMs ?? 400, "durationMs", 100, 2000);
        if (!buttons[button]) throw new Error("Invalid mouse button"); break;
      case "button_down": case "button_up": if (!buttons[button]) throw new Error("Invalid mouse button"); break;
      case "scroll": if (!scrollButtons[action.direction]) throw new Error("Invalid scroll direction"); integer(action.amount ?? 3, "amount", 1, 20); break;
      case "key": case "key_down": case "key_up": keyChord(action.key); break;
      case "type":
        if (typeof action.text !== "string" || !action.text.length || Buffer.byteLength(action.text) > 16_000 || action.text.includes("\0")) throw new Error("text must contain 1-16000 UTF-8 bytes without NUL"); break;
      case "focus": if (typeof action.windowId !== "string" || !/^0x[0-9a-f]{1,16}$/i.test(action.windowId)) throw new Error("Invalid window ID"); break;
      case "launch": if (typeof action.application !== "string" || (!/^[a-zA-Z0-9][a-zA-Z0-9._+-]{0,127}\.desktop$/.test(action.application) && !["terminal", "files", "browser"].includes(action.application))) throw new Error("application must be terminal, files, browser, or an application ID from desktop_observe"); break;
      case "open": {
        if (typeof action.target !== "string" || action.target.length > 2048 || action.target.includes("\0")) throw new Error("Invalid open target");
        if (!action.target.startsWith("/home/user/") && !/^https?:\/\//.test(action.target)) throw new Error("Open an HTTP(S) URL or an absolute path under /home/user");
        if (action.target.startsWith("http")) { const url = new URL(action.target); if (url.username || url.password) throw new Error("URL credentials are not supported"); }
        break;
      }
      case "release": break;
      default: throw new Error("Unsupported desktop action");
    }
  }

  async action(action, frame, signal, check) {
    const run = (args) => { signal?.throwIfAborted(); check(); return this.run("xdotool", args, { signal }); };
    const button = buttons[action.button || "left"];
    switch (action.action) {
      case "move": return run(["mousemove", ...this.coordinates(frame, action.x, action.y)]);
      case "click":
        await run(["mousemove", ...this.coordinates(frame, action.x, action.y)]);
        this.buttons.add(button);
        await run(["click", "--repeat", String(action.count ?? 1), "--delay", "100", String(button)]);
        this.buttons.delete(button);
        return;
      case "drag": {
        const from = this.coordinates(frame, action.x, action.y).map(Number);
        const to = this.coordinates(frame, action.toX, action.toY).map(Number);
        await run(["mousemove", ...from.map(String)]);
        this.buttons.add(button);
        await run(["mousedown", String(button)]);
        for (let step = 1; step <= 12; step++) {
          await new Promise((resolve) => setTimeout(resolve, (action.durationMs ?? 400) / 12));
          await run(["mousemove", String(Math.round(from[0] + (to[0] - from[0]) * step / 12)), String(Math.round(from[1] + (to[1] - from[1]) * step / 12))]);
        }
        await run(["mouseup", String(button)]); this.buttons.delete(button); return;
      }
      case "button_down": this.buttons.add(button); return run(["mousedown", String(button)]);
      case "button_up": await run(["mouseup", String(button)]); this.buttons.delete(button); return;
      case "scroll": return run(["click", "--repeat", String(action.amount ?? 3), "--delay", "20", String(scrollButtons[action.direction])]);
      case "key": {
        const keys = keyChord(action.key);
        for (const key of keys) { this.keys.add(key); await run(["keydown", key]); }
        for (const key of keys.reverse()) { await run(["keyup", key]); this.keys.delete(key); }
        return;
      }
      case "key_down": for (const key of keyChord(action.key)) { this.keys.add(key); await run(["keydown", key]); } return;
      case "key_up": for (const key of keyChord(action.key).reverse()) { await run(["keyup", key]); this.keys.delete(key); } return;
      case "type": {
        // Clipboard insertion preserves Unicode and newlines without typing secrets into a process argument.
        await this.run("xclip", ["-selection", "clipboard", "-in"], { input: action.text, signal });
        await run(["key", "--clearmodifiers", "ctrl+v"]);
        await new Promise((resolve) => setTimeout(resolve, 150));
        await this.run("xclip", ["-selection", "clipboard", "-in"], { input: "", signal });
        return;
      }
      case "focus": {
        if (!(await this.windows(signal)).some((window) => parseInt(window.id, 16) === parseInt(action.windowId, 16))) throw new Error("Window no longer exists; observe the desktop again");
        return this.run("wmctrl", ["-ia", action.windowId], { signal });
      }
      case "launch": {
        const application = action.application;
        if (application.endsWith(".desktop")) {
          applicationId(application);
          if (!(await this.applications()).some((entry) => entry.id === application)) throw new Error("Application is not installed or visible. Observe the desktop for available applications.");
          return this.launch("gtk-launch", [application], signal);
        }
        const targets = { terminal: ["xfce4-terminal", ["--disable-server"]], files: ["thunar", ["/home/user/workspace"]], browser: ["xdg-open", ["about:blank"]] };
        const [file, args] = targets[application];
        return this.launch(file, args, signal);
      }
      case "open": return this.launch("xdg-open", [action.target], signal);
      case "release": return this.releaseInput();
    }
  }

  async launch(file, args, signal) {
    signal?.throwIfAborted();
    const child = spawn(file, args, { env: { ...process.env, DISPLAY: process.env.DISPLAY || ":99" }, detached: true, stdio: "ignore" });
    await new Promise((resolve, reject) => { child.once("spawn", resolve); child.once("error", reject); });
    child.unref();
  }

  async releaseInput() {
    const keys = [...this.keys]; const held = [...this.buttons];
    this.keys.clear(); this.buttons.clear();
    const operations = [
      ...keys.map((key) => this.run("xdotool", ["keyup", key])),
      ...held.map((button) => this.run("xdotool", ["mouseup", String(button)])),
    ];
    await Promise.allSettled(operations);
  }

  async resetInput() {
    await this.releaseInput();
    // A disconnected VNC client can leave inputs held even when this process did not press them.
    await this.run("python3", [fileURLToPath(new URL("./release-input.py", import.meta.url))]);
  }
}
