/**
 * The database rate limiter (src/server/services/rate-limits.ts) against Postgres: one bucket per
 * key shared by every server instance, exact under concurrency, and refilled over time.
 */
import { beforeEach, describe, expect, it } from "vitest";

import { SYNC_RATE } from "~/server/app-api/rate-limit";
import { createDbRateLimiter } from "~/server/services/rate-limits";

import { db, resetDb } from "./db";

beforeEach(resetDb);

describe("createDbRateLimiter", () => {
  it("allows a burst of 12, then one every 10 s, per user", async () => {
    let now = new Date("2026-09-25T12:00:00.000Z");
    const limiter = createDbRateLimiter(db, {
      name: "sync",
      ...SYNC_RATE,
      now: () => now,
    });
    for (let i = 0; i < 12; i++)
      expect(await limiter.take("ann")).toEqual({ ok: true });
    expect(await limiter.take("ann")).toEqual({
      ok: false,
      retryAfterSeconds: 10,
    });
    // Another user has their own bucket.
    expect(await limiter.take("bob")).toEqual({ ok: true });

    now = new Date(now.getTime() + 5_000);
    expect(await limiter.take("ann")).toEqual({
      ok: false,
      retryAfterSeconds: 5,
    });
    now = new Date(now.getTime() + 5_000);
    expect(await limiter.take("ann")).toEqual({ ok: true });
    expect((await limiter.take("ann")).ok).toBe(false);

    // Long idle: full again, never fuller.
    now = new Date(now.getTime() + 3_600_000);
    for (let i = 0; i < 12; i++)
      expect((await limiter.take("ann")).ok).toBe(true);
    expect((await limiter.take("ann")).ok).toBe(false);
  });

  it("is shared: two server instances draw from one bucket", async () => {
    const now = () => new Date("2026-09-25T12:00:00.000Z");
    const a = createDbRateLimiter(db, { name: "sync", ...SYNC_RATE, now });
    const b = createDbRateLimiter(db, { name: "sync", ...SYNC_RATE, now });
    for (let i = 0; i < 6; i++) {
      expect((await a.take("ann")).ok).toBe(true);
      expect((await b.take("ann")).ok).toBe(true);
    }
    expect((await a.take("ann")).ok).toBe(false);
    expect((await b.take("ann")).ok).toBe(false);
  });

  it("gives out exactly the burst under concurrent calls", async () => {
    const limiter = createDbRateLimiter(db, {
      name: "sync",
      ...SYNC_RATE,
      now: () => new Date("2026-09-25T12:00:00.000Z"),
    });
    const results = await Promise.all(
      Array.from({ length: 30 }, () => limiter.take("ann")),
    );
    expect(results.filter((r) => r.ok)).toHaveLength(12);
  });

  it("keeps limiters with different names apart", async () => {
    const now = () => new Date("2026-09-25T12:00:00.000Z");
    const sync = createDbRateLimiter(db, {
      name: "sync",
      capacity: 1,
      perMs: 60_000,
      now,
    });
    const other = createDbRateLimiter(db, {
      name: "other",
      capacity: 1,
      perMs: 60_000,
      now,
    });
    expect((await sync.take("ann")).ok).toBe(true);
    expect((await sync.take("ann")).ok).toBe(false);
    expect((await other.take("ann")).ok).toBe(true);
  });
});
