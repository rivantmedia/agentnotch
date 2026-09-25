/**
 * The visibility and pool rules in src/server/services/access.ts, exhaustively over small worlds.
 */
import { describe, expect, it } from "vitest";

import {
  AccessDenied,
  buildAccessScope,
  canSeeAccount,
  canSeeRow,
  decideCreatePool,
  decideJoin,
  decideLeave,
  decideRemoveMember,
  decideRevoke,
  enforce,
  codeIsActive,
  isPooledWithOthers,
  ownedRowWhere,
  visibleAccountKeys,
  visibleOwnerIds,
  type OwnedRow,
  type OwnedRowWhere,
  type PoolFacts,
  type PoolMembership,
} from "~/server/services/access";

const K1 = "1".repeat(64);
const K2 = "2".repeat(64);
const K3 = "3".repeat(64);
const USERS = ["ann", "bob", "cat", "dan"] as const;
const ACCOUNTS = [K1, K2, K3] as const;

/** Evaluates the Prisma `where` that ownedRowWhere builds, the way Postgres would. */
function matches(where: OwnedRowWhere, row: OwnedRow): boolean {
  if ("OR" in where) {
    return where.OR.some((clause) =>
      "accountKey" in clause
        ? clause.accountKey === row.accountKey &&
          clause.userId.in.includes(row.userId)
        : clause.userId === row.userId,
    );
  }
  return (
    where.accountKey === row.accountKey && where.userId.in.includes(row.userId)
  );
}

/** The rule as the design states it, straight from the pools (no scope). */
function specCanSee(
  viewer: string,
  pools: PoolMembership[],
  row: OwnedRow,
): boolean {
  if (row.userId === viewer) return true;
  return pools.some(
    (p) =>
      p.accountKey === row.accountKey &&
      p.memberIds.includes(viewer) &&
      p.memberIds.includes(row.userId),
  );
}

const ALL_ROWS: OwnedRow[] = USERS.flatMap((userId) =>
  ACCOUNTS.map((accountKey) => ({ userId, accountKey })),
);

/** A few pool layouts that cover the interesting cases. */
const WORLDS: Array<{ name: string; pools: PoolMembership[] }> = [
  { name: "no pools", pools: [] },
  {
    name: "ann+bob on K1",
    pools: [{ poolId: "p1", accountKey: K1, memberIds: ["ann", "bob"] }],
  },
  {
    name: "two pools on K1 sharing ann",
    pools: [
      { poolId: "p1", accountKey: K1, memberIds: ["ann", "bob"] },
      { poolId: "p2", accountKey: K1, memberIds: ["ann", "cat"] },
    ],
  },
  {
    name: "pools on different accounts",
    pools: [
      { poolId: "p1", accountKey: K1, memberIds: ["ann", "bob"] },
      { poolId: "p2", accountKey: K2, memberIds: ["bob", "cat", "dan"] },
    ],
  },
  {
    name: "a pool with only its creator",
    pools: [{ poolId: "p1", accountKey: K3, memberIds: ["dan"] }],
  },
];

