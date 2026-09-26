/**
 * Pools end to end against Postgres: codes, joining, revoking, leaving, removing, and what each
 * person can read through the dashboard services at every step.
 */
import { beforeEach, describe, expect, it } from "vitest";

import { AccessDenied, loadAccessScope } from "~/server/services/access";
import {
  getAccount,
  listAccounts,
  MAX_WINDOWS_PER_ACCOUNT,
} from "~/server/services/accounts";
import { formatPoolCode, POOL_CODE_TTL_MS } from "~/server/services/pool-code";
import { ipKey } from "~/server/client-ip";
import {
  createPoolCode,
  JOIN_FAILURE_LIMIT,
  JOIN_FAILURE_PRUNE_BATCH,
  JOIN_FAILURE_WINDOW_MS,
  JOIN_IP_FAILURE_LIMIT,
  joinPool,
  leavePool,
  listPools,
  removePoolMember,
  revokePoolCode,
} from "~/server/services/pools";
import { getProject, listProjects } from "~/server/services/projects";
import { getSession, listSessions } from "~/server/services/sessions";
import { applySync } from "~/server/services/sync";
import { usageHistory } from "~/server/services/usage";

import { createUser, db, resetDb } from "./db";
import { AFTER_FIXTURE, K1, K2, request, requestFor, S1, S2 } from "./seed";

const BOB_SESSION = "b0b0b0b0-0000-4000-8000-000000000001";
/** A well-formed code nobody holds. */
const UNKNOWN_CODE = "ZZZZ-ZZZZ-ZZZZ";
const CAT_SESSION = "cacacaca-0000-4000-8000-000000000001";
const WEEK_AGO = new Date("2026-09-19T00:00:00Z");

const scope = (userId: string) => loadAccessScope(db, userId);

async function sessionIds(
  userId: string,
  filter: { accountKey?: string } = {},
) {
  const page = await listSessions(db, await scope(userId), {
    ...filter,
    limit: 100,
  });
  return page.items.map((s) => s.sessionId).sort();
}

async function denied(promise: Promise<unknown>): Promise<string> {
  try {
    await promise;
  } catch (error) {
    if (error instanceof AccessDenied) return error.code;
    throw error;
  }
  throw new Error("expected an AccessDenied");
}

beforeEach(async () => {
  await resetDb();
  await createUser("ann", "Ann");
  await createUser("bob", "Bob");
  await createUser("cat", null); // shown by her email's local part
  await createUser("dan", "Dan");

  // Ann syncs both fixture accounts; Bob uses Ann's personal account (K1); Cat the company one.
  await applySync(db, "ann", request(), AFTER_FIXTURE);
  await applySync(
    db,
    "bob",
    requestFor({
      deviceId: "11111111-2222-4333-8444-555555555555",
      accountKey: K1,
      sessionId: BOB_SESSION,
      projectKey: "b".repeat(64),
      projectName: "bobs-app",
      observedAt: "2026-09-25T12:00:00Z",
    }),
    AFTER_FIXTURE,
  );
  await applySync(
    db,
    "cat",
    requestFor({
      deviceId: "33333333-2222-4333-8444-555555555555",
      accountKey: K2,
      sessionId: CAT_SESSION,
      projectKey: "c".repeat(64),
      projectName: "cats-api",
    }),
    AFTER_FIXTURE,
  );
});

