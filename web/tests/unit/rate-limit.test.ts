/**
 * The token-bucket arithmetic the database limiter uses (src/server/services/rate-limits.ts;
 * tests/integration/rate-limits.test.ts runs it against Postgres).
 */
import { describe, expect, it } from "vitest";

import {
  refilledTokens,
  retryAfterSeconds,
  SYNC_RATE,
} from "~/server/app-api/rate-limit";

import { memoryRateLimiter } from "../support/memory-rate-limiter";

const at = (ms: number) => new Date(ms);

describe("sync rate", () => {
  it("is a burst of 12 syncs, refilled at one every 10 s (6 a minute) per user", () => {
    expect(SYNC_RATE).toEqual({ capacity: 12, perMs: 120_000 });
    expect(SYNC_RATE.perMs / SYNC_RATE.capacity).toBe(10_000);
  });

  it("fits one whole catch-up pass of the app (10 requests back to back)", async () => {
    // The Mac sends at most 10 batches in one pass, with no pause between them. A smaller burst
    // made the last ones of every catch-up pass a 429, and the app backed off for longer each time.
    const MAC_REQUESTS_PER_PASS = 10;
    const limiter = memoryRateLimiter({ ...SYNC_RATE, now: () => 0 });
    for (let i = 0; i < MAC_REQUESTS_PER_PASS; i++) {
      expect(await limiter.take("u1")).toEqual({ ok: true });
    }
  });
});

describe("refilledTokens", () => {
  it("refills continuously, never past capacity", () => {
    const bucket = { tokens: 0, updatedAt: at(0) };
    expect(refilledTokens(bucket, at(0), SYNC_RATE)).toBe(0);
    expect(refilledTokens(bucket, at(10_000), SYNC_RATE)).toBeCloseTo(1, 9);
    expect(refilledTokens(bucket, at(30_000), SYNC_RATE)).toBeCloseTo(3, 9);
    expect(refilledTokens(bucket, at(3_600_000), SYNC_RATE)).toBe(12);
  });

  it("never runs backwards when another instance's clock is behind", () => {
    const bucket = { tokens: 2.5, updatedAt: at(100_000) };
    expect(refilledTokens(bucket, at(90_000), SYNC_RATE)).toBe(2.5);
  });
});

describe("retryAfterSeconds", () => {
  it("is the time until a whole token is back, at least a second", () => {
    expect(retryAfterSeconds(0, SYNC_RATE)).toBe(10);
    expect(retryAfterSeconds(0.5, SYNC_RATE)).toBe(5);
    expect(retryAfterSeconds(0.99, SYNC_RATE)).toBe(1);
    expect(retryAfterSeconds(0.9999, SYNC_RATE)).toBe(1);
  });
});

describe("the test stand-in", () => {
  it("behaves like the database limiter: burst, then one per refill", async () => {
    let now = 1_000_000;
    const limiter = memoryRateLimiter({ ...SYNC_RATE, now: () => now });
    for (let i = 0; i < 12; i++)
      expect((await limiter.take("u1")).ok).toBe(true);
    expect(await limiter.take("u1")).toEqual({
      ok: false,
      retryAfterSeconds: 10,
    });
    expect((await limiter.take("u2")).ok).toBe(true);
    now += 10_000;
    expect((await limiter.take("u1")).ok).toBe(true);
    expect((await limiter.take("u1")).ok).toBe(false);
  });
});
