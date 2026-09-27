export class BrowserLease {
  constructor({ now = Date.now, leaseMs = 60_000 } = {}) {
    this.now = now;
    this.leaseMs = leaseMs;
    this.owner = null;
    this.expiresAt = 0;
    this.waiting = new Map();
  }

  acquire(owner, handoff = false) {
    if (!owner || !/^[A-Za-z0-9_-]{1,128}$/.test(owner)) throw new Error('Missing browser owner');
    const now = this.now();
    if (handoff) throw new Error('The user currently controls the browser; wait for return control');
    for (const [id, at] of this.waiting) if (now - at > 45_000) this.waiting.delete(id);
    if (this.owner && this.expiresAt <= now) this.owner = null;
    if (this.owner === owner) { this.expiresAt = now + this.leaseMs; return false; }
    if (!this.waiting.has(owner)) {
      if (this.waiting.size >= 256) throw new Error('Browser queue is full');
      this.waiting.set(owner, now);
    } else this.waiting.set(owner, now);
    if (this.owner || this.waiting.keys().next().value !== owner) {
      throw new Error('Browser is busy with another bot. Retry browser_snapshot when it becomes available');
    }
    this.owner = owner;
    this.expiresAt = now + this.leaseMs;
    this.waiting.delete(owner);
    return true;
  }

  release(owner) {
    this.waiting.delete(owner);
    if (this.owner === owner) { this.owner = null; this.expiresAt = 0; return true; }
    return false;
  }

  reset() {
    this.owner = null;
    this.expiresAt = 0;
    this.waiting.clear();
  }
}
