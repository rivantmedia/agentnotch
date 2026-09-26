/**
 * Removing your own data from /settings (src/server/services/my-data.ts), on Postgres: exactly
 * the viewer's rows go (and account keys nobody refers to any more), nobody else's row changes,
 * the share-code guess limit survives, each runs in one transaction that waits for a sync of
 * theirs, and the tRPC mutations need the typed phrase.
 */
import { TRPCError } from "@trpc/server";
import { beforeEach, describe, expect, it } from "vitest";

import {
  DELETE_DATA_CONFIRMATION,
  REMOVE_SUMMARIES_CONFIRMATION,
} from "~/lib/data-controls";
import { createCaller } from "~/server/api/root";
import { type TRPCContext } from "~/server/api/trpc";
import {
  SYNC_IP_LIMITER,
  SYNC_IP_RATE,
  SYNC_LIMITER,
  SYNC_RATE,
} from "~/server/app-api/rate-limit";
import { ipKey } from "~/server/client-ip";
import { loadAccessScope } from "~/server/services/access";
import { listAccounts } from "~/server/services/accounts";
import { deleteSyncedData, removeSummaries } from "~/server/services/my-data";
import {
  createPoolCode,
  JOIN_FAILURE_LIMIT,
  joinPool,
} from "~/server/services/pools";
import { createDbRateLimiter } from "~/server/services/rate-limits";
import { applySync, lockUserData } from "~/server/services/sync";

import { createUser, db, resetDb } from "./db";
import { AFTER_FIXTURE, K1, K2, request, requestFor } from "./seed";

const UNKNOWN_CODE = "ZZZZ-ZZZZ-ZZZZ";
/** A third account, which only Ann ever syncs. */
const K3 = "3".repeat(64);

/** Ann's report of K3: the fixture's first account, session and reading, moved to K3. */
function annsOwnAccount() {
  return request((r) => {
    r.accounts = [{ ...r.accounts[0]!, key: K3, label: "Side project" }];
    r.sessions = [
      {
        ...r.sessions[0]!,
        accountKey: K3,
        sessionId: "a3a3a3a3-0000-4000-8000-000000000003",
      },
    ];
    r.usage = [{ ...r.usage[0]!, accountKey: K3 }];
  });
}

async function denied(promise: Promise<unknown>): Promise<string> {
  try {
    await promise;
  } catch (error) {
    if (error instanceof Error && "code" in error) return String(error.code);
    throw error;
  }
  throw new Error("expected a refusal");
}

/** Every row of every table, in a stable order. */
async function everything() {
  const [
    users,
    accounts,
    userAccounts,
    devices,
    projects,
    sessions,
    readings,
    windows,
    pools,
    members,
    failures,
    ipFailures,
    rateLimits,
  ] = await Promise.all([
    db.user.findMany({ orderBy: { id: "asc" } }),
    db.claudeAccount.findMany({ orderBy: { key: "asc" } }),
    db.userAccount.findMany({
      orderBy: [{ userId: "asc" }, { accountKey: "asc" }],
    }),
    db.device.findMany({ orderBy: [{ userId: "asc" }, { id: "asc" }] }),
    db.project.findMany({ orderBy: { id: "asc" } }),
    db.session.findMany({ orderBy: { id: "asc" } }),
    db.usageReading.findMany({ orderBy: { id: "asc" } }),
    db.usageWindow.findMany({
      orderBy: [{ userId: "asc" }, { accountKey: "asc" }, { windowId: "asc" }],
    }),
    db.pool.findMany({ orderBy: { id: "asc" } }),
    db.poolMember.findMany({ orderBy: [{ poolId: "asc" }, { userId: "asc" }] }),
    db.poolJoinFailure.findMany({ orderBy: { id: "asc" } }),
    db.poolJoinIpFailure.findMany({ orderBy: { id: "asc" } }),
    db.rateLimit.findMany({ orderBy: { key: "asc" } }),
  ]);
  return {
    users,
    accounts,
    userAccounts,
    devices,
    projects,
    sessions,
    readings,
    windows,
    pools,
    members,
    failures,
    ipFailures,
    rateLimits,
  };
}
type Everything = Awaited<ReturnType<typeof everything>>;