describe("before any pool", () => {
  it("everyone sees only their own sessions and accounts", async () => {
    expect(await sessionIds("ann")).toEqual([S2, S1].sort());
    expect(await sessionIds("bob")).toEqual([BOB_SESSION]);
    expect(await sessionIds("cat")).toEqual([CAT_SESSION]);
    expect(await sessionIds("dan")).toEqual([]);

    const bobAccounts = await listAccounts(
      db,
      await scope("bob"),
      AFTER_FIXTURE,
    );
    expect(bobAccounts.map((a) => [a.key, a.pooled, a.memberCount])).toEqual([
      [K1, false, 1],
    ]);
    expect(await listAccounts(db, await scope("dan"), AFTER_FIXTURE)).toEqual(
      [],
    );
  });

  it("hides other people's rows by id and by account", async () => {
    const annS1 = await db.session.findUniqueOrThrow({
      where: {
        userId_accountKey_sessionId: {
          userId: "ann",
          accountKey: K1,
          sessionId: S1,
        },
      },
    });
    expect(await denied(getSession(db, await scope("bob"), annS1.id))).toBe(
      "NOT_FOUND",
    );
    expect(
      await denied(getProject(db, await scope("bob"), annS1.projectId)),
    ).toBe("NOT_FOUND");
    expect(
      await denied(
        listSessions(db, await scope("dan"), { accountKey: K1, limit: 10 }),
      ),
    ).toBe("NOT_FOUND");
    expect(await denied(listProjects(db, await scope("dan"), K1))).toBe(
      "NOT_FOUND",
    );
    expect(await denied(getAccount(db, await scope("dan"), K1))).toBe(
      "NOT_FOUND",
    );
    expect(
      await denied(
        usageHistory(db, await scope("dan"), { accountKey: K1 }, AFTER_FIXTURE),
      ),
    ).toBe("NOT_FOUND");
    // Filtering by someone else's id or project changes nothing.
    const page = await listSessions(db, await scope("bob"), {
      ownerId: "ann",
      projectId: annS1.projectId,
      limit: 10,
    });
    expect(page.items).toEqual([]);
  });

  it("needs a synced account to create a code", async () => {
    expect(await denied(createPoolCode(db, "bob", K2))).toBe("FORBIDDEN");
    expect(await denied(createPoolCode(db, "dan", K1))).toBe("FORBIDDEN");
    expect(await db.pool.count()).toBe(0);
  });
});

