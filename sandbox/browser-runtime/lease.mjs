export class BrowserLease {
  constructor({ now = Date.now, leaseMs = 60_000 } = {}) {
    this.now = now;
    this.leaseMs = leaseMs;
    this.owner = null;
    this.expiresAt = 0;
    this.generation = 0;
    this.waiting = new Map();
  }

  acquire(owner, handoff = false) {
    if (!owner || !/^[A-Za-z0-9_-]{1,128}$/.test(owner)) throw new Error("Missing browser owner");
    const now = this.now();
    if (handoff) throw new Error("The user currently controls the desktop; wait for return control");
    for (const [id, at] of this.waiting) if (now - at > 45_000) this.waiting.delete(id);
    this.expire();
    if (this.owner === owner) { this.expiresAt = now + this.leaseMs; return false; }
    if (!this.waiting.has(owner) && this.waiting.size >= 256) throw new Error("Desktop queue is full");
    this.waiting.set(owner, now);
    if (this.owner || this.waiting.keys().next().value !== owner) throw new Error("Desktop is busy with another bot. Retry observation when it becomes available");
    this.owner = owner;
    this.generation += 1;
    this.expiresAt = now + this.leaseMs;
    this.waiting.delete(owner);
    return true;
  }

  valid(owner, generation) {
    return this.owner === owner && this.generation === generation && this.expiresAt > this.now();
  }

  expire() {
    if (this.owner && this.expiresAt <= this.now()) { this.release(this.owner); return true; }
    return false;
  }

  release(owner) {
    this.waiting.delete(owner);
    if (this.owner !== owner) return false;
    this.owner = null;
    this.expiresAt = 0;
    this.generation += 1;
    return true;
  }

  reset() {
    this.owner = null;
    this.expiresAt = 0;
    this.generation += 1;
    this.waiting.clear();
  }
}
