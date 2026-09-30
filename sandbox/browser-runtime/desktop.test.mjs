import assert from "node:assert/strict";
import { test } from "node:test";
import { Desktop, keyChord } from "./desktop.mjs";
import { Handoff } from "./handoff.mjs";
import { BrowserLease } from "./lease.mjs";

function fixture() {
  const calls = [];
  const run = async (file, args) => {
    calls.push([file, ...args]);
    if (args[0] === "getdisplaygeometry") return "2560 1600";
    if (args[0] === "getactivewindow") return "10";
    if (args[0] === "getmouselocation") return "X=10\nY=20\n";
    if (file === "wmctrl") return "0x0000000a  0  123  0  0  1280  800  terminal.Terminal host Terminal\n";
    return "";
  };
  const capture = { capture: async () => ({ displayWidth: 2560, displayHeight: 1600, imageWidth: 1280, imageHeight: 800, jpeg: "fixture" }) };
  const desktop = new Desktop({ run, capture });
  return { desktop, calls };
}

test("desktop coordinates use delivered image geometry and cannot be reused after actions", async () => {
  const { desktop, calls } = fixture();
  const frame = await desktop.observe("a", 1);
  await desktop.act({ frameId: frame.frameId, actions: [{ action: "click", x: 100, y: 200 }] }, "a", 1);
  assert.ok(calls.some((args) => args.join(" ") === "xdotool mousemove 200 400"));
  await assert.rejects(desktop.act({ frameId: frame.frameId, actions: [{ action: "move", x: 0, y: 0 }] }, "a", 1), /Stale/);
});

test("rejects foreign frames, stale owners, out of bounds and malformed batches before input", async () => {
  const { desktop, calls } = fixture();
  const frame = await desktop.observe("a", 1);
  await assert.rejects(desktop.act({ frameId: frame.frameId, actions: [{ action: "click", x: 0, y: 0 }] }, "b", 1), /Stale/);
  await assert.rejects(desktop.act({ frameId: frame.frameId, actions: [{ action: "click", x: 0, y: 0 }] }, "a", 2), /Stale/);
  await assert.rejects(desktop.act({ frameId: frame.frameId, actions: [{ action: "click", x: 1280, y: 0 }] }, "a", 1), /x must/);
  await assert.rejects(desktop.act({ frameId: frame.frameId, actions: [{ action: "click", x: 2, y: 2 }, { action: "key", key: "--file=/tmp/attack" }] }, "a", 1), /Unsupported key/);
  assert.equal(calls.filter((args) => ["click", "mousemove", "keydown"].includes(args[1])).length, 0);
});

test("a cancelled drag releases all tracked buttons", async () => {
  const { desktop, calls } = fixture();
  const frame = await desktop.observe("a", 1);
  const controller = new AbortController();
  setTimeout(() => controller.abort(), 30);
  await assert.rejects(desktop.act({ frameId: frame.frameId, actions: [{ action: "drag", x: 10, y: 10, toX: 100, toY: 100, durationMs: 100 }] }, "a", 1, controller.signal));
  assert.equal(desktop.buttons.size, 0);
  assert.ok(calls.some((args) => args.join(" ") === "xdotool mouseup 1"));
});

test("held keys can be released across observations under the same owner", async () => {
  const { desktop, calls } = fixture();
  const frame = await desktop.observe("a", 1);
  await desktop.act({ frameId: frame.frameId, actions: [{ action: "key_down", key: "Ctrl" }] }, "a", 1);
  assert.equal(desktop.keys.size, 1);
  await desktop.releaseInput();
  assert.equal(desktop.keys.size, 0);
  assert.ok(calls.some((args) => args.join(" ") === "xdotool keyup Control_L"));
});

test("installed application IDs are discoverable without executing desktop entry contents", async () => {
  const { desktop } = fixture();
  const apps = await desktop.applications();
  assert.ok(Array.isArray(apps));
  assert.ok(apps.every((app) => /^[a-zA-Z0-9][a-zA-Z0-9._+-]{0,127}\.desktop$/.test(app.id)));
  assert.ok(apps.length <= 150);
  assert.doesNotThrow(() => desktop.validateAction({ action: "launch", application: "discord.desktop" }, {}));
  assert.throws(() => desktop.validateAction({ action: "launch", application: "../../discord.desktop" }, {}));
});

test("only allow-listed applications and named chords are accepted", () => {
  assert.deepEqual(keyChord("Ctrl+Shift+L"), ["Control_L", "Shift_L", "l"]);
  assert.throws(() => keyChord("Ctrl+$(id)"));
  const { desktop } = fixture();
  assert.throws(() => desktop.validateAction({ action: "launch", application: "sh -c rm" }, {}));
  assert.throws(() => desktop.validateAction({ action: "open", target: "javascript:alert(1)" }, {}));
});

test("help requests are adoptable, but another viewer and stale stops are rejected", async () => {
  let time = 1000;
  const handoff = new Handoff({ now: () => time });
  await handoff.request("agent", { minutes: 1, botId: "main", sessionId: "dm-a" });
  assert.equal(handoff.publicState().handoff, false);
  assert.equal(handoff.publicState().helpRequested, true);
  const first = "first-viewer-123456";
  const second = "second-viewer-123456";
  await handoff.start({ handoffId: first, minutes: 1 });
  assert.equal(handoff.publicState().sessionId, "dm-a");
  await assert.rejects(handoff.start({ handoffId: second }), /Another viewer/);
  await assert.rejects(handoff.cancelRequest("agent"), /authenticated user/);
  await handoff.stop(first);
  await handoff.start({ handoffId: second });
  await assert.rejects(handoff.stop(first), /Stale/);
  time += 600_001;
  assert.equal(await handoff.expire(), true);
  assert.equal(handoff.publicState().handoff, false);
  await handoff.stop(second);
  await assert.rejects(handoff.start({ handoffId: second }), /already ended/);
});

test("lease generations revoke pending work even after the same owner reacquires", () => {
  let time = 0;
  const lease = new BrowserLease({ now: () => time, leaseMs: 10 });
  lease.acquire("a");
  const generation = lease.generation;
  time = 11;
  assert.equal(lease.expire(), true);
  lease.acquire("a");
  assert.equal(lease.valid("a", generation), false);
  assert.equal(lease.valid("a", lease.generation), true);
});
