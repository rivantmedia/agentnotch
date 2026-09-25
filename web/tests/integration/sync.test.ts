/**
 * POST /sync's service against Postgres: what it stores, that repeating a batch changes nothing,
 * how summaries are kept or replaced, and how usage readings are deduplicated.
 */
import { beforeEach, describe, expect, it } from "vitest";

import { AppApiError } from "~/server/app-api/errors";
import { applySync, SYNC_QUOTAS, TOKEN_CAP } from "~/server/services/sync";

import { keysFixture } from "../support/fixtures";
import { createUser, db, resetDb } from "./db";
import { AFTER_FIXTURE, K1, K2, request, requestFor, S1, S2 } from "./seed";

const NOW = new Date("2026-09-25T11:21:00Z");
const DEVICE = "0e6f0b4c-2f7a-4e53-9d1b-6a2c7f9e1d35";
const DAY_MS = 24 * 60 * 60 * 1000;

async function counts() {
  const [users, devices, accounts, userAccounts, projects, sessions, usage] =
    await Promise.all([
      db.user.count(),
      db.device.count(),
      db.claudeAccount.count(),
      db.userAccount.count(),
      db.project.count(),
      db.session.count(),
      db.usageReading.count(),
    ]);
  return { users, devices, accounts, userAccounts, projects, sessions, usage };
}

/** S1 is on the personal account (K1), S2 on the company one (K2). */
async function session(
  sessionId: string,
  userId = "ann",
  accountKey = sessionId === S2 ? K2 : K1,
) {
  return db.session.findUniqueOrThrow({
    where: {
      userId_accountKey_sessionId: { userId, accountKey, sessionId },
    },
    include: { project: true },
  });
}

async function device(userId: string, id = DEVICE) {
  return db.device.findUniqueOrThrow({
    where: { userId_id: { userId, id } },
  });
}

beforeEach(async () => {
  await resetDb();
  await createUser("ann", "Ann");
});