describe("a pool on Ann's personal account", () => {
  async function annAndBob() {
    const { code, poolId, created, expiresAt } = await createPoolCode(
      db,
      "ann",
      K1,
    );
    expect(created).toBe(true);
    expect(code).toMatch(/^[0-9A-HJKMNP-TV-Z]{12}$/);
    const lifetime = expiresAt.getTime() - Date.now();
    expect(lifetime).toBeLessThanOrEqual(POOL_CODE_TTL_MS);
    expect(lifetime).toBeGreaterThan(POOL_CODE_TTL_MS - 60_000);
    // Typed back loosely: lowercase, with the dashes it's shown with.
    const typed = formatPoolCode(code).toLowerCase();
    expect(await joinPool(db, "bob", typed)).toEqual({
      poolId,
      accountKey: K1,
      joined: true,
    });
    return { code, poolId };
  }

  it("has one active code per account and creator", async () => {
    const first = await createPoolCode(db, "ann", K1);
    const again = await createPoolCode(db, "ann", K1);
    expect(again).toEqual({ ...first, created: false });

    // Concurrent requests still make one pool.
    const results = await Promise.all(
      Array.from({ length: 5 }, () => createPoolCode(db, "ann", K2)),
    );
    expect(new Set(results.map((r) => r.code)).size).toBe(1);
    expect(await db.pool.count({ where: { accountKey: K2 } })).toBe(1);

    // The creator is the first member.
    const members = await db.poolMember.findMany({
      where: { poolId: first.poolId },
    });
    expect(members.map((m) => m.userId)).toEqual(["ann"]);
  });

  it("shares that account's sessions, usage and projects both ways, and nothing else", async () => {
    await annAndBob();

    expect(await sessionIds("ann")).toEqual([BOB_SESSION, S2, S1].sort());
    expect(await sessionIds("bob")).toEqual([BOB_SESSION, S1].sort()); // not Ann's K2 session
    expect(await sessionIds("bob", { accountKey: K1 })).toEqual(
      [BOB_SESSION, S1].sort(),
    );
    expect(
      await denied(
        listSessions(db, await scope("bob"), { accountKey: K2, limit: 10 }),
      ),
    ).toBe("NOT_FOUND");
    expect(await sessionIds("cat")).toEqual([CAT_SESSION]);
    expect(await sessionIds("dan")).toEqual([]);

    // Pooled sessions carry their owner's email (with their name beside it), and no device.
    const bobsView = await listSessions(db, await scope("bob"), {
      accountKey: K1,
      limit: 10,
    });
    const annsSession = bobsView.items.find((s) => s.sessionId === S1)!;
    expect(annsSession.owner).toEqual({
      id: "ann",
      displayName: "ann@example.com",
      name: "Ann",
      isViewer: false,
    });
    expect(annsSession.device).toBeNull();
    expect(annsSession.summary?.text).toContain("stays 'working'");
    const bobsOwn = bobsView.items.find((s) => s.sessionId === BOB_SESSION)!;
    expect(bobsOwn.owner.isViewer).toBe(true);
    expect(bobsOwn.device).toEqual({
      id: "11111111-2222-4333-8444-555555555555",
      name: "Other Mac",
    });

    // Filters narrow within what is visible.
    const onlyAnn = await listSessions(db, await scope("bob"), {
      ownerId: "ann",
      limit: 10,
    });
    expect(onlyAnn.items.map((s) => s.sessionId)).toEqual([S1]);
    const search = await listSessions(db, await scope("bob"), {
      search: "BACKGROUND",
      limit: 10,
    });
    expect(search.items.map((s) => s.sessionId)).toEqual([S1]);

    // Account totals include both people.
    const [k1] = await listAccounts(db, await scope("bob"), AFTER_FIXTURE);
    expect(k1).toMatchObject({
      key: K1,
      pooled: true,
      memberCount: 2,
      syncedByViewer: true,
      label: "Theirs", // Bob's own label, never Ann's
      email: "me@example.com",
    });
    expect(k1!.last7Days.sessions).toBe(2);
    expect(k1!.last7Days.tokens.input).toBe(18234n + 100n);
    expect(k1!.last7Days.tokens.total).toBe(
      18234n + 96512n + 402118n + 12873120n + 100n + 200n + 300n + 400n,
    );
    expect(k1!.last30Days.costUsd).toBeCloseTo(14.82 * 2, 6);
    // Latest reading per window, across both people's Macs (Bob's is newer).
    expect(
      k1!.usage.map((u) => [u.windowId, u.observedAt.toISOString()]),
    ).toEqual([
      ["session", "2026-09-25T12:00:00.000Z"],
      ["weekly_all", "2026-09-25T12:00:00.000Z"],
      ["weekly_opus", "2026-09-25T12:00:00.000Z"],
    ]);

    const detail = await getAccount(db, await scope("ann"), K1, AFTER_FIXTURE);
    expect(detail.members.map((m) => [m.displayName, m.name])).toEqual([
      ["ann@example.com", "Ann"],
      ["bob@example.com", "Bob"],
    ]);

    // Usage history has both people's readings.
    const history = await usageHistory(
      db,
      await scope("bob"),
      { accountKey: K1, windowId: "session", from: WEEK_AGO },
      AFTER_FIXTURE,
    );
    expect(history.windows).toHaveLength(1);
    expect(
      history.windows[0]!.points.map((p) => p.observedAt.toISOString()),
    ).toEqual(["2026-09-25T09:50:00.000Z", "2026-09-25T12:00:00.000Z"]);

    // Projects of both people, each with its owner.
    const projects = await listProjects(db, await scope("bob"), K1);
    expect(
      projects.map((p) => [p.name, p.owner.displayName, p.sessionCount]),
    ).toEqual(
      expect.arrayContaining([
        ["agentnotch", "ann@example.com", 1],
        ["bobs-app", "bob@example.com", 1],
      ]),
    );
    // Nobody is ever shown a project key, not even their own.
    for (const project of projects) expect(project).not.toHaveProperty("key");
    const annsProject = projects.find((p) => p.name === "agentnotch")!;
    expect(annsProject.latestSummaries.map((s) => s.text)).toHaveLength(1);
    expect(annsProject.tokens.cacheRead).toBe(12873120n);

    // Bob never synced K2 and sees nothing of it.
    expect(await denied(listProjects(db, await scope("bob"), K2))).toBe(
      "NOT_FOUND",
    );
  });

  it("lets someone who never synced the account see it through the pool", async () => {
    const { code } = await annAndBob();
    await joinPool(db, "dan", code);
    const accounts = await listAccounts(db, await scope("dan"), AFTER_FIXTURE);
    expect(
      accounts.map((a) => [a.key, a.syncedByViewer, a.email, a.label]),
    ).toEqual([[K1, false, "me@example.com", null]]);
    expect(await sessionIds("dan")).toEqual([BOB_SESSION, S1].sort());
    // But Dan can't share it on.
    expect(await denied(createPoolCode(db, "dan", K1))).toBe("FORBIDDEN");
  });

  it("joining twice is one membership", async () => {
    const { code, poolId } = await annAndBob();
    expect(await joinPool(db, "bob", code)).toEqual({
      poolId,
      accountKey: K1,
      joined: false,
    });
    expect(await joinPool(db, "ann", code)).toEqual({
      poolId,
      accountKey: K1,
      joined: false,
    });
    await Promise.all([joinPool(db, "dan", code), joinPool(db, "dan", code)]);
    expect(await db.poolMember.count({ where: { poolId } })).toBe(3);
  });

  it("refuses unknown and malformed codes", async () => {
    await annAndBob();
    expect(await denied(joinPool(db, "dan", UNKNOWN_CODE))).toBe("NOT_FOUND");
    expect(await denied(joinPool(db, "dan", "short"))).toBe("BAD_REQUEST");
    // The old 8-character form isn't a code any more.
    expect(await denied(joinPool(db, "dan", "7K3M-9QX2"))).toBe("BAD_REQUEST");
    expect(await sessionIds("dan")).toEqual([]);
  });

  it("revoking the code stops new joins but keeps members", async () => {
    const { code, poolId } = await annAndBob();
    expect(await denied(revokePoolCode(db, "bob", poolId))).toBe("FORBIDDEN");
    expect(await denied(revokePoolCode(db, "dan", poolId))).toBe("NOT_FOUND");
    expect(await revokePoolCode(db, "ann", poolId)).toEqual({ revoked: true });
    expect(await revokePoolCode(db, "ann", poolId)).toEqual({ revoked: false });

    // To anyone new, a revoked code is indistinguishable from one that never existed.
    const unknown = await joinPool(db, "dan", UNKNOWN_CODE).catch(
      (e: unknown) => e,
    );
    const revoked = await joinPool(db, "dan", code).catch((e: unknown) => e);
    expect(revoked).toBeInstanceOf(AccessDenied);
    expect(revoked).toMatchObject({
      code: (unknown as AccessDenied).code,
      message: (unknown as AccessDenied).message,
    });
    expect(await sessionIds("dan")).toEqual([]);
    // Bob stays in, and "joining" again is harmless.
    expect(await sessionIds("bob")).toEqual([BOB_SESSION, S1].sort());
    expect((await joinPool(db, "bob", code)).joined).toBe(false);

    // A new code belongs to the same pool: whoever joins with it shares with Bob too.
    const next = await createPoolCode(db, "ann", K1);
    expect(next).toMatchObject({ created: true, poolId });
    expect(next.code).not.toBe(code);
    await joinPool(db, "dan", next.code);
    expect(await sessionIds("dan")).toEqual([BOB_SESSION, S1].sort());
    expect(await db.pool.count()).toBe(1);
    // The old code is gone for good.
    expect(await denied(joinPool(db, "cat", code))).toBe("NOT_FOUND");
  });

  it("codes expire after 7 days; a new one keeps the same pool", async () => {
    const issued = new Date("2026-09-25T12:00:00Z");
    const first = await createPoolCode(db, "ann", K1, undefined, issued);
    expect(first.expiresAt).toEqual(
      new Date(issued.getTime() + POOL_CODE_TTL_MS),
    );
    const justBefore = new Date(first.expiresAt.getTime() - 1);
    expect((await joinPool(db, "bob", first.code, justBefore)).joined).toBe(
      true,
    );

    // Expired: refused like an unknown code, but members still "join" harmlessly.
    expect(await denied(joinPool(db, "dan", first.code, first.expiresAt))).toBe(
      "NOT_FOUND",
    );
    expect(
      (await joinPool(db, "bob", first.code, first.expiresAt)).joined,
    ).toBe(false);
    const [lapsed] = await listPools(db, "ann", first.expiresAt);
    expect(lapsed!.share).toEqual({
      code: null,
      status: "expired",
      expiresAt: first.expiresAt,
      revokedAt: null,
    });

    // Asking again renews the code for the same pool.
    const renewed = await createPoolCode(
      db,
      "ann",
      K1,
      undefined,
      first.expiresAt,
    );
    expect(renewed).toMatchObject({ poolId: first.poolId, created: true });
    expect(renewed.code).not.toBe(first.code);
    expect(
      (await joinPool(db, "dan", renewed.code, first.expiresAt)).joined,
    ).toBe(true);
    expect(await sessionIds("dan")).toEqual([BOB_SESSION, S1].sort());
  });

  it("the creator removes members; members can't", async () => {
    const { code, poolId } = await annAndBob();
    await joinPool(db, "dan", code);
    expect(await denied(removePoolMember(db, "bob", poolId, "dan"))).toBe(
      "FORBIDDEN",
    );
    expect(await denied(removePoolMember(db, "ann", poolId, "ann"))).toBe(
      "BAD_REQUEST",
    );
    expect(await removePoolMember(db, "ann", poolId, "bob")).toEqual({
      removed: true,
      codeRevoked: true,
    });
    expect(await removePoolMember(db, "ann", poolId, "bob")).toEqual({
      removed: false,
      codeRevoked: false,
    });

    expect(await sessionIds("bob")).toEqual([BOB_SESSION]);
    expect(await sessionIds("ann")).toEqual([S2, S1].sort());
    expect(await sessionIds("dan")).toEqual([S1]);
    // Bob still has his own account.
    const accounts = await listAccounts(db, await scope("bob"), AFTER_FIXTURE);
    expect(accounts.map((a) => [a.key, a.pooled])).toEqual([[K1, false]]);
  });

  it("a removed member can't rejoin with a code made before the removal", async () => {
    const { code, poolId } = await annAndBob();
    await removePoolMember(db, "ann", poolId, "bob");

    // Removing him revoked the code he knows.
    expect(await denied(joinPool(db, "bob", code))).toBe("NOT_FOUND");
    expect(await sessionIds("bob")).toEqual([BOB_SESSION]);
    const [annsPool] = await listPools(db, "ann");
    expect(annsPool!.share).toMatchObject({ status: "revoked", code: null });

    // A new code is Ann's choice to hand out; the old one stays dead.
    const next = await createPoolCode(db, "ann", K1);
    expect(next.poolId).toBe(poolId);
    expect(await denied(joinPool(db, "bob", code))).toBe("NOT_FOUND");
    expect((await joinPool(db, "dan", next.code)).joined).toBe(true);
  });

  it("limits failed redemptions per user, then refuses even the right code", async () => {
    const { code } = await annAndBob();
    const start = new Date("2026-09-25T12:00:00Z");
    for (let i = 0; i < JOIN_FAILURE_LIMIT; i++) {
      expect(
        await denied(
          joinPool(db, "dan", UNKNOWN_CODE, new Date(start.getTime() + i)),
        ),
      ).toBe("NOT_FOUND");
    }
    // Refused without looking the code up: a right guess can't be told from a wrong one.
    const later = new Date(start.getTime() + 60_000);
    expect(await denied(joinPool(db, "dan", code, later))).toBe(
      "TOO_MANY_REQUESTS",
    );
    expect(await denied(joinPool(db, "dan", UNKNOWN_CODE, later))).toBe(
      "TOO_MANY_REQUESTS",
    );
    expect(await sessionIds("dan")).toEqual([]);
    // Malformed input isn't a guess and isn't counted; other people aren't affected.
    expect(await denied(joinPool(db, "cat", "short", later))).toBe(
      "BAD_REQUEST",
    );
    expect((await joinPool(db, "cat", code, later)).joined).toBe(true);

    // An hour after the failures, Dan may try again.
    const nextHour = new Date(start.getTime() + JOIN_FAILURE_WINDOW_MS + 10);
    expect((await joinPool(db, "dan", code, nextHour)).joined).toBe(true);
    // Only the last hour's failures are kept.
    expect(await db.poolJoinFailure.count()).toBe(0);
  });

  it("counts failures exactly under concurrent guesses", async () => {
    await annAndBob();
    const at = new Date("2026-09-25T12:00:00Z");
    const outcomes = await Promise.all(
      Array.from({ length: JOIN_FAILURE_LIMIT + 5 }, () =>
        denied(joinPool(db, "dan", UNKNOWN_CODE, at)),
      ),
    );
    expect(outcomes.filter((c) => c === "NOT_FOUND")).toHaveLength(
      JOIN_FAILURE_LIMIT,
    );
    expect(outcomes.filter((c) => c === "TOO_MANY_REQUESTS")).toHaveLength(5);
  });

  it("limits failed redemptions per IP address too, whoever is signed in", async () => {
    const { code } = await annAndBob();
    for (const id of ["eve", "fay", "gus", "hal"]) await createUser(id);
    const office = ipKey("203.0.113.7", undefined);
    const start = new Date("2026-09-25T12:00:00Z");
    // 30 failures from one address, nobody past their own 10.
    let tries = 0;
    for (const [userId, count] of [
      ["dan", 9],
      ["cat", 9],
      ["eve", 9],
      ["fay", 3],
    ] as const) {
      for (let i = 0; i < count; i++) {
        const at = new Date(start.getTime() + tries++);
        expect(
          await denied(joinPool(db, userId, UNKNOWN_CODE, at, office)),
        ).toBe("NOT_FOUND");
      }
    }
    expect(tries).toBe(JOIN_IP_FAILURE_LIMIT);

    // Someone without a failure of their own, from that address: refused, the right code too.
    const later = new Date(start.getTime() + 60_000);
    const refused = await joinPool(db, "gus", code, later, office).catch(
      (e: unknown) => e,
    );
    expect(refused).toMatchObject({
      code: "TOO_MANY_REQUESTS",
      message: expect.stringContaining("from this network") as unknown,
    });
    // From another address, or with none known, the right code works.
    const home = ipKey("198.51.100.1", undefined);
    expect((await joinPool(db, "gus", code, later, home)).joined).toBe(true);
    expect((await joinPool(db, "hal", code, later, null)).joined).toBe(true);

    // Only the address's hash is stored, and the rows name nobody.
    const rows = await db.poolJoinIpFailure.findMany();
    expect(rows).toHaveLength(JOIN_IP_FAILURE_LIMIT);
    expect(new Set(rows.map((r) => r.ipKey))).toEqual(new Set([office]));
    expect(
      JSON.stringify(rows, (_, v: unknown) =>
        typeof v === "bigint" ? String(v) : v,
      ),
    ).not.toContain("203.0.113.7");

    // An hour after the failures, the address may try again.
    const nextHour = new Date(start.getTime() + JOIN_FAILURE_WINDOW_MS + 60);
    expect((await joinPool(db, "eve", code, nextHour, office)).joined).toBe(
      true,
    );
    expect(await db.poolJoinIpFailure.count()).toBe(0);
  });

  it("counts an address's failures exactly under concurrent guesses by several people", async () => {
    await annAndBob();
    for (const id of ["eve", "fay"]) await createUser(id);
    const office = ipKey("203.0.113.7", undefined);
    const at = new Date("2026-09-25T12:00:00Z");
    // 9 each from four people: 36 guesses, each person under their own limit.
    const outcomes = await Promise.all(
      ["dan", "cat", "eve", "fay"].flatMap((userId) =>
        Array.from({ length: 9 }, () =>
          denied(joinPool(db, userId, UNKNOWN_CODE, at, office)),
        ),
      ),
    );
    expect(outcomes.filter((c) => c === "NOT_FOUND")).toHaveLength(
      JOIN_IP_FAILURE_LIMIT,
    );
    expect(outcomes.filter((c) => c === "TOO_MANY_REQUESTS")).toHaveLength(6);
  });

  it("deletes failures past the hour on each attempt, a batch at a time, oldest first", async () => {
    const start = new Date("2026-09-25T12:00:00Z");
    const office = ipKey("203.0.113.7", undefined);
    // 150 failures from long ago, per user and per address, and 3 of each from this hour.
    const old = JOIN_FAILURE_PRUNE_BATCH + 50;
    const longAgo = (i: number) =>
      new Date(start.getTime() - 2 * JOIN_FAILURE_WINDOW_MS + i);
    const recent = (i: number) => new Date(start.getTime() - 60_000 + i);
    await db.poolJoinFailure.createMany({
      data: [
        ...Array.from({ length: old }, (_, i) => ({
          userId: "cat",
          at: longAgo(i),
        })),
        ...Array.from({ length: 3 }, (_, i) => ({
          userId: "dan",
          at: recent(i),
        })),
      ],
    });
    await db.poolJoinIpFailure.createMany({
      data: [
        ...Array.from({ length: old }, (_, i) => ({
          ipKey: office,
          at: longAgo(i),
        })),
        ...Array.from({ length: 3 }, (_, i) => ({
          ipKey: office,
          at: recent(i),
        })),
      ],
    });
    const past = () => ({ at: { lte: longAgo(old) } });
    const oldest = async () =>
      (
        await db.poolJoinFailure.findFirst({
          where: past(),
          orderBy: { at: "asc" },
        })
      )?.at;

    // One attempt deletes one batch of each table, the oldest; the hour's failures still count.
    expect(await denied(joinPool(db, "dan", UNKNOWN_CODE, start, office))).toBe(
      "NOT_FOUND",
    );
    expect(await db.poolJoinFailure.count({ where: past() })).toBe(50);
    expect(await db.poolJoinIpFailure.count({ where: past() })).toBe(50);
    expect(await oldest()).toEqual(longAgo(JOIN_FAILURE_PRUNE_BATCH));
    expect(await db.poolJoinFailure.count({ where: { userId: "dan" } })).toBe(
      4,
    );

    // The next deletes the rest.
    expect(await denied(joinPool(db, "dan", UNKNOWN_CODE, start, office))).toBe(
      "NOT_FOUND",
    );
    expect(await db.poolJoinFailure.count({ where: past() })).toBe(0);
    expect(await db.poolJoinIpFailure.count({ where: past() })).toBe(0);
    expect(await db.poolJoinFailure.count()).toBe(5);
    expect(await db.poolJoinIpFailure.count()).toBe(5);
  });

  it("describes a pooled account by the creator's report, never another member's", async () => {
    const { code } = await annAndBob();
    // Bob's app reports the account differently, and more recently than Ann's.
    await applySync(
      db,
      "bob",
      request((r) => {
        r.device.id = "11111111-2222-4333-8444-555555555555";
        r.accounts = [
          {
            key: K1,
            email: "spoofed@evil.example",
            organizationName: "Evil Corp",
            plan: "Free",
            label: "Bob's",
          },
        ];
        r.sessions = [];
        r.usage = [];
      }),
      new Date(AFTER_FIXTURE.getTime() + 60_000),
    );
    await joinPool(db, "dan", code);

    const [dans] = await listAccounts(db, await scope("dan"), AFTER_FIXTURE);
    expect(dans).toMatchObject({
      email: "me@example.com",
      plan: "Max 20x",
      organizationName: null,
      label: null,
    });
    const [dansPool] = await listPools(db, "dan");
    expect(dansPool!.account).toMatchObject({ email: "me@example.com" });
    // Bob still sees what his own app said, and Ann hers.
    const [bobs] = await listAccounts(db, await scope("bob"), AFTER_FIXTURE);
    expect(bobs).toMatchObject({
      email: "spoofed@evil.example",
      label: "Bob's",
    });
    const anns = await getAccount(db, await scope("ann"), K1, AFTER_FIXTURE);
    expect(anns).toMatchObject({ email: "me@example.com", label: "Personal" });
  });

  it("caps the meters per account, keeping the real windows", async () => {
    const { code } = await annAndBob();
    await joinPool(db, "dan", code);
    // Bob's app reports 40 made-up windows, newer than Ann's real ones.
    for (const batch of [0, 1]) {
      await applySync(
        db,
        "bob",
        request((r) => {
          r.device.id = "11111111-2222-4333-8444-555555555555";
          r.accounts = [r.accounts[0]!];
          r.sessions = [];
          r.usage = [
            {
              accountKey: K1,
              source: "probe",
              observedAt: "2026-09-25T12:30:00Z",
              windows: Array.from({ length: 20 }, (_, i) => ({
                id: `weekly_bogus_${batch}_${i}`,
                utilization: 100,
                resetsAt: null,
              })),
            },
          ];
        }),
        AFTER_FIXTURE,
      );
    }
    // Sync kept Bob to 32 window ids on the account: his 3 real ones and 29 made up.
    expect(
      await db.usageWindow.count({ where: { userId: "bob", accountKey: K1 } }),
    ).toBe(32);

    const [k1] = await listAccounts(db, await scope("dan"), AFTER_FIXTURE);
    const ids = k1!.usage.map((u) => u.windowId);
    expect(ids).toHaveLength(MAX_WINDOWS_PER_ACCOUNT);
    // The fixed windows, then the per-model window Ann (the pool's creator) reported, although
    // Bob's made-up ones are newer; then the rest by id.
    expect(ids.slice(0, 4)).toEqual([
      "session",
      "weekly_all",
      "weekly_opus",
      "weekly_bogus_0_0",
    ]);
  });

  it("keeps a member's future-dated reading from pinning everyone's meters", async () => {
    const { code } = await annAndBob();
    await joinPool(db, "dan", code);
    // A day ahead passes validation (clock skew)…
    await applySync(
      db,
      "bob",
      requestFor({
        deviceId: "11111111-2222-4333-8444-555555555555",
        accountKey: K1,
        sessionId: BOB_SESSION,
        projectKey: "b".repeat(64),
        projectName: "bobs-app",
        observedAt: "2026-09-26T06:00:00Z",
      }),
      AFTER_FIXTURE,
    );
    // …but "latest" only counts readings up to the viewer's now.
    const [k1] = await listAccounts(db, await scope("dan"), AFTER_FIXTURE);
    expect(
      k1!.usage.find((u) => u.windowId === "session")!.observedAt.toISOString(),
    ).toBe("2026-09-25T12:00:00.000Z");
  });

  it("members leave; the creator leaving deletes the pool", async () => {
    const { code, poolId } = await annAndBob();
    await joinPool(db, "dan", code);

    expect(await leavePool(db, "bob", poolId)).toEqual({ deletedPool: false });
    expect(await sessionIds("bob")).toEqual([BOB_SESSION]);
    expect(await sessionIds("dan")).toEqual([S1]);
    expect(await denied(leavePool(db, "bob", poolId))).toBe("NOT_FOUND");

    expect(await leavePool(db, "ann", poolId)).toEqual({ deletedPool: true });
    expect(await db.pool.count()).toBe(0);
    expect(await db.poolMember.count()).toBe(0);
    expect(await sessionIds("dan")).toEqual([]);
    expect(await listAccounts(db, await scope("dan"), AFTER_FIXTURE)).toEqual(
      [],
    );
    expect(await denied(joinPool(db, "cat", code))).toBe("NOT_FOUND");
  });

  it("lists pools with the code for the creator only, and members by email", async () => {
    const { code, poolId } = await annAndBob();
    await joinPool(db, "cat", code);

    const [annsPool] = await listPools(db, "ann");
    expect(annsPool).toMatchObject({
      id: poolId,
      accountKey: K1,
      role: "creator",
      share: { code, status: "active", revokedAt: null },
      createdBy: { id: "ann", displayName: "ann@example.com", name: "Ann" },
      account: { email: "me@example.com", plan: "Max 20x", label: "Personal" },
    });
    expect(
      annsPool!.members.map((m) => [
        m.displayName,
        m.name,
        m.isCreator,
        m.isViewer,
      ]),
    ).toEqual([
      ["ann@example.com", "Ann", true, true],
      ["bob@example.com", "Bob", false, false],
      ["cat@example.com", null, false, false], // no name: just her email
    ]);

    const [bobsPool] = await listPools(db, "bob");
    expect(bobsPool).toMatchObject({
      role: "member",
      share: null,
      account: { label: "Theirs" },
    });
    expect(JSON.stringify(bobsPool)).not.toContain(code);

    // Cat never synced K1: she sees the creator's description of it, without its label.
    const [catsPool] = await listPools(db, "cat");
    expect(catsPool!.account).toEqual({
      email: "me@example.com",
      organizationName: null,
      plan: "Max 20x",
      label: null,
    });
    expect(await listPools(db, "dan")).toEqual([]);
  });
});

describe("session paging", () => {
  it("pages newest first without gaps or repeats", async () => {
    const ids: string[] = [];
    let cursor: string | undefined;
    do {
      const page = await listSessions(db, await scope("ann"), {
        limit: 1,
        cursor,
      });
      ids.push(...page.items.map((s) => s.sessionId));
      cursor = page.nextCursor ?? undefined;
    } while (cursor);
    expect(ids).toEqual([S2, S1]); // S2 started later
  });
});
