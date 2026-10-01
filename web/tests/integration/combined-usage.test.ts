/**
 * Usage across accounts on Postgres: the usage page's totals, each account's part, its buckets
 * over time in the viewer's calendar, and usage by project on a selection of accounts. Every
 * figure agrees with the account cards' and adds up only what the viewer may see.
 */
import { beforeEach, describe, expect, it } from "vitest";

import { loadAccessScope } from "~/server/services/access";
import { listAccounts } from "~/server/services/accounts";
import {
  combinedUsage,
  usageTimeline,
  type UsageTimeline,
} from "~/server/services/combined-usage";
import { createPoolCode, joinPool } from "~/server/services/pools";
import { projectUsage } from "~/server/services/project-usage";
import { applySync } from "~/server/services/sync";
import { addUp } from "~/server/services/totals";

import { type SyncFixture } from "../support/fixtures";
import { createUser, db, resetDb } from "./db";
import { AFTER_FIXTURE, K1, K2, request, requestFor } from "./seed";

const scope = (userId: string) => loadAccessScope(db, userId);

const ANNS_MAC = "0e6f0b4c-2f7a-4e53-9d1b-6a2c7f9e1d35";
const BOBS_MAC = "11111111-2222-4333-8444-555555555555";
/** An account nobody synced. */
const K3 = "f".repeat(64);

/** The fixture's sessions, as tokens: S1 on K1, S2 on K2, both on 2026-09-25 (UTC). */
const S1_TOKENS = 18234n + 96512n + 402118n + 12873120n;
const S2_TOKENS = 5120n + 8840n + 64000n + 910000n;

const tokens = (input: number) => ({
  input,
  output: input * 2,
  cacheCreation: input * 3,
  cacheRead: input * 4,
});

type SessionEdit = Partial<SyncFixture["sessions"][number]> & {
  sessionId: string;
};

/** Sessions on one account from Ann's Mac, each built on the fixture's session (and project) there. */
function sessionsOn(accountKey: string, sessions: SessionEdit[]) {
  return request((r) => {
    r.device.id = ANNS_MAC;
    r.accounts = r.accounts.filter((a) => a.key === accountKey);
    const base = r.sessions.find((s) => s.accountKey === accountKey)!;
    r.sessions = sessions.map((edit) => {
      const session = { ...base, ...edit, accountKey };
      delete session.summary;
      return session;
    });
    r.usage = [];
  });
}

/** The buckets added up, to compare with the period's totals. */
const bucketTotal = (timeline: UsageTimeline) => addUp(timeline.buckets);

beforeEach(async () => {
  await resetDb();
  await createUser("ann", "Ann");
  await createUser("bob", "Bob");
  await createUser("dan", "Dan");

  // Ann: the fixture (S1 on K1, S2 on K2), then more on each account.
  await applySync(db, "ann", request(), AFTER_FIXTURE);
  await applySync(
    db,
    "ann",
    sessionsOn(K1, [
      // 20:00 UTC on the 24th is 01:30 on the 25th in India.
      {
        sessionId: "c4c4c4c4-0000-4000-8000-000000000004",
        startedAt: "2026-09-24T20:00:00Z",
        lastActivityAt: "2026-09-24T20:30:00Z",
        endedAt: "2026-09-24T20:31:00Z",
        tokens: tokens(1),
        costUsd: 1.5,
      },
      {
        sessionId: "c5c5c5c5-0000-4000-8000-000000000005",
        startedAt: "2026-07-01T08:00:00Z",
        lastActivityAt: "2026-07-01T08:30:00Z",
        endedAt: "2026-07-01T08:31:00Z",
        tokens: tokens(10),
        costUsd: null,
      },
    ]),
    AFTER_FIXTURE,
  );
  await applySync(
    db,
    "ann",
    sessionsOn(K2, [
      {
        sessionId: "c3c3c3c3-0000-4000-8000-000000000003",
        startedAt: "2026-09-10T08:00:00Z",
        lastActivityAt: "2026-09-10T09:00:00Z",
        endedAt: "2026-09-10T09:01:00Z",
        tokens: tokens(1000),
        costUsd: 0.5,
      },
      // From a Mac whose clock runs six hours ahead: it counts as now.
      {
        sessionId: "c6c6c6c6-0000-4000-8000-000000000006",
        startedAt: "2026-09-26T06:00:00Z",
        lastActivityAt: "2026-09-26T06:30:00Z",
        tokens: tokens(2),
        costUsd: 0.25,
      },
    ]),
    AFTER_FIXTURE,
  );
  // Bob has sessions on K1 and K2 of his own. Ann shares K1 with him, not K2.
  for (const [accountKey, sessionId, input] of [
    [K1, "b0b0b0b0-0000-4000-8000-000000000001", 100],
    [K2, "b0b0b0b0-0000-4000-8000-000000000002", 7],
  ] as const) {
    await applySync(
      db,
      "bob",
      requestFor({
        deviceId: BOBS_MAC,
        accountKey,
        sessionId,
        projectKey: accountKey === K1 ? "b".repeat(64) : "c".repeat(64),
        projectName: "agentnotch",
        tokens: tokens(input),
      }),
      AFTER_FIXTURE,
    );
  }
  const { code } = await createPoolCode(db, "ann", K1);
  await joinPool(db, "bob", code);
});