/**
 * What isn't `userId`'s: rows owned by someone else, and memberships of pools someone else made
 * (the viewer's own pools go with everyone's memberships of them).
 */
function othersRows(all: Everything, userId: string): Everything {
  const theirPools = new Set(
    all.pools.filter((p) => p.createdById === userId).map((p) => p.id),
  );
  const notTheirs = <T extends { userId: string }>(rows: T[]) =>
    rows.filter((r) => r.userId !== userId);
  return {
    ...all,
    users: all.users,
    userAccounts: notTheirs(all.userAccounts),
    devices: notTheirs(all.devices),
    projects: notTheirs(all.projects),
    sessions: notTheirs(all.sessions),
    readings: notTheirs(all.readings),
    windows: notTheirs(all.windows),
    pools: all.pools.filter((p) => !theirPools.has(p.id)),
    members: notTheirs(all.members).filter((m) => !theirPools.has(m.poolId)),
    rateLimits: all.rateLimits.filter((r) => r.key !== `sync:${userId}`),
  };
}

function caller(viewer: { id: string } | null) {
  const ctx: TRPCContext = {
    db,
    viewer: viewer
      ? {
          id: viewer.id,
          email: `${viewer.id}@example.com`,
          name: null,
          via: "cookie",
        }
      : null,
    headers: new Headers(),
  };
  return createCaller(ctx);
}

async function trpcCode(promise: Promise<unknown>): Promise<string> {
  try {
    await promise;
  } catch (error) {
    if (error instanceof TRPCError) return error.code;
    throw error;
  }
  throw new Error("expected a TRPCError");
}

beforeEach(async () => {
  await resetDb();
  await createUser("ann", "Ann");
  await createUser("bob", "Bob");
  await createUser("cat", "Cat");

  // Ann syncs both fixture accounts (S1 has a summary); Bob uses K1, Cat K2.
  await applySync(db, "ann", request(), AFTER_FIXTURE);
  await applySync(
    db,
    "bob",
    requestFor({
      deviceId: "11111111-2222-4333-8444-555555555555",
      accountKey: K1,
      sessionId: "b0b0b0b0-0000-4000-8000-000000000001",
      projectKey: "b".repeat(64),
      projectName: "bobs-app",
    }),
    AFTER_FIXTURE,
  );
  await db.session.updateMany({
    where: { userId: "bob" },
    data: {
      summaryText: "Bob's summary.",
      summaryModel: "m",
      summaryAt: new Date("2026-09-25T10:00:00Z"),
    },
  });
  await applySync(
    db,
    "cat",
    requestFor({
      deviceId: "33333333-2222-4333-8444-555555555555",
      accountKey: K2,
      sessionId: "cacacaca-0000-4000-8000-000000000001",
      projectKey: "c".repeat(64),
      projectName: "cats-api",
    }),
    AFTER_FIXTURE,
  );

  // Pools every way round: Ann's on K1 (Bob joined), Bob's on K1 (Ann joined), Cat's on K2
  // (Ann joined).
  await joinPool(db, "bob", (await createPoolCode(db, "ann", K1)).code);
  await joinPool(db, "ann", (await createPoolCode(db, "bob", K1)).code);
  await joinPool(db, "ann", (await createPoolCode(db, "cat", K2)).code);

  // Rate limits and failed codes, per person and per address.
  const now = () => new Date("2026-09-25T12:00:00Z");
  const sync = createDbRateLimiter(db, {
    name: SYNC_LIMITER,
    ...SYNC_RATE,
    now,
  });
  const syncIp = createDbRateLimiter(db, {
    name: SYNC_IP_LIMITER,
    ...SYNC_IP_RATE,
    now,
  });
  await sync.take("ann");
  await sync.take("bob");
  await syncIp.take(ipKey("203.0.113.7", undefined));
  const office = ipKey("203.0.113.7", undefined);
  for (const userId of ["ann", "ann", "bob"]) {
    await joinPool(db, userId, UNKNOWN_CODE, new Date(), office).catch(
      () => undefined,
    );
  }
});

