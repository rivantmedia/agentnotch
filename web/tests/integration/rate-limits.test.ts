/**
 * The database rate limiter (src/server/services/rate-limits.ts) against Postgres: one bucket per
 * key shared by every server instance, exact under concurrency, refilled over time, and deleted
 * once it has been idle long enough to be full again.
 */
import { beforeEach, describe, expect, it } from "vitest";

import { handleSync, type AppApiDeps } from "~/server/app-api/handlers";
import {
  SYNC_IP_LIMITER,
  SYNC_IP_RATE,
  SYNC_LIMITER,
  SYNC_RATE,
} from "~/server/app-api/rate-limit";
import { clientIpKey } from "~/server/client-ip";
import {
  createDbRateLimiter,
  RATE_LIMIT_PRUNE_BATCH,
} from "~/server/services/rate-limits";

import { syncFixture } from "../support/fixtures";
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

  it("holds one IP address to 60 syncs a minute across people, keyed by a hash", async () => {
    const now = () => new Date("2026-09-25T12:00:00.000Z");
    // Wired as src/server/app-api/deps.ts wires them, with a fake sign-in and store.
    const deps: AppApiDeps = {
      resolveViewer: async (headers) => ({
        id: (headers.get("authorization") ?? "").replace("Bearer ", ""),
        email: "someone@example.com",
        name: null,
      }),
      applySync: async () => ({ sessions: 2, usage: 2 }),
      limiter: createDbRateLimiter(db, {
        name: SYNC_LIMITER,
        ...SYNC_RATE,
        now,
      }),
      ipLimiter: createDbRateLimiter(db, {
        name: SYNC_IP_LIMITER,
        ...SYNC_IP_RATE,
        now,
      }),
      clientIpKey: (headers) =>
        clientIpKey(headers, "a-test-pepper-0123456789"),
      siteUrl: "https://agentnotch.example.com",
      now,
    };
    const body = JSON.stringify(syncFixture());
    const sync = (user: string, ip: string) =>
      handleSync(
        new Request("https://agentnotch.example.com/api/app/v1/sync", {
          method: "POST",
          headers: {
            authorization: `Bearer ${user}`,
            "content-type": "application/json",
            "x-forwarded-for": ip,
          },
          body,
        }),
        deps,
      );

    for (let i = 0; i < 60; i++) {
      expect((await sync(`user-${i}`, "203.0.113.7")).status).toBe(200);
    }
    const limited = await sync("user-60", "203.0.113.7");
    expect(limited.status).toBe(429);
    expect(limited.headers.get("retry-after")).toBe("1");
    expect((await sync("user-61", "198.51.100.1")).status).toBe(200);

    // One bucket per address, under its keyed hash; the address itself is nowhere.
    const keys = (await db.rateLimit.findMany({ select: { key: true } })).map(
      (r) => r.key,
    );
    const ipBuckets = keys.filter((k) => k.startsWith(`${SYNC_IP_LIMITER}:`));
    expect(ipBuckets).toHaveLength(2);
    for (const key of ipBuckets) expect(key).toMatch(/^sync-ip:[0-9a-f]{64}$/);
    expect(keys.join(" ")).not.toMatch(/203\.0\.113\.7|198\.51\.100\.1/);
    // Each person's own bucket too (a refused sync still took one from it).
    expect(
      keys.filter((k) => k.startsWith(`${SYNC_LIMITER}:user-`)),
    ).toHaveLength(62);
  });
});

describe("idle buckets", () => {
  const T0 = new Date("2026-09-25T12:00:00.000Z");
  const at = (ms: number) => new Date(T0.getTime() + ms);
  // Sorted in JavaScript, never by the database's collation.
  const keys = async () =>
    (await db.rateLimit.findMany({ select: { key: true } }))
      .map((r) => r.key)
      .sort();

  it("are deleted once they are full again, and only those", async () => {
    let now = T0;
    const ips = createDbRateLimiter(db, {
      name: SYNC_IP_LIMITER,
      ...SYNC_IP_RATE,
      now: () => now,
    });
    const users = createDbRateLimiter(db, {
      name: SYNC_LIMITER,
      ...SYNC_RATE,
      now: () => now,
    });
    // Address a spends its whole minute; b and Ann's own bucket come later.
    for (let i = 0; i < SYNC_IP_RATE.capacity; i++) {
      expect((await ips.take("a")).ok).toBe(true);
    }
    expect((await ips.take("a")).ok).toBe(false);
    now = at(30_000);
    await ips.take("b");
    await users.take("ann");

    // A minute after a's last take it is full again, so it goes; b (idle 30 s) stays, as does
    // the other limiter's bucket, which the address limiter never touches.
    now = at(SYNC_IP_RATE.perMs);
    await ips.take("c");
    expect(await keys()).toEqual(["sync-ip:b", "sync-ip:c", "sync:ann"]);

    // Its next caller finds a full bucket, exactly as if it had stayed.
    for (let i = 0; i < SYNC_IP_RATE.capacity; i++) {
      expect((await ips.take("a")).ok).toBe(true);
    }
    expect((await ips.take("a")).ok).toBe(false);

    // Much later, a take by anyone clears every idle bucket of its own limiter.
    now = at(10 * 60_000);
    await ips.take("d");
    expect(await keys()).toEqual(["sync-ip:d", "sync:ann"]);
    await users.take("bob");
    expect(await keys()).toEqual(["sync-ip:d", "sync:bob"]);
  });

  it("are deleted a batch at a time, and never by a refused take", async () => {
    let now = T0;
    const limiter = createDbRateLimiter(db, {
      name: "test",
      capacity: 1,
      perMs: 60_000,
      now: () => now,
      pruneBatch: 2,
    });
    for (const key of ["a", "b", "c", "d", "e"]) await limiter.take(key);
    now = at(60_000);
    await limiter.take("f");
    expect(await keys()).toHaveLength(4); // six made, two deleted
    // A refused take writes nothing and deletes nothing.
    expect((await limiter.take("f")).ok).toBe(false);
    expect(await keys()).toHaveLength(4);
    await limiter.take("g");
    expect(await keys()).toHaveLength(3);
    await limiter.take("h");
    expect(await keys()).toEqual(["test:f", "test:g", "test:h"]);
    expect(RATE_LIMIT_PRUNE_BATCH).toBeGreaterThan(0);
  });
});