describe("visibility", () => {
  for (const world of WORLDS) {
    for (const viewer of USERS) {
      it(`${world.name}: ${viewer} sees exactly what the rule allows`, () => {
        const scope = buildAccessScope(viewer, [], world.pools);
        for (const row of ALL_ROWS) {
          const expected = specCanSee(viewer, world.pools, row);
          expect(canSeeRow(scope, row), JSON.stringify(row)).toBe(expected);
          // The database filter agrees with the in-memory rule…
          expect(matches(ownedRowWhere(scope), row), JSON.stringify(row)).toBe(
            expected,
          );
          // …and so does the per-account filter, on its account.
          for (const accountKey of ACCOUNTS) {
            expect(matches(ownedRowWhere(scope, { accountKey }), row)).toBe(
              expected && row.accountKey === accountKey,
            );
          }
        }
      });
    }
  }

  it("members of two pools on one account don't see each other through a third member", () => {
    const pools = WORLDS[2]!.pools;
    const bob = buildAccessScope("bob", [K1], pools);
    const cat = buildAccessScope("cat", [K1], pools);
    const ann = buildAccessScope("ann", [K1], pools);
    expect(canSeeRow(bob, { userId: "cat", accountKey: K1 })).toBe(false);
    expect(canSeeRow(cat, { userId: "bob", accountKey: K1 })).toBe(false);
    expect(canSeeRow(ann, { userId: "bob", accountKey: K1 })).toBe(true);
    expect(canSeeRow(ann, { userId: "cat", accountKey: K1 })).toBe(true);
  });

  it("a pool shares one account only, never the members' other accounts", () => {
    const scope = buildAccessScope("ann", [K1, K2], WORLDS[1]!.pools);
    expect(canSeeRow(scope, { userId: "bob", accountKey: K1 })).toBe(true);
    expect(canSeeRow(scope, { userId: "bob", accountKey: K2 })).toBe(false);
    expect(canSeeRow(scope, { userId: "bob", accountKey: K3 })).toBe(false);
  });

  it("ignores pools the viewer is not in, whatever the caller passes", () => {
    const scope = buildAccessScope("cat", [], WORLDS[1]!.pools);
    expect(canSeeRow(scope, { userId: "ann", accountKey: K1 })).toBe(false);
    expect(canSeeAccount(scope, K1)).toBe(false);
    expect(scope.pooledMembers.size).toBe(0);
  });

  it("sees an account it synced or is pooled on, and no other", () => {
    const pools = WORLDS[3]!.pools;
    const cat = buildAccessScope("cat", [K3], pools);
    expect(canSeeAccount(cat, K3)).toBe(true); // synced
    expect(canSeeAccount(cat, K2)).toBe(true); // pooled, never synced
    expect(canSeeAccount(cat, K1)).toBe(false);
    expect(visibleAccountKeys(cat)).toEqual([K2, K3]);

    const dan = buildAccessScope("dan", [], []);
    expect(visibleAccountKeys(dan)).toEqual([]);
    expect(canSeeAccount(dan, K1)).toBe(false);
  });

  it("reports whether an account is shared with someone else", () => {
    const ann = buildAccessScope("ann", [K1, K2], WORLDS[1]!.pools);
    expect(isPooledWithOthers(ann, K1)).toBe(true);
    expect(isPooledWithOthers(ann, K2)).toBe(false);
    const dan = buildAccessScope("dan", [K3], WORLDS[4]!.pools);
    expect(isPooledWithOthers(dan, K3)).toBe(false); // alone in the pool
    expect(canSeeAccount(dan, K3)).toBe(true);
  });

  it("lists the owners visible on an account, the viewer always included", () => {
    const pools = WORLDS[2]!.pools;
    expect(visibleOwnerIds(buildAccessScope("ann", [], pools), K1)).toEqual([
      "ann",
      "bob",
      "cat",
    ]);
    expect(visibleOwnerIds(buildAccessScope("bob", [], pools), K1)).toEqual([
      "ann",
      "bob",
    ]);
    expect(visibleOwnerIds(buildAccessScope("bob", [], pools), K2)).toEqual([
      "bob",
    ]);
    expect(buildAccessScope("ann", [], pools).poolIds.get(K1)).toEqual([
      "p1",
      "p2",
    ]);
  });

  it("remembers who created the viewer's pools, oldest first, and no one else's", () => {
    const pools: PoolMembership[] = [
      {
        poolId: "p1",
        accountKey: K1,
        memberIds: ["ann", "bob"],
        createdById: "ann",
      },
      {
        poolId: "p2",
        accountKey: K1,
        memberIds: ["cat", "bob"],
        createdById: "cat",
      },
      {
        poolId: "p3",
        accountKey: K2,
        memberIds: ["dan", "cat"],
        createdById: "dan",
      },
    ];
    const bob = buildAccessScope("bob", [], pools);
    expect(bob.poolCreators.get(K1)).toEqual(["ann", "cat"]);
    expect(bob.poolCreators.has(K2)).toBe(false); // not his pool
    expect(buildAccessScope("ann", [], pools).poolCreators.get(K1)).toEqual([
      "ann",
    ]);
  });

  it("builds a deterministic filter", () => {
    const pools = [...WORLDS[3]!.pools].reverse();
    expect(ownedRowWhere(buildAccessScope("bob", [], pools))).toEqual({
      OR: [
        { userId: "bob" },
        { accountKey: K1, userId: { in: ["ann", "bob"] } },
        { accountKey: K2, userId: { in: ["bob", "cat", "dan"] } },
      ],
    });
    expect(
      ownedRowWhere(buildAccessScope("dan", [], []), { accountKey: K1 }),
    ).toEqual({
      accountKey: K1,
      userId: { in: ["dan"] },
    });
  });
});

