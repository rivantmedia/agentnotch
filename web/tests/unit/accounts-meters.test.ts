/**
 * Which usage meters an account card shows (src/server/services/accounts.ts), and that the
 * queries behind them stay bounded however many window ids pool members made up. The database is
 * a fake that records what it was asked; tests/integration/pools.test.ts runs it on Postgres.
 */
import { describe, expect, it } from "vitest";

import { compareWindowIds, FIXED_WINDOW_IDS } from "~/lib/format";
import { buildAccessScope } from "~/server/services/access";
import {
  listAccounts,
  MAX_WINDOWS_PER_ACCOUNT,
  rankMeterWindows,
} from "~/server/services/accounts";
import { SYNC_QUOTAS } from "~/server/services/sync";
import { type Db } from "~/server/db-types";

const K1 = "1".repeat(64);
const NOW = new Date("2026-09-25T12:02:00Z");

describe("compareWindowIds", () => {
  it("puts the fixed windows first, in their order, then the rest by id", () => {
    const ids = [
      "weekly_sonnet",
      "extra_usage",
      "weekly_all",
      "weekly_opus",
      "session",
      "weekly_aardvark",
    ];
    expect([...ids].sort(compareWindowIds)).toEqual([
      "session",
      "weekly_all",
      "extra_usage",
      "weekly_aardvark",
      "weekly_opus",
      "weekly_sonnet",
    ]);
    expect(FIXED_WINDOW_IDS).toEqual(["session", "weekly_all", "extra_usage"]);
  });
});

describe("rankMeterWindows", () => {
  it("ranks fixed windows, then trusted per-model ones, then the rest, each by id", () => {
    expect(
      rankMeterWindows([
        { windowId: "weekly_zeta", trusted: false },
        { windowId: "weekly_beta", trusted: false },
        { windowId: "weekly_sonnet", trusted: true },
        { windowId: "extra_usage", trusted: false },
        { windowId: "weekly_opus", trusted: true },
        { windowId: "weekly_all", trusted: false },
        { windowId: "session", trusted: true },
      ]),
    ).toEqual([
      "session",
      "weekly_all",
      "extra_usage",
      "weekly_opus",
      "weekly_sonnet",
      "weekly_beta",
      "weekly_zeta",
    ]);
  });

  it("keeps at most MAX_WINDOWS_PER_ACCOUNT, cutting untrusted windows first", () => {
    const madeUp = Array.from({ length: 40 }, (_, i) => ({
      windowId: `weekly_aaa_${String(i).padStart(2, "0")}`,
      trusted: false,
    }));
    const ranked = rankMeterWindows([
      ...madeUp,
      { windowId: "weekly_opus", trusted: true },
      { windowId: "weekly_all", trusted: false },
    ]);
    expect(MAX_WINDOWS_PER_ACCOUNT).toBe(20);
    expect(ranked).toHaveLength(20);
    expect(ranked.slice(0, 3)).toEqual([
      "weekly_all",
      "weekly_opus",
      "weekly_aaa_00",
    ]);
    expect(ranked.at(-1)).toBe("weekly_aaa_17");
  });
});

type Reading = {
  id: number;
  userId: string;
  accountKey: string;
  source: string;
  windowId: string;
  utilization: number;
  resetsAt: Date | null;
  observedAt: Date;
};

/** Enough of Prisma for listAccounts, answering from `readings` and recording each call. */
function fakeDb(readings: Reading[]) {
  const calls: { groupBy: unknown[]; findMany: unknown[] } = {
    groupBy: [],
    findMany: [],
  };
  const db = {
    userAccount: { findMany: async () => [] },
    session: { groupBy: async () => [] },
    usageReading: {
      groupBy: async (args: { take?: number }) => {
        calls.groupBy.push(args);
        const groups = new Map<
          string,
          {
            accountKey: string;
            userId: string;
            windowId: string;
            _max: { observedAt: Date };
          }
        >();
        for (const r of readings) {
          const key = `${r.accountKey}|${r.userId}|${r.windowId}`;
          const group = groups.get(key);
          if (!group || group._max.observedAt < r.observedAt) {
            groups.set(key, {
              accountKey: r.accountKey,
              userId: r.userId,
              windowId: r.windowId,
              _max: { observedAt: r.observedAt },
            });
          }
        }
        return [...groups.values()]
          .sort((a, b) => (a.windowId < b.windowId ? -1 : 1))
          .slice(0, args.take);
      },
      findMany: async (args: {
        where: { AND: [unknown, { OR: Array<Partial<Reading>> }] };
        take?: number;
      }) => {
        calls.findMany.push(args);
        const wanted = args.where.AND[1].OR;
        return readings
          .filter((r) =>
            wanted.some(
              (w) =>
                w.accountKey === r.accountKey &&
                w.windowId === r.windowId &&
                (w.userId === undefined || w.userId === r.userId) &&
                w.observedAt?.getTime() === r.observedAt.getTime(),
            ),
          )
          .sort(
            (a, b) =>
              b.observedAt.getTime() - a.observedAt.getTime() || b.id - a.id,
          )
          .slice(0, args.take);
      },
    },
  };
  return { db: db as unknown as Db, calls };
}

