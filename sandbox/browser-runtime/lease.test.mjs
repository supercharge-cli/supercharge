import assert from 'node:assert/strict';
import { test } from 'node:test';
import { BrowserLease } from './lease.mjs';

test('bots own browser sequences, not only individual requests', () => {
  const lease = new BrowserLease();
  assert.equal(lease.acquire('bot-a'), true);
  assert.equal(lease.acquire('bot-a'), false);
  assert.throws(() => lease.acquire('bot-b'), /busy/);
  assert.equal(lease.release('bot-a'), true);
  assert.equal(lease.acquire('bot-b'), true);
});

test('handoff blocks all bot input and resume resets stale references', () => {
  const lease = new BrowserLease();
  lease.acquire('bot-a');
  assert.throws(() => lease.acquire('bot-a', true), /user currently/);
  lease.reset();
  assert.equal(lease.acquire('bot-b'), true);
});

test('lease expiry recovers a crashed bot and fairly admits waiter', () => {
  let time = 1;
  const lease = new BrowserLease({now:() => time, leaseMs:10_000});
  lease.acquire('a');
  assert.throws(() => lease.acquire('b'), /busy/);
  time += 11_000;
  assert.throws(() => lease.acquire('c'), /busy/);
  assert.equal(lease.acquire('b'), true);
});