describe("pool rules", () => {
  const NOW = new Date("2026-09-25T12:00:00Z");
  const pool = (overrides: Partial<PoolFacts> = {}): PoolFacts => ({
    id: "p1",
    accountKey: K1,
    createdById: "ann",
    codeExpiresAt: new Date("2026-10-01T00:00:00Z"),
    revokedAt: null,
    memberIds: ["ann", "bob"],
    ...overrides,
  });
  const revoked = new Date("2026-09-25T00:00:00Z");
  const expired = { codeExpiresAt: new Date("2026-09-25T11:59:59.999Z") };

  it("a code works until it expires or is revoked", () => {
    expect(codeIsActive(pool(), NOW)).toBe(true);
    expect(codeIsActive(pool(expired), NOW)).toBe(false);
    expect(codeIsActive(pool({ codeExpiresAt: NOW }), NOW)).toBe(false);
    expect(codeIsActive(pool({ revokedAt: revoked }), NOW)).toBe(false);
  });

  describe("create", () => {
    it("needs the viewer to have synced the account", () => {
      expect(
        decideCreatePool(
          "ann",
          { viewerSyncedAccount: false, ownPool: null },
          NOW,
        ),
      ).toMatchObject({
        ok: false,
        code: "FORBIDDEN",
      });
      // Being a member of someone else's pool on it is not enough.
      expect(
        decideCreatePool(
          "bob",
          { viewerSyncedAccount: false, ownPool: null },
          NOW,
        ).ok,
      ).toBe(false);
    });

    it("reuses a working code, renews a lapsed one in the same pool, else creates a pool", () => {
      const create = (ownPool: PoolFacts | null, viewer = "ann") =>
        decideCreatePool(viewer, { viewerSyncedAccount: true, ownPool }, NOW);
      expect(create(null)).toEqual({ ok: true, action: "create" });
      expect(create(pool())).toEqual({ ok: true, action: "reuse" });
      expect(create(pool({ revokedAt: revoked }))).toEqual({
        ok: true,
        action: "renew",
      });
      expect(create(pool(expired))).toEqual({ ok: true, action: "renew" });
      // Someone else's pool on the same account doesn't count as the viewer's.
      expect(create(pool(), "bob")).toEqual({ ok: true, action: "create" });
    });
  });

  describe("join", () => {
    it("joins with a working code", () => {
      expect(decideJoin("cat", pool(), NOW)).toEqual({
        ok: true,
        action: "join",
      });
    });
    it("is idempotent for members, even after the code lapsed", () => {
      for (const facts of [
        pool(),
        pool({ revokedAt: revoked }),
        pool(expired),
      ]) {
        expect(decideJoin("bob", facts, NOW)).toEqual({
          ok: true,
          action: "already-member",
        });
        expect(decideJoin("ann", facts, NOW)).toEqual({
          ok: true,
          action: "already-member",
        });
      }
    });
    it("answers unknown, revoked and expired codes identically", () => {
      const unknown = decideJoin("cat", null, NOW);
      expect(unknown).toMatchObject({ ok: false, code: "NOT_FOUND" });
      expect(decideJoin("cat", pool({ revokedAt: revoked }), NOW)).toEqual(
        unknown,
      );
      expect(decideJoin("cat", pool(expired), NOW)).toEqual(unknown);
    });
  });

  describe("revoke", () => {
    it("is the creator's alone", () => {
      expect(decideRevoke("ann", pool())).toEqual({
        ok: true,
        action: "revoke",
      });
      expect(decideRevoke("ann", pool({ revokedAt: revoked }))).toEqual({
        ok: true,
        action: "already-revoked",
      });
      expect(decideRevoke("bob", pool())).toMatchObject({
        ok: false,
        code: "FORBIDDEN",
      });
    });
    it("hides pools the viewer is not in", () => {
      expect(decideRevoke("cat", pool())).toMatchObject({
        ok: false,
        code: "NOT_FOUND",
      });
      expect(decideRevoke("ann", null)).toMatchObject({
        ok: false,
        code: "NOT_FOUND",
      });
    });
  });

  describe("remove member", () => {
    it("lets the creator remove others", () => {
      expect(decideRemoveMember("ann", pool(), "bob")).toEqual({
        ok: true,
        action: "remove",
      });
      expect(decideRemoveMember("ann", pool(), "cat")).toEqual({
        ok: true,
        action: "not-a-member",
      });
      // Works on a pool whose code was revoked: membership outlives the code.
      expect(
        decideRemoveMember("ann", pool({ revokedAt: revoked }), "bob").ok,
      ).toBe(true);
    });
    it("refuses members, outsiders and self-removal", () => {
      expect(decideRemoveMember("bob", pool(), "ann")).toMatchObject({
        code: "FORBIDDEN",
      });
      expect(decideRemoveMember("bob", pool(), "bob")).toMatchObject({
        code: "FORBIDDEN",
      });
      expect(decideRemoveMember("cat", pool(), "bob")).toMatchObject({
        code: "NOT_FOUND",
      });
      expect(decideRemoveMember("ann", pool(), "ann")).toMatchObject({
        code: "BAD_REQUEST",
      });
      expect(decideRemoveMember("ann", null, "bob")).toMatchObject({
        code: "NOT_FOUND",
      });
    });
  });

  describe("leave", () => {
    it("lets members leave and the creator delete", () => {
      expect(decideLeave("bob", pool())).toEqual({ ok: true, action: "leave" });
      expect(decideLeave("ann", pool())).toEqual({
        ok: true,
        action: "delete-pool",
      });
      expect(decideLeave("bob", pool({ revokedAt: revoked }))).toEqual({
        ok: true,
        action: "leave",
      });
    });
    it("hides pools the viewer is not in", () => {
      expect(decideLeave("cat", pool())).toMatchObject({
        ok: false,
        code: "NOT_FOUND",
      });
      expect(decideLeave("cat", null)).toMatchObject({
        ok: false,
        code: "NOT_FOUND",
      });
    });
  });

  it("enforce throws AccessDenied with the refusal's code", () => {
    expect(enforce({ ok: true, action: "join" })).toEqual({
      ok: true,
      action: "join",
    });
    try {
      enforce(decideJoin("cat", null, new Date()));
      expect.unreachable();
    } catch (error) {
      expect(error).toBeInstanceOf(AccessDenied);
      expect((error as AccessDenied).code).toBe("NOT_FOUND");
    }
  });
});