describe("applySync", () => {
  it("stores the contract fixture", async () => {
    expect(await applySync(db, "ann", request(), NOW)).toEqual({
      sessions: 2,
      usage: 2,
    });
    expect(await counts()).toEqual({
      users: 1,
      devices: 1,
      accounts: 2,
      userAccounts: 2,
      projects: 2,
      sessions: 2,
      usage: 4, // 3 windows + 1 window
    });

    expect(await device("ann")).toEqual({
      id: DEVICE, // stored lowercased
      userId: "ann",
      name: "Studio MacBook Pro",
      appVersion: "1.18.0",
      lastSeenAt: NOW,
    });

    const personal = await db.userAccount.findUniqueOrThrow({
      where: { userId_accountKey: { userId: "ann", accountKey: K1 } },
    });
    expect(personal).toMatchObject({
      email: "me@example.com",
      organizationName: null,
      plan: "Max 20x",
      label: "Personal",
      firstSeenAt: NOW,
      lastSeenAt: NOW,
    });

    const s1 = await session(S1);
    expect(s1).toMatchObject({
      accountKey: K1,
      title: "Keep sessions working while background agents run",
      source: "vscode",
      models: ["claude-opus-4-5-20251101", "claude-haiku-4-5-20251001"],
      startedAt: new Date("2026-09-25T08:02:11.482Z"),
      lastActivityAt: new Date("2026-09-25T09:47:03Z"),
      endedAt: new Date("2026-09-25T09:48:00Z"),
      messageCount: 212,
      inputTokens: 18234n,
      outputTokens: 96512n,
      cacheCreationTokens: 402118n,
      cacheReadTokens: 12873120n,
      summaryModel: "claude-haiku-4-5-20251001",
      summaryAt: new Date("2026-09-25T10:05:00Z"),
      deviceId: DEVICE,
    });
    expect(s1.costUsd?.toString()).toBe("14.82");
    expect(s1.summaryText).toContain("stays 'working'");
    expect(s1.project).toMatchObject({
      userId: "ann",
      accountKey: K1,
      key: keysFixture().projects[0]!.key,
      name: "agentnotch",
    });

    const s2 = await session(S2);
    expect(s2).toMatchObject({
      accountKey: K2,
      title: null,
      endedAt: null,
      costUsd: null,
      summaryText: null,
      summaryModel: null,
      summaryAt: null,
    });

    const readings = await db.usageReading.findMany({
      where: { accountKey: K1 },
      orderBy: { windowId: "asc" },
    });
    expect(
      readings.map((r) => [
        r.source,
        r.windowId,
        r.utilization,
        r.resetsAt?.toISOString() ?? null,
      ]),
    ).toEqual([
      ["desktop", "session", 42, "2026-09-25T13:00:00.000Z"],
      ["desktop", "weekly_all", 61.5, "2026-09-29T08:00:00.000Z"],
      ["desktop", "weekly_opus", 12, null],
    ]);
  });

  it("is idempotent: the same batch again changes nothing", async () => {
    await applySync(db, "ann", request(), NOW);
    const before = await counts();
    const ids = (
      await db.session.findMany({ orderBy: { sessionId: "asc" } })
    ).map((s) => s.id);

    await applySync(db, "ann", request(), new Date(NOW.getTime() + 60_000));
    await applySync(db, "ann", request(), new Date(NOW.getTime() + 120_000));
    expect(await counts()).toEqual(before);
    expect(
      (await db.session.findMany({ orderBy: { sessionId: "asc" } })).map(
        (s) => s.id,
      ),
    ).toEqual(ids);
    // Only the "last seen" times move.
    const personal = await db.userAccount.findUniqueOrThrow({
      where: { userId_accountKey: { userId: "ann", accountKey: K1 } },
    });
    expect(personal.firstSeenAt).toEqual(NOW);
    expect(personal.lastSeenAt).toEqual(new Date(NOW.getTime() + 120_000));
  });

  it("replaces a session's totals and fields with the newest absolute values", async () => {
    await applySync(db, "ann", request(), NOW);
    await applySync(
      db,
      "ann",
      request((r) => {
        const s = r.sessions[1]!;
        s.title = "Billing retries";
        s.endedAt = "2026-09-25T12:00:00Z";
        s.lastActivityAt = "2026-09-25T11:59:00Z";
        s.messageCount = 40;
        s.tokens = {
          input: 6000,
          output: 9000,
          cacheCreation: 70000,
          cacheRead: 1_000_000,
        };
        s.costUsd = 3.5;
        s.models = ["claude-sonnet-4-5-20250929", "claude-haiku-4-5-20251001"];
        s.project.name = "billing"; // the folder was renamed
      }),
      NOW,
    );
    const s2 = await session(S2);
    expect(s2).toMatchObject({
      title: "Billing retries",
      endedAt: new Date("2026-09-25T12:00:00Z"),
      messageCount: 40,
      inputTokens: 6000n,
      outputTokens: 9000n,
      cacheCreationTokens: 70000n,
      cacheReadTokens: 1_000_000n,
      models: ["claude-sonnet-4-5-20250929", "claude-haiku-4-5-20251001"],
    });
    expect(s2.costUsd?.toString()).toBe("3.5");
    expect(s2.project.name).toBe("billing");
    expect(await db.project.count()).toBe(2);
    expect(await db.session.count()).toBe(2);
  });

  it("keeps a stored summary when the field is absent or null, and replaces it when present", async () => {
    await applySync(db, "ann", request(), NOW);
    const original = await session(S1);

    await applySync(
      db,
      "ann",
      request((r) => delete r.sessions[0]!.summary),
      NOW,
    );
    expect(await session(S1)).toMatchObject({
      summaryText: original.summaryText,
      summaryModel: original.summaryModel,
      summaryAt: original.summaryAt,
    });

    await applySync(
      db,
      "ann",
      request((r) => (r.sessions[0]!.summary = null)),
      NOW,
    );
    expect((await session(S1)).summaryText).toBe(original.summaryText);

    await applySync(
      db,
      "ann",
      request(
        (r) =>
          (r.sessions[0]!.summary = {
            text: "Rewrote the summary.",
            model: "claude-sonnet-4-5-20250929",
            generatedAt: "2026-09-25T12:00:00Z",
          }),
      ),
      NOW,
    );
    expect(await session(S1)).toMatchObject({
      summaryText: "Rewrote the summary.",
      summaryModel: "claude-sonnet-4-5-20250929",
      summaryAt: new Date("2026-09-25T12:00:00Z"),
    });

    // A session that never had one gains it the same way (S1's is left out, so it stays).
    await applySync(
      db,
      "ann",
      request((r) => {
        delete r.sessions[0]!.summary;
        r.sessions[1]!.summary = {
          text: "First summary.",
          model: "m",
          generatedAt: "2026-09-25T12:30:00Z",
        };
      }),
      NOW,
    );
    expect((await session(S2)).summaryText).toBe("First summary.");
    expect((await session(S1)).summaryText).toBe("Rewrote the summary.");
  });

  it("deduplicates usage readings on (user, account, source, window, observedAt)", async () => {
    await applySync(db, "ann", request(), NOW);
    expect(await db.usageReading.count()).toBe(4);

    // Same readings, different values: the first one stored wins; nothing is added.
    await applySync(
      db,
      "ann",
      request((r) => (r.usage[0]!.windows[0]!.utilization = 99)),
      NOW,
    );
    expect(await db.usageReading.count()).toBe(4);
    const first = await db.usageReading.findFirstOrThrow({
      where: { accountKey: K1, windowId: "session" },
    });
    expect(first.utilization).toBe(42);

    // A later observation adds rows.
    await applySync(
      db,
      "ann",
      request((r) => (r.usage[0]!.observedAt = "2026-09-25T10:00:00Z")),
      NOW,
    );
    expect(await db.usageReading.count()).toBe(7);

    // So does the same moment from another source.
    await applySync(
      db,
      "ann",
      request((r) => (r.usage[1]!.source = "statusLine")),
      NOW,
    );
    expect(await db.usageReading.count()).toBe(8);

    // Duplicates inside one batch collapse too.
    await applySync(
      db,
      "ann",
      request((r) => {
        const reading = { ...r.usage[0]!, observedAt: "2026-09-25T11:00:00Z" };
        r.usage = [
          reading,
          reading,
          { ...reading, windows: [...reading.windows, reading.windows[0]!] },
        ];
      }),
      NOW,
    );
    expect(await db.usageReading.count()).toBe(11);

    // Fractional seconds are kept: a millisecond apart is another reading.
    await applySync(
      db,
      "ann",
      request(
        (r) =>
          (r.usage = [
            { ...r.usage[1]!, observedAt: "2026-09-25T11:15:00.001Z" },
          ]),
      ),
      NOW,
    );
    expect(await db.usageReading.count()).toBe(12);
  });

  it("keeps each person's rows apart on a shared account", async () => {
    await createUser("bob", "Bob");
    await applySync(db, "ann", request(), NOW);
    // Bob's app reports the same account and even the same session id.
    await applySync(
      db,
      "bob",
      request((r) => (r.device.id = "11111111-2222-4333-8444-555555555555")),
      NOW,
    );
    expect(await counts()).toMatchObject({
      accounts: 2, // one row per Claude account, shared
      userAccounts: 4,
      projects: 4,
      sessions: 4,
      usage: 8,
    });
    expect((await session(S1, "bob")).userId).toBe("bob");
  });

  it("treats session ids case-insensitively", async () => {
    await applySync(db, "ann", request(), NOW);
    await applySync(
      db,
      "ann",
      request((r) => (r.sessions[0]!.sessionId = S1.toUpperCase())),
      NOW,
    );
    expect(await db.session.count()).toBe(2);
    expect((await session(S1)).sessionId).toBe(S1);
  });

  it("keeps each person's Mac their own, even under the same device id", async () => {
    await createUser("bob", "Bob");
    await applySync(db, "ann", request(), NOW);
    // Bob's sync names Ann's device id (a shared Mac, or a copied id): it can't touch her row.
    await applySync(
      db,
      "bob",
      request((r) => (r.device = { ...r.device, name: "Bob's name for it" })),
      new Date(NOW.getTime() + 60_000),
    );
    expect(await device("ann")).toMatchObject({
      name: "Studio MacBook Pro",
      lastSeenAt: NOW,
    });
    expect(await device("bob")).toMatchObject({ name: "Bob's name for it" });
    expect(await db.device.count()).toBe(2);
    // Each one's sessions point at their own row.
    const annS1 = await db.session.findUniqueOrThrow({
      where: {
        userId_accountKey_sessionId: {
          userId: "ann",
          accountKey: K1,
          sessionId: S1,
        },
      },
      include: { device: true },
    });
    expect(annS1.device).toMatchObject({
      userId: "ann",
      name: "Studio MacBook Pro",
    });
  });

  it("stores a session resumed under another account once per account", async () => {
    // Claude Parallel Profiles shares history between folders: the app sends the same session
    // id under each account, each with that account's share.
    await applySync(
      db,
      "ann",
      request((r) => {
        r.sessions[1] = {
          ...r.sessions[1]!,
          sessionId: S1,
          messageCount: 5,
        };
      }),
      NOW,
    );
    const onPersonal = await session(S1, "ann", K1);
    const onCompany = await session(S1, "ann", K2);
    expect(onPersonal.id).not.toBe(onCompany.id);
    expect(onPersonal.messageCount).toBe(212);
    expect(onCompany.messageCount).toBe(5);
    expect(onCompany.project.name).toBe("billing-service");
    expect(await db.session.count()).toBe(2);
  });

  it("takes the last copy of a session repeated in one batch", async () => {
    await applySync(
      db,
      "ann",
      request((r) => {
        r.sessions.push({ ...r.sessions[0]!, messageCount: 999 });
      }),
      NOW,
    );
    expect(await db.session.count()).toBe(2);
    expect((await session(S1)).messageCount).toBe(999);
  });

  it("refuses sessions and readings of accounts missing from accounts[], writing nothing", async () => {
    const bad = request();
    bad.accounts = bad.accounts.slice(0, 1);
    await expect(applySync(db, "ann", bad, NOW)).rejects.toMatchObject({
      code: "BAD_REQUEST",
    });
    await expect(applySync(db, "ann", bad, NOW)).rejects.toBeInstanceOf(
      AppApiError,
    );
    expect(await counts()).toMatchObject({
      devices: 0,
      accounts: 0,
      sessions: 0,
      usage: 0,
    });
  });

  it("writes all of a batch or none of it", async () => {
    const bad = request();
    // Past what Postgres' integer holds; the schema would refuse it, the database does too.
    bad.sessions[1]!.messageCount = 2 ** 31;
    await expect(applySync(db, "ann", bad, NOW)).rejects.toThrow();
    expect(await counts()).toMatchObject({
      devices: 0,
      accounts: 0,
      userAccounts: 0,
      projects: 0,
      sessions: 0,
      usage: 0,
    });
  });

  it("accepts an empty batch (it still records the Mac)", async () => {
    const empty = request((r) => {
      r.accounts = [];
      r.sessions = [];
      r.usage = [];
    });
    expect(await applySync(db, "ann", empty, NOW)).toEqual({
      sessions: 0,
      usage: 0,
    });
    expect(await counts()).toMatchObject({
      devices: 1,
      accounts: 0,
      sessions: 0,
    });
  });

  it("handles a full-size batch: 200 sessions, 500 readings of 20 windows", async () => {
    const big = request((r) => {
      const base = r.sessions[0]!;
      r.sessions = Array.from({ length: 200 }, (_, i) => ({
        ...base,
        sessionId: `a1b2c3d4-e5f6-4789-8abc-${i.toString(16).padStart(12, "0")}`,
        project: {
          key: (i % 20).toString(16).padStart(64, "0"),
          name: `project-${i % 20}`,
        },
      }));
      const reading = r.usage[0]!;
      r.usage = Array.from({ length: 500 }, (_, i) => ({
        ...reading,
        observedAt: new Date(Date.UTC(2026, 8, 1) + i * 60_000).toISOString(),
        windows: Array.from({ length: 20 }, (_, w) => ({
          id: w === 0 ? "session" : `weekly_model_${w}`,
          utilization: w,
          resetsAt: null,
        })),
      }));
    });
    const started = Date.now();
    expect(await applySync(db, "ann", big, NOW)).toEqual({
      sessions: 200,
      usage: 500,
    });
    expect(Date.now() - started).toBeLessThan(30_000);
    expect(await counts()).toMatchObject({
      projects: 20,
      sessions: 200,
      usage: 10_000,
    });

    // And again, idempotently.
    await applySync(db, "ann", big, NOW);
    expect(await counts()).toMatchObject({
      projects: 20,
      sessions: 200,
      usage: 10_000,
    });
  });

  it("caps accounts per person, writing nothing past the cap", async () => {
    const quotas = { accounts: 2 };
    await applySync(db, "ann", request(), NOW, quotas);
    // Re-sending the same two accounts is fine.
    await applySync(db, "ann", request(), NOW, quotas);
    const third = "e".repeat(64);
    const error: unknown = await applySync(
      db,
      "ann",
      request((r) => {
        r.accounts.push({ ...r.accounts[0]!, key: third });
        r.sessions = [];
        r.usage = [];
      }),
      NOW,
      quotas,
    ).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(AppApiError);
    expect(error).toMatchObject({ code: "FORBIDDEN" });
    expect(await db.claudeAccount.count({ where: { key: third } })).toBe(0);
    expect(await db.userAccount.count()).toBe(2);
  });

  it("caps new sessions a day per person by dropping the rest, and still updates known ones", async () => {
    const quotas = { newSessionsPerDay: 3 };
    await applySync(db, "ann", request(), NOW, quotas); // 2 new
    const freshId = (i: number) =>
      `c0ffee00-0000-4000-8000-${i.toString(16).padStart(12, "0")}`;
    const fresh = (n: number, from = 0) =>
      request((r) => {
        r.sessions = Array.from({ length: n }, (_, i) => ({
          ...r.sessions[0]!,
          sessionId: freshId(from + i),
          // The later the id, the more recent the session.
          lastActivityAt: new Date(
            Date.UTC(2026, 8, 25, 10, from + i),
          ).toISOString(),
        }));
        r.usage = [];
      });

    // Room for one more: the newest of the two is stored, the other dropped. Not an error.
    expect(await applySync(db, "ann", fresh(2), NOW, quotas)).toEqual({
      sessions: 1,
      usage: 0,
    });
    expect(await db.session.count()).toBe(3);
    expect(await db.session.count({ where: { sessionId: freshId(1) } })).toBe(
      1,
    );

    // Full: new ones are dropped, updates to stored sessions still go through.
    expect(
      await applySync(
        db,
        "ann",
        request((r) => {
          r.sessions[0]!.messageCount = 300;
          r.sessions.push(...fresh(2, 10).sessions);
        }),
        NOW,
        quotas,
      ),
    ).toEqual({ sessions: 2, usage: 2 });
    expect((await session(S1)).messageCount).toBe(300);
    expect(await db.session.count()).toBe(3);

    // A day later there is room again.
    expect(
      await applySync(
        db,
        "ann",
        fresh(2, 20),
        new Date(NOW.getTime() + DAY_MS + 1),
        quotas,
      ),
    ).toEqual({ sessions: 2, usage: 0 });
    expect(await db.session.count()).toBe(5);
  });

  it("stores a batch's updates and usage readings even when its new sessions are over the quota", async () => {
    // What used to refuse the whole batch for up to a day: a Mac catching up, with its running
    // sessions and its readings in the same batch as sessions the quota has no room for.
    const quotas = { newSessionsPerDay: 2 };
    await applySync(db, "ann", request(), NOW, quotas); // S1 and S2: the day is full
    const later = new Date(NOW.getTime() + 60_000);
    const result = await applySync(
      db,
      "ann",
      request((r) => {
        r.sessions[1]!.messageCount = 99; // S2 is still running
        r.sessions.push({
          ...r.sessions[0]!,
          sessionId: "c0ffee00-0000-4000-8000-000000000001",
          project: { key: "d".repeat(64), name: "new-folder" },
        });
        r.usage = r.usage.map((u) => ({
          ...u,
          observedAt: "2026-09-25T11:20:00Z",
        }));
      }),
      later,
      quotas,
    );
    expect(result).toEqual({ sessions: 2, usage: 2 });
    expect((await session(S2)).messageCount).toBe(99);
    expect(await db.session.count()).toBe(2);
    expect(await db.usageReading.count()).toBe(8);
    // The dropped session's project isn't stored either.
    expect(await db.project.count({ where: { key: "d".repeat(64) } })).toBe(0);
    expect(await device("ann")).toMatchObject({ lastSeenAt: later });
  });

  it("caps new usage readings a day per person, dropping the rest", async () => {
    const quotas = { newUsageReadingsPerDay: 6 };
    // The fixture: 4 readings (3 windows + 1).
    expect(await applySync(db, "ann", request(), NOW, quotas)).toEqual({
      sessions: 2,
      usage: 2,
    });
    const at = (observedAt: string) =>
      request((r) => {
        r.sessions = [];
        r.usage = [{ ...r.usage[0]!, observedAt }]; // K1's 3 windows
      });

    // Room for 2 of these 3 (all of one moment): the first two are kept.
    expect(
      await applySync(db, "ann", at("2026-09-25T10:00:00Z"), NOW, quotas),
    ).toEqual({ sessions: 0, usage: 1 });
    expect(await db.usageReading.count()).toBe(6);

    // Full: new readings are dropped (not an error), and accepted says nothing was stored…
    expect(
      await applySync(db, "ann", at("2026-09-25T10:30:00Z"), NOW, quotas),
    ).toEqual({ sessions: 0, usage: 0 });
    expect(await db.usageReading.count()).toBe(6);
    // …while readings stored before still count as accepted, and cost nothing.
    expect(await applySync(db, "ann", request(), NOW, quotas)).toEqual({
      sessions: 2,
      usage: 2,
    });

    // Another person has their own quota.
    await createUser("bob", "Bob");
    await applySync(db, "bob", request(), NOW, quotas);
    expect(await db.usageReading.count({ where: { userId: "bob" } })).toBe(4);

    // 24 hours after they were stored there is room again, whatever the readings' own dates.
    expect(
      await applySync(
        db,
        "ann",
        at("2026-09-25T10:30:00Z"),
        new Date(NOW.getTime() + DAY_MS + 1),
        quotas,
      ),
    ).toEqual({ sessions: 0, usage: 1 });
    expect(await db.usageReading.count({ where: { userId: "ann" } })).toBe(9);
  });

  it("caps distinct usage window ids per person and account, even after their readings are pruned", async () => {
    const quotas = { windowsPerAccount: 5 };
    const windows = (ids: string[], observedAt: string) =>
      request((r) => {
        r.accounts = [r.accounts[0]!];
        r.sessions = [];
        r.usage = [
          {
            ...r.usage[0]!,
            observedAt,
            windows: ids.map((id) => ({ id, utilization: 1, resetsAt: null })),
          },
        ];
      });
    // session, weekly_all and weekly_opus, then 4 made-up ones: only 2 of those fit.
    await applySync(db, "ann", request(), NOW, quotas);
    const madeUp = ["weekly_a", "weekly_b", "weekly_c", "weekly_d"];
    expect(
      await applySync(
        db,
        "ann",
        windows(madeUp, "2026-09-25T11:00:00Z"),
        NOW,
        quotas,
      ),
    ).toEqual({ sessions: 0, usage: 1 });
    const stored = async () =>
      [
        ...new Set(
          (
            await db.usageReading.findMany({
              where: { userId: "ann", accountKey: K1 },
              select: { windowId: true },
            })
          ).map((r) => r.windowId),
        ),
      ].sort();
    expect(await stored()).toEqual([
      "session",
      "weekly_a",
      "weekly_all",
      "weekly_b",
      "weekly_opus",
    ]);

    // Known windows keep coming in; a reading of nothing but new ones stores nothing.
    expect(
      await applySync(
        db,
        "ann",
        windows(["weekly_e", "weekly_a"], "2026-09-25T11:05:00Z"),
        NOW,
        quotas,
      ),
    ).toEqual({ sessions: 0, usage: 1 });
    expect(
      await applySync(
        db,
        "ann",
        windows(["weekly_e"], "2026-09-25T11:06:00Z"),
        NOW,
        quotas,
      ),
    ).toEqual({ sessions: 0, usage: 0 });
    expect(await stored()).not.toContain("weekly_e");

    // Pruning the readings doesn't free the ids: they count "ever".
    const later = new Date(NOW.getTime() + 91 * DAY_MS);
    const afterPruning = windows(["weekly_f"], "2026-09-25T11:00:00Z");
    afterPruning.usage[0]!.observedAt = later.toISOString(); // as that day's clock allows
    await applySync(db, "ann", afterPruning, later, quotas);
    expect(await stored()).toEqual([]);
    expect(
      await db.usageWindow.count({ where: { userId: "ann", accountKey: K1 } }),
    ).toBe(5);

    // Other people and other accounts have their own ids.
    await createUser("bob", "Bob");
    await applySync(
      db,
      "bob",
      windows(madeUp, "2026-09-25T11:00:00Z"),
      NOW,
      quotas,
    );
    expect(await db.usageReading.count({ where: { userId: "bob" } })).toBe(4);
  });

  it("holds one person to 20,000 new readings a day, whatever the batches carry", async () => {
    // The storage attack: full batches of made-up windows at distinct in-range dates, back to back.
    const full = (prefix: string, from: number) =>
      request((r) => {
        r.accounts = [r.accounts[0]!];
        r.sessions = [];
        r.usage = Array.from({ length: 500 }, (_, i) => ({
          ...r.usage[0]!,
          observedAt: new Date(
            Date.UTC(2026, 8, 1) + (from + i) * 60_000,
          ).toISOString(),
          windows: Array.from({ length: 20 }, (_, w) => ({
            id: `weekly_${prefix}_${w}`,
            utilization: w,
            resetsAt: null,
          })),
        }));
      });
    // 20 window ids: all 10,000 readings fit.
    expect(await applySync(db, "ann", full("a", 0), NOW)).toEqual({
      sessions: 0,
      usage: 500,
    });
    // 20 more ids, but only 12 fit under the 32-id cap: 6,000 readings.
    expect(await applySync(db, "ann", full("b", 1000), NOW)).toEqual({
      sessions: 0,
      usage: 500,
    });
    expect(await db.usageReading.count()).toBe(16_000);
    // Known ids again: the day's 20,000 run out after 4,000, the newest.
    expect(await applySync(db, "ann", full("a", 2000), NOW)).toEqual({
      sessions: 0,
      usage: 200,
    });
    expect(await db.usageReading.count()).toBe(
      SYNC_QUOTAS.newUsageReadingsPerDay,
    );
    expect(await applySync(db, "ann", full("a", 3000), NOW)).toEqual({
      sessions: 0,
      usage: 0,
    });
    expect(await db.usageReading.count()).toBe(20_000);
    expect(
      await db.usageWindow.count({ where: { userId: "ann", accountKey: K1 } }),
    ).toBe(SYNC_QUOTAS.windowsPerAccount);
  });

  it("drops usage readings past 90 days, on arrival and from what is stored", async () => {
    await applySync(db, "ann", request(), NOW);
    expect(await db.usageReading.count()).toBe(4);
    // 91 days on, a sync of the personal account prunes its old readings (whoever sent them)…
    const later = new Date(NOW.getTime() + 91 * DAY_MS);
    await applySync(
      db,
      "ann",
      request((r) => {
        r.accounts = [r.accounts[0]!];
        r.sessions = [];
        r.usage = [
          { ...r.usage[0]!, observedAt: "2026-06-01T00:00:00Z" }, // too old: dropped
        ];
      }),
      later,
    );
    const left = await db.usageReading.findMany({
      select: { accountKey: true },
    });
    // …and leaves accounts it didn't touch for their own syncs.
    expect(left.map((r) => r.accountKey)).toEqual([K2]);
  });

  it("caps absurd token counts so totals stay summable", async () => {
    await applySync(
      db,
      "ann",
      request((r) => {
        r.sessions[0]!.tokens = {
          input: Number.MAX_SAFE_INTEGER,
          output: 5,
          cacheCreation: TOKEN_CAP + 1,
          cacheRead: TOKEN_CAP,
        };
      }),
      NOW,
    );
    expect(await session(S1)).toMatchObject({
      inputTokens: BigInt(TOKEN_CAP),
      outputTokens: 5n,
      cacheCreationTokens: BigInt(TOKEN_CAP),
      cacheReadTokens: BigInt(TOKEN_CAP),
    });
  });

  it("stores text Postgres would refuse after making it storable", async () => {
    await applySync(
      db,
      "ann",
      request((r) => {
        r.device.name = "Mac\u0000Book";
        r.accounts[0]!.label = "Per\u0000sonal";
        r.sessions[0]!.title = "Fix \u0000 bug \uD800";
        r.sessions[0]!.project.name = "agent\u0000notch";
        r.sessions[0]!.models = ["claude\u0000"];
        r.sessions[0]!.summary!.text = "Done\u0000.";
      }),
      NOW,
    );
    const s1 = await session(S1);
    expect(s1.title).toBe("Fix  bug \uFFFD");
    expect(s1.project.name).toBe("agentnotch");
    expect(s1.models).toEqual(["claude"]);
    expect(s1.summaryText).toBe("Done.");
    expect((await device("ann")).name).toBe("MacBook");
  });

  it("records other people's batches built by the seed helper", async () => {
    await createUser("bob", "Bob");
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
    expect(await counts()).toMatchObject({
      userAccounts: 1,
      projects: 1,
      sessions: 1,
      usage: 3,
    });
  });
});