describe("usage across accounts", () => {
  it("add up every account the viewer sees, with each one's part", async () => {
    const week = await combinedUsage(
      db,
      await scope("ann"),
      { period: "7d" },
      AFTER_FIXTURE,
    );
    expect(week.from).toEqual(new Date("2026-09-19T00:00:00Z"));
    expect(week.to).toEqual(AFTER_FIXTURE);
    expect(week.accountKeys).toEqual([K1, K2].sort());
    // K1: S1, the 24th's session and Bob's (pooled). K2: S2 and the one from the future.
    expect(
      week.accounts.map((a) => [a.accountKey, a.sessions, a.tokens.total]),
    ).toEqual([
      [K1, 3, S1_TOKENS + 10n + 1000n],
      [K2, 2, S2_TOKENS + 20n],
    ]);
    const [k1, k2] = week.accounts;
    expect(k1!.costUsd).toBeCloseTo(14.82 + 1.5 + 14.82, 6);
    // S2 reported no cost; the other did. Unknown parts stay out of the sum, not zero it.
    expect(k2!.costUsd).toBeCloseTo(0.25, 6);
    // The future session dates K2's last use as now, never later.
    expect(k2!.lastUsedAt).toEqual(AFTER_FIXTURE);
    expect(k1!.lastUsedAt).toEqual(new Date("2026-09-25T09:47:03Z"));

    expect(week.total.sessions).toBe(5);
    expect(week.total.tokens.total).toBe(S1_TOKENS + S2_TOKENS + 1030n);
    expect(week.total.costUsd).toBeCloseTo(14.82 * 2 + 1.5 + 0.25, 6);
    expect(week.earlierSessions).toBe(true);
    expect(week.firstStartedAt).toEqual(new Date("2026-07-01T08:00:00Z"));
    expect(week.averageDays).toBe(7);

    const ever = await combinedUsage(
      db,
      await scope("ann"),
      { period: "all" },
      AFTER_FIXTURE,
    );
    expect(ever.from).toBeNull();
    expect(ever.total.sessions).toBe(7);
    expect(ever.earlierSessions).toBe(false);
    // From 1 July 08:00 to 26 September: 86⅔ days, the day begun counted whole.
    expect(ever.averageDays).toBe(87);
  });

  it("agree with the account cards' totals", async () => {
    const annScope = await scope("ann");
    const cards = await listAccounts(db, annScope, AFTER_FIXTURE);
    for (const [period, name] of [
      ["7d", "last7Days"],
      ["30d", "last30Days"],
    ] as const) {
      const report = await combinedUsage(
        db,
        annScope,
        { period },
        AFTER_FIXTURE,
      );
      for (const card of cards) {
        const part = report.accounts.find((a) => a.accountKey === card.key)!;
        expect(part.sessions).toBe(card[name].sessions);
        expect(part.tokens).toEqual(card[name].tokens);
        expect(part.costUsd).toBeCloseTo(card[name].costUsd!, 6);
        expect(part.lastUsedAt).toEqual(card.lastActivityAt);
      }
    }
  });

  it("add up only the chosen accounts the viewer can see", async () => {
    const annScope = await scope("ann");
    const k2 = await combinedUsage(
      db,
      annScope,
      { period: "30d", accountKeys: [K2] },
      AFTER_FIXTURE,
    );
    expect(k2.accountKeys).toEqual([K2]);
    expect(k2.accounts.map((a) => a.accountKey)).toEqual([K2]);
    expect(k2.total.tokens.total).toBe(S2_TOKENS + 10_020n);
    expect(k2.total.costUsd).toBeCloseTo(0.75, 6);

    // An account nobody synced, repeats and order make no difference.
    const named = await combinedUsage(
      db,
      annScope,
      { period: "30d", accountKeys: [K3, K2, K2] },
      AFTER_FIXTURE,
    );
    expect(named).toEqual(k2);

    // Bob synced K2 himself, so he sees it, but only his own session there: Ann's K2 isn't
    // shared with him.
    const bobs = await combinedUsage(
      db,
      await scope("bob"),
      { period: "30d", accountKeys: [K1, K2] },
      AFTER_FIXTURE,
    );
    expect(bobs.accountKeys).toEqual([K1, K2].sort());
    expect(
      bobs.accounts.map((a) => [a.accountKey, a.sessions, a.tokens.total]),
    ).toEqual([
      [K1, 3, S1_TOKENS + 10n + 1000n],
      [K2, 1, 70n],
    ]);

    // Dan sees no account, so none of the ones he names counts.
    const dans = await combinedUsage(
      db,
      await scope("dan"),
      { period: "all", accountKeys: [K1, K2] },
      AFTER_FIXTURE,
    );
    expect(dans).toMatchObject({
      accountKeys: [],
      accounts: [],
      firstStartedAt: null,
      averageDays: 0,
      earlierSessions: false,
    });
    expect(dans.total.sessions).toBe(0);
    expect(dans.total.costUsd).toBeNull();
  });

  it("list accounts without sessions in the period last, at zero", async () => {
    // Two months on, nothing started in the last week.
    const later = new Date("2026-11-26T00:00:00Z");
    const quiet = await combinedUsage(
      db,
      await scope("ann"),
      { period: "7d" },
      later,
    );
    expect(quiet.accounts.map((a) => a.sessions)).toEqual([0, 0]);
    expect(quiet.total.costUsd).toBeNull();
    expect(quiet.averageDays).toBe(0);
    expect(quiet.earlierSessions).toBe(true);
  });
});