function reading(
  id: number,
  userId: string,
  windowId: string,
  observedAt: string,
  utilization = 10,
): Reading {
  return {
    id,
    userId,
    accountKey: K1,
    source: "probe",
    windowId,
    utilization,
    resetsAt: null,
    observedAt: new Date(observedAt),
  };
}

describe("listAccounts' meters", () => {
  // Ann made the pool on K1; Bob and Dan joined it. Dan looks at the account.
  const scope = buildAccessScope(
    "dan",
    [],
    [
      {
        poolId: "p1",
        accountKey: K1,
        memberIds: ["ann", "bob", "dan"],
        createdById: "ann",
      },
    ],
  );

  // Ann's real windows, and Bob's made-up ones, dated after hers (up to the 5 minutes ahead that
  // "latest" still takes).
  const readings = [
    reading(1, "ann", "session", "2026-09-25T11:50:00Z"),
    reading(2, "ann", "weekly_all", "2026-09-25T11:50:00Z"),
    reading(3, "ann", "weekly_opus", "2026-09-25T11:50:00Z", 44),
    reading(4, "bob", "session", "2026-09-25T12:00:00Z", 70),
    reading(5, "bob", "weekly_sonnet", "2026-09-25T12:00:00Z"),
    ...Array.from({ length: 29 }, (_, i) =>
      reading(
        100 + i,
        "bob",
        `weekly_aa_fake_${String(i).padStart(2, "0")}`,
        "2026-09-25T12:06:00Z",
        100,
      ),
    ),
  ];

  it("keeps the pool creator's per-model window on the card ahead of fresher made-up ones", async () => {
    const { db } = fakeDb(readings);
    const [k1] = await listAccounts(db, scope, NOW);
    const ids = k1!.usage.map((u) => u.windowId);
    expect(ids).toHaveLength(MAX_WINDOWS_PER_ACCOUNT);
    expect(ids.slice(0, 3)).toEqual(["session", "weekly_all", "weekly_opus"]);
    // Then the other members' windows, by id, as many as fit.
    expect(ids.slice(3, 5)).toEqual(["weekly_aa_fake_00", "weekly_aa_fake_01"]);
    expect(ids).not.toContain("weekly_sonnet");
    // Each meter is still the newest reading of its window, whoever took it.
    expect(k1!.usage[0]).toMatchObject({
      windowId: "session",
      utilization: 70,
    });
    expect(k1!.usage[2]).toMatchObject({
      windowId: "weekly_opus",
      utilization: 44,
    });
  });

  it("ranks the viewer's own per-model windows as trusted too", async () => {
    // Bob looks this time; Cat is another member.
    const bobs = buildAccessScope(
      "bob",
      [K1],
      [
        {
          poolId: "p1",
          accountKey: K1,
          memberIds: ["ann", "bob", "cat"],
          createdById: "ann",
        },
      ],
    );
    const { db } = fakeDb([
      reading(1, "ann", "session", "2026-09-25T11:50:00Z"),
      reading(2, "ann", "weekly_opus", "2026-09-25T11:50:00Z"),
      reading(3, "bob", "weekly_sonnet", "2026-09-25T11:40:00Z"),
      reading(4, "cat", "weekly_aa_cats", "2026-09-25T12:01:00Z"),
    ]);
    const [k1] = await listAccounts(db, bobs, NOW);
    expect(k1!.usage.map((u) => u.windowId)).toEqual([
      "session",
      "weekly_opus", // the creator's
      "weekly_sonnet", // his own
      "weekly_aa_cats", // another member's: last, although first by id and newest
    ]);
  });

  it("bounds both queries by the per-account window cap and the meter cap", async () => {
    const { db, calls } = fakeDb(readings);
    await listAccounts(db, scope, NOW);

    expect(calls.groupBy).toHaveLength(1);
    // Three people's rows are visible on K1, each with at most `windowsPerAccount` window ids.
    expect(calls.groupBy[0]).toMatchObject({
      by: ["accountKey", "userId", "windowId"],
      take: 3 * SYNC_QUOTAS.windowsPerAccount,
    });

    expect(calls.findMany).toHaveLength(1);
    const lookup = calls.findMany[0] as {
      where: { AND: [unknown, { OR: Array<{ userId?: string }> }] };
      take: number;
    };
    // One wanted reading per meter, each pinned to the person who took it: one row per source.
    expect(lookup.where.AND[1].OR).toHaveLength(MAX_WINDOWS_PER_ACCOUNT);
    for (const wanted of lookup.where.AND[1].OR) {
      expect(typeof wanted.userId).toBe("string");
    }
    expect(lookup.take).toBe(MAX_WINDOWS_PER_ACCOUNT * 4);
  });
});