describe("remove my summaries", () => {
  it("clears the summary of the viewer's sessions only", async () => {
    const before = await everything();
    expect(await removeSummaries(db, "ann")).toEqual({ sessions: 1 });
    const after = await everything();

    // Ann's sessions lost their summaries and nothing else.
    const strip = (rows: Everything["sessions"]) =>
      rows.map((s) => ({
        ...s,
        summaryText: null,
        summaryModel: null,
        summaryAt: null,
        updatedAt: null,
      }));
    const annsAfter = after.sessions.filter((s) => s.userId === "ann");
    expect(annsAfter).toHaveLength(2);
    for (const s of annsAfter) {
      expect([s.summaryText, s.summaryModel, s.summaryAt]).toEqual([
        null,
        null,
        null,
      ]);
    }
    expect(strip(annsAfter)).toEqual(
      strip(before.sessions.filter((s) => s.userId === "ann")),
    );
    // Every other row, in every table, is as it was.
    expect({
      ...after,
      sessions: after.sessions.filter((s) => s.userId !== "ann"),
    }).toEqual({
      ...before,
      sessions: before.sessions.filter((s) => s.userId !== "ann"),
    });
    expect(after.sessions.find((s) => s.userId === "bob")!.summaryText).toBe(
      "Bob's summary.",
    );

    // Again: nothing left to clear.
    expect(await removeSummaries(db, "ann")).toEqual({ sessions: 0 });
  });
});

