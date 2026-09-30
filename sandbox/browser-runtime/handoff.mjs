import { randomUUID } from "node:crypto";
import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import { dirname } from "node:path";

export class Handoff {
  constructor({ path, now = Date.now } = {}) {
    this.path = path;
    this.now = now;
    this.state = { phase: "idle", expiresAt: 0, handoffId: null };
  }

  async load() {
    if (!this.path) return;
    try {
      const state = JSON.parse(await readFile(this.path, "utf8"));
      if (!["idle", "requested", "active"].includes(state.phase) || !Number.isSafeInteger(state.expiresAt)) throw new Error("Invalid desktop handoff state");
      this.state = state;
    } catch (error) { if (error.code !== "ENOENT") throw error; }
  }

  active() { return this.state.phase === "active"; }
  due() { return this.state.phase !== "idle" && this.state.expiresAt <= this.now(); }

  publicState() {
    return { handoff: this.active(), handoffPhase: this.state.phase, handoffId: this.state.handoffId, expiresAt: this.state.expiresAt, helpRequested: this.state.phase === "requested", botId: this.state.botId || null, sessionId: this.state.sessionId || null, handoffProtocol: 2 };
  }

  async save(state) {
    if (this.path) {
      await mkdir(dirname(this.path), { recursive: true });
      const temporary = `${this.path}.${randomUUID()}.tmp`;
      await writeFile(temporary, JSON.stringify(state), { mode: 0o600 });
      await rename(temporary, this.path);
    }
    this.state = state;
    return { ok: true, ...this.publicState() };
  }

  async request(owner, input) {
    if (this.active()) throw new Error("The user currently controls the desktop");
    if (this.state.phase === "requested" && this.state.owner !== owner && !this.due()) throw new Error("Another agent has already requested human help");
    return this.save({ phase: "requested", owner, handoffId: null, expiresAt: this.now() + this.minutes(input.minutes) * 60_000, botId: input.botId || null, sessionId: input.sessionId || null });
  }

  async cancelRequest(owner) {
    if (this.active()) throw new Error("Only the authenticated user can return desktop control");
    if (this.state.phase === "requested" && this.state.owner !== owner) throw new Error("Another agent requested this help");
    return this.save({ phase: "idle", expiresAt: 0, handoffId: this.state.handoffId });
  }

  async start(input) {
    const id = input.handoffId;
    if (typeof id !== "string" || !/^[a-zA-Z0-9_-]{16,128}$/.test(id)) throw new Error("A unique handoffId is required; update the app before taking control");
    if (this.active()) {
      if (this.state.handoffId === id && !this.due()) return { ok: true, ...this.publicState() };
      throw new Error("Another viewer already controls the desktop");
    }
    if (id === this.state.handoffId) throw new Error("This handoff already ended; start a new takeover");
    return this.save({ phase: "active", handoffId: id, expiresAt: this.now() + this.minutes(input.minutes) * 60_000, botId: input.botId || this.state.botId || null, sessionId: input.sessionId || this.state.sessionId || null });
  }

  async stop(id) {
    if (!id || id !== this.state.handoffId) throw new Error("Stale handoff: this viewer does not own current control");
    return this.save({ phase: "idle", handoffId: id, expiresAt: 0 });
  }

  async expire() {
    if (!this.due()) return false;
    await this.save({ phase: "idle", expiresAt: 0, handoffId: this.state.handoffId });
    return true;
  }

  minutes(value = 10) {
    if (!Number.isInteger(value) || value < 1 || value > 30) throw new Error("minutes must be an integer from 1 to 30");
    return value;
  }
}
