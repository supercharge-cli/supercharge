import { chromium } from "playwright";
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { writeFile } from "node:fs/promises";

// The image installs browsers under /opt/ms-playwright and exports it for the
// template's own boot. Hosts that start this from their own environment lose
// the variable, so Playwright falls back to ~/.cache and the spawn fails.
const BAKED_BROWSERS = "/opt/ms-playwright";

function browserPath() {
  const env = process.env.PLAYWRIGHT_BROWSERS_PATH;
  const resolved = chromium.executablePath();
  if (existsSync(resolved)) return resolved;
  if (!env && existsSync(BAKED_BROWSERS)) {
    process.env.PLAYWRIGHT_BROWSERS_PATH = BAKED_BROWSERS;
    const baked = chromium.executablePath();
    if (existsSync(baked)) return baked;
  }
  return resolved;
}

const child = spawn(browserPath(), [
  `--user-data-dir=${process.env.SUPERAGENT_BROWSER_PROFILE_DIR || "/home/user/.supercharge/browser-profile"}`,
  "--remote-debugging-address=127.0.0.1", `--remote-debugging-port=${process.env.SUPERAGENT_BROWSER_CDP_PORT || "9222"}`,
  "--no-first-run", "--no-default-browser-check", "--disable-dev-shm-usage",
  "--start-maximized", "--restore-last-session",
  // E2B does not provide the namespace support needed by Chromium's nested sandbox.
  "--no-sandbox",
], { detached: true, stdio: "ignore", env: { ...process.env, DISPLAY: process.env.DISPLAY || ":99" } });
await new Promise((resolve, reject) => { child.once("spawn", resolve); child.once("error", reject); });
await writeFile(process.argv[2], `${child.pid}\n`, { mode: 0o600 });
child.unref();