describe("delete all my synced data", () => {
  it("deletes every row of the viewer's, and touches nobody else's", async () => {
    const before = await everything();
    const annsPools = before.pools.filter((p) => p.createdById === "ann");
    expect(annsPools).toHaveLength(1);

    expect(await deleteSyncedData(db, "ann")).toEqual({
      sessions: 2,
      projects: 2,
      usageReadings: 4,
      usageWindows: 4,
      accounts: 2,
      accountKeys: 0, // Bob still uses K1, and Cat K2
      devices: 1,
      poolsDeleted: 1,
      poolsLeft: 2, // Bob's and Cat's
    });
    const after = await everything();

    // Nothing of Ann's is left but her sign-in.
    expect(after.users.map((u) => u.id)).toEqual(["ann", "bob", "cat"]);
    for (const table of [
      "userAccounts",
      "devices",
      "projects",
      "sessions",
      "readings",
      "windows",
      "members",
    ] as const) {
      expect(
        (after[table] as Array<{ userId: string }>).filter(
          (r) => r.userId === "ann",
        ),
        table,
      ).toEqual([]);
    }
    expect(after.pools.filter((p) => p.createdById === "ann")).toEqual([]);
    // Her pool went with everyone's membership of it.
    expect(after.members.filter((m) => m.poolId === annsPools[0]!.id)).toEqual(
      [],
    );
    // Her sync limit stays too: deleting can't be used to reset it.
    expect(after.rateLimits.map((r) => r.key)).toContain("sync:ann");

    // Everyone else's rows, in every table, are exactly as they were: the shared account keys,
    // the per-address rows and Bob's and Cat's own pools included. Ann's failed share codes stay
    // too: they are the guess limit, not her data, and expire within the hour.
    expect(othersRows(after, "ann")).toEqual(othersRows(before, "ann"));
    expect(after.accounts).toEqual(before.accounts);
    expect(after.ipFailures).toEqual(before.ipFailures);
    expect(after.failures).toEqual(before.failures);
    expect(after.failures.filter((f) => f.userId === "ann")).toHaveLength(2);
    expect(after.rateLimits.map((r) => r.key)).toContain("sync:bob");

    // Ann now sees nothing; Bob still has his own account and pool, now just his.
    expect(
      await listAccounts(db, await loadAccessScope(db, "ann"), AFTER_FIXTURE),
    ).toEqual([]);
    const [bobsK1] = await listAccounts(
      db,
      await loadAccessScope(db, "bob"),
      AFTER_FIXTURE,
    );
    expect(bobsK1).toMatchObject({ key: K1, pooled: false, memberCount: 1 });
    expect(bobsK1!.last30Days.sessions).toBe(1);
  });

  it("deletes the account keys nobody else refers to, and keeps the shared ones", async () => {
    await applySync(db, "ann", annsOwnAccount(), AFTER_FIXTURE);
    await createPoolCode(db, "ann", K3);
    // Sorted in JavaScript, never by the database's collation.
    const keys = async () =>
      (await db.claudeAccount.findMany({ select: { key: true } }))
        .map((a) => a.key)
        .sort();
    expect(await keys()).toEqual([K3, K1, K2].sort());

    const deleted = await deleteSyncedData(db, "ann");
    expect(deleted).toMatchObject({ accounts: 3, accountKeys: 1 });
    // K3 was Ann's alone; K1 and K2 are still Bob's and Cat's.
    expect(await keys()).toEqual([K1, K2].sort());
    expect(
      await db.userAccount.count({ where: { accountKey: { in: [K1, K2] } } }),
    ).toBe(2);

    // Syncing K3 again brings it back.
    await applySync(db, "ann", annsOwnAccount(), AFTER_FIXTURE);
    expect(await keys()).toEqual([K3, K1, K2].sort());
  });

  it("keeps an account key another person's sync is storing rows for right now", async () => {
    await applySync(db, "ann", annsOwnAccount(), AFTER_FIXTURE);
    // Bob's sync of K3 is under way: its account report is written, not yet committed.
    let written!: () => void;
    let commit!: () => void;
    const writing = new Promise<void>((resolve) => (written = resolve));
    const committing = new Promise<void>((resolve) => (commit = resolve));
    const bobsSync = db.$transaction(
      async (tx) => {
        await tx.userAccount.create({
          data: { userId: "bob", accountKey: K3, label: "Bob's too" },
        });
        written();
        await committing;
      },
      { timeout: 20_000 },
    );
    await writing;

    // Ann's deletion doesn't wait for Bob's sync, and leaves K3 to it.
    const deletion = deleteSyncedData(db, "ann");
    const outcome = await Promise.race([
      deletion.then((d) => d),
      new Promise<"waited">((resolve) =>
        setTimeout(() => resolve("waited"), 5_000),
      ),
    ]);
    commit();
    await bobsSync;
    expect(outcome).toMatchObject({ accounts: 3, accountKeys: 0 });
    await deletion;

    // Bob's report of K3 survived Ann's deletion, and so did the key.
    expect(
      await db.userAccount.findMany({
        where: { accountKey: K3 },
        select: { userId: true, label: true },
      }),
    ).toEqual([{ userId: "bob", label: "Bob's too" }]);
    expect(await db.claudeAccount.count({ where: { key: K3 } })).toBe(1);
  });

  it("keeps the share-code guess limit", async () => {
    // Dan never synced anything, so deleting his data costs him nothing.
    await createUser("dan", "Dan");
    const { code } = await createPoolCode(db, "cat", K2);
    const at = new Date();
    for (let i = 0; i < JOIN_FAILURE_LIMIT; i++) {
      expect(await denied(joinPool(db, "dan", UNKNOWN_CODE, at))).toBe(
        "NOT_FOUND",
      );
    }
    expect(await denied(joinPool(db, "dan", code, at))).toBe(
      "TOO_MANY_REQUESTS",
    );

    await deleteSyncedData(db, "dan");
    // Still refused, the right code too, until the hour is up.
    expect(await denied(joinPool(db, "dan", code, at))).toBe(
      "TOO_MANY_REQUESTS",
    );
    expect(await db.poolJoinFailure.count({ where: { userId: "dan" } })).toBe(
      JOIN_FAILURE_LIMIT,
    );
  });

  it("starts afresh when a Mac syncs again", async () => {
    await deleteSyncedData(db, "ann");
    await applySync(db, "ann", request(), AFTER_FIXTURE);
    const accounts = await listAccounts(
      db,
      await loadAccessScope(db, "ann"),
      AFTER_FIXTURE,
    );
    // Most recently active first; no pool of before comes back.
    expect(
      accounts.map((a) => [a.key, a.pooled, a.last30Days.sessions]),
    ).toEqual([
      [K2, false, 1],
      [K1, false, 1],
    ]);
  });

  it("waits for a sync of the viewer's that is under way", async () => {
    let locked!: () => void;
    let release!: () => void;
    const holding = new Promise<void>((resolve) => (locked = resolve));
    const released = new Promise<void>((resolve) => (release = resolve));
    // Stands in for a sync: it holds the same lock until released.
    const sync = db.$transaction(
      async (tx) => {
        await lockUserData(tx, "ann");
        locked();
        await released;
      },
      { timeout: 20_000 },
    );
    await holding;

    let finished = false;
    const deletion = deleteSyncedData(db, "ann").then((deleted) => {
      finished = true;
      return deleted;
    });
    await new Promise((resolve) => setTimeout(resolve, 300));
    expect(finished).toBe(false);
    expect(await db.session.count({ where: { userId: "ann" } })).toBe(2);

    release();
    await sync;
    expect((await deletion).sessions).toBe(2);
    expect(await db.session.count({ where: { userId: "ann" } })).toBe(0);
  });
});