describe("usage over time", () => {
  it("count days of the viewer's calendar, adding up to the period", async () => {
    const annScope = await scope("ann");
    const week = await combinedUsage(
      db,
      annScope,
      { period: "7d" },
      AFTER_FIXTURE,
    );

    const utc = await usageTimeline(
      db,
      annScope,
      { period: "7d", timeZone: "UTC" },
      AFTER_FIXTURE,
    );
    expect(utc).toMatchObject({ unit: "day", timeZone: "UTC" });
    // The week starts at midnight UTC here, so seven whole days.
    expect(utc.buckets.map((b) => b.start.toISOString())).toEqual(
      [19, 20, 21, 22, 23, 24, 25].map((d) => `2026-09-${d}T00:00:00.000Z`),
    );
    expect(utc.buckets.at(-1)!.end).toEqual(AFTER_FIXTURE);
    expect(utc.buckets.map((b) => b.sessions)).toEqual([0, 0, 0, 0, 0, 1, 4]);
    expect(bucketTotal(utc).tokens).toEqual(week.total.tokens);
    // The 25th's sessions by account, most tokens first.
    expect(
      utc.buckets
        .at(-1)!
        .accounts.map((a) => [a.accountKey, a.sessions, a.tokens.total]),
    ).toEqual([
      [K1, 2, S1_TOKENS + 1000n],
      [K2, 2, S2_TOKENS + 20n],
    ]);

    // In India the week starts at 05:30 on the 19th, and the 24th's late session falls on the 25th.
    const india = await usageTimeline(
      db,
      annScope,
      { period: "7d", timeZone: "Asia/Kolkata" },
      AFTER_FIXTURE,
    );
    expect(india.timeZone).toBe("Asia/Kolkata");
    expect(india.buckets.map((b) => b.start.toISOString())).toEqual([
      "2026-09-19T00:00:00.000Z",
      ...[19, 20, 21, 22, 23, 24, 25].map((d) => `2026-09-${d}T18:30:00.000Z`),
    ]);
    // The 25th (from 18:30 UTC on the 24th) has S1, Bob's and the 24th's; the future session is
    // now, 05:30 on the 26th there.
    expect(india.buckets.map((b) => b.sessions)).toEqual([
      0, 0, 0, 0, 0, 0, 4, 1,
    ]);
    expect(bucketTotal(india).sessions).toBe(week.total.sessions);
    expect(bucketTotal(india).costUsd).toBeCloseTo(week.total.costUsd!, 6);
  });

  it("count weeks or months over all time, from the first session", async () => {
    const annScope = await scope("ann");
    const ever = await combinedUsage(
      db,
      annScope,
      { period: "all" },
      AFTER_FIXTURE,
    );
    const timeline = await usageTimeline(
      db,
      annScope,
      { period: "all", timeZone: "UTC" },
      AFTER_FIXTURE,
    );
    // 1 July to 26 September is more than 62 days: weeks, from the Monday of the first one.
    expect(timeline.unit).toBe("week");
    expect(timeline.from).toBeNull();
    expect(timeline.buckets[0]!.start).toEqual(
      new Date("2026-06-29T00:00:00Z"),
    );
    expect(timeline.buckets).toHaveLength(13);
    expect(timeline.buckets[0]!.sessions).toBe(1);
    expect(bucketTotal(timeline).tokens).toEqual(ever.total.tokens);
    expect(bucketTotal(timeline).sessions).toBe(ever.total.sessions);

    // On K2 alone the first session is 10 September: days.
    const k2 = await usageTimeline(
      db,
      annScope,
      { period: "all", accountKeys: [K2], timeZone: "UTC" },
      AFTER_FIXTURE,
    );
    expect(k2.unit).toBe("day");
    expect(k2.buckets[0]!.start).toEqual(new Date("2026-09-10T00:00:00Z"));
    expect(k2.buckets).toHaveLength(16);
    expect(bucketTotal(k2).sessions).toBe(3);
  });

  it("fall back to UTC for a zone it doesn't know, and have nothing for no account", async () => {
    const annScope = await scope("ann");
    const unknown = await usageTimeline(
      db,
      annScope,
      { period: "7d", timeZone: "Mars/Olympus_Mons" },
      AFTER_FIXTURE,
    );
    expect(unknown.timeZone).toBe("UTC");
    expect(unknown.buckets).toHaveLength(7);

    const dans = await usageTimeline(
      db,
      await scope("dan"),
      { period: "7d", timeZone: "UTC" },
      AFTER_FIXTURE,
    );
    expect(dans).toMatchObject({ accountKeys: [], buckets: [] });
    const nothingYet = await usageTimeline(
      db,
      annScope,
      { period: "all", accountKeys: [K3], timeZone: "UTC" },
      AFTER_FIXTURE,
    );
    expect(nothingYet.buckets).toEqual([]);
  });
});

describe("usage by project on chosen accounts", () => {
  it("list only those accounts' projects, and only their usage there", async () => {
    const annScope = await scope("ann");
    const k2 = await projectUsage(
      db,
      annScope,
      { period: "30d", accountKeys: [K2, K3] },
      AFTER_FIXTURE,
    );
    expect(
      k2.projects.map((p) => [
        p.name,
        p.owner.id,
        p.accounts.map((a) => a.accountKey),
      ]),
    ).toEqual([["billing-service", "ann", [K2]]]);
    expect(k2.total.sessions).toBe(3);

    // Both accounts are every account Ann sees: the same report as none named.
    expect(
      await projectUsage(
        db,
        annScope,
        { period: "30d", accountKeys: [K1, K2] },
        AFTER_FIXTURE,
      ),
    ).toEqual(
      await projectUsage(db, annScope, { period: "30d" }, AFTER_FIXTURE),
    );

    const dans = await projectUsage(
      db,
      await scope("dan"),
      { period: "all", accountKeys: [K1] },
      AFTER_FIXTURE,
    );
    expect(dans).toMatchObject({ projects: [], earlierSessions: false });
  });
});