describe("the tRPC mutations", () => {
  it("need a signed-in viewer", async () => {
    const anon = caller(null);
    expect(
      await trpcCode(
        anon.myData.removeSummaries({ confirm: REMOVE_SUMMARIES_CONFIRMATION }),
      ),
    ).toBe("UNAUTHORIZED");
    expect(
      await trpcCode(
        anon.myData.deleteAll({ confirm: DELETE_DATA_CONFIRMATION }),
      ),
    ).toBe("UNAUTHORIZED");
  });

  it("need the phrase, exactly, and change nothing without it", async () => {
    const before = await everything();
    const ann = caller({ id: "ann" });
    for (const confirm of [
      "",
      "delete",
      "DELETE MY DATA",
      "remove summaries",
    ]) {
      expect(
        await trpcCode(ann.myData.deleteAll({ confirm } as never)),
        confirm,
      ).toBe("BAD_REQUEST");
    }
    expect(
      await trpcCode(
        ann.myData.removeSummaries({
          confirm: DELETE_DATA_CONFIRMATION,
        } as never),
      ),
    ).toBe("BAD_REQUEST");
    expect(await everything()).toEqual(before);
  });

  it("act on the signed-in viewer's own data", async () => {
    const bob = caller({ id: "bob" });
    expect(
      await bob.myData.removeSummaries({
        confirm: REMOVE_SUMMARIES_CONFIRMATION,
      }),
    ).toEqual({ sessions: 1 });
    expect(
      await db.session.count({
        where: { userId: "ann", summaryText: { not: null } },
      }),
    ).toBe(1);

    const deleted = await bob.myData.deleteAll({
      confirm: DELETE_DATA_CONFIRMATION,
    });
    expect(deleted).toMatchObject({
      sessions: 1,
      poolsDeleted: 1,
      poolsLeft: 1,
    });
    expect(await db.session.count({ where: { userId: "bob" } })).toBe(0);
    expect(await db.session.count({ where: { userId: "ann" } })).toBe(2);
  });
});
