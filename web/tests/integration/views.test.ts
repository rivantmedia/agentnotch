/**
 * What the read services show on Postgres: projects grouped by name across a person's Macs, and
 * data dated in the future (a Mac whose clock runs ahead) never dating anything later than now.
 */
import { beforeEach, describe, expect, it } from "vitest";

import { loadAccessScope } from "~/server/services/access";
import { listAccounts } from "~/server/services/accounts";
import { createPoolCode, joinPool } from "~/server/services/pools";
import { getProject, listProjects } from "~/server/services/projects";
import {
  getSession,
  listSessions,
  parseSessionCursor,
  sessionCursor,
} from "~/server/services/sessions";
import { applySync } from "~/server/services/sync";

import { type SyncFixture } from "../support/fixtures";
import { createUser, db, resetDb } from "./db";
import { AFTER_FIXTURE, K1, K2, request, requestFor, S1, S2 } from "./seed";

const scope = (userId: string) => loadAccessScope(db, userId);

/** Ann's first Mac is the fixture's; this is her second. */
const FIRST_MAC = "0e6f0b4c-2f7a-4e53-9d1b-6a2c7f9e1d35";
const SECOND_MAC = "22222222-2222-4333-8444-555555555555";
const BOBS_MAC = "11111111-2222-4333-8444-555555555555";
/** agentnotch's key on Ann's second Mac (another install secret, so another key). */
const SECOND_KEY = "a".repeat(64);
const OTHER_KEY = "f".repeat(64);
const S3 = "c3c3c3c3-0000-4000-8000-000000000003";
const S4 = "c4c4c4c4-0000-4000-8000-000000000004";
const S5 = "c5c5c5c5-0000-4000-8000-000000000005";
const S6 = "c6c6c6c6-0000-4000-8000-000000000006";
const BOB_SESSION = "b0b0b0b0-0000-4000-8000-000000000001";

type SessionEdit = Partial<SyncFixture["sessions"][number]> & {
  sessionId: string;
  project: { key: string; name: string };
};

/** A batch of K1 sessions from one Mac, each built on the fixture's S1. */
function sessionsFrom(deviceId: string, sessions: SessionEdit[]) {
  return request((r) => {
    r.device.id = deviceId;
    r.accounts = [r.accounts[0]!];
    const base = r.sessions[0]!;
    r.sessions = sessions.map((edit) => {
      const session = { ...base, ...edit };
      if (!("summary" in edit)) delete session.summary;
      return session;
    });
    r.usage = [];
  });
}

const ids = (items: Array<{ sessionId: string }>) =>
  items.map((s) => s.sessionId).sort();

beforeEach(async () => {
  await resetDb();
  await createUser("ann", "Ann");
  await createUser("bob", "Bob");
});

describe("projects grouped by name", () => {
  beforeEach(async () => {
    // Ann's first Mac: S1 in agentnotch (K1), S2 in billing-service (K2); later S6 too.
    await applySync(db, "ann", request(), AFTER_FIXTURE);
    await applySync(
      db,
      "ann",
      sessionsFrom(FIRST_MAC, [
        {
          sessionId: S6,
          project: {
            key: request().sessions[0]!.project.key,
            name: "agentnotch",
          },
          startedAt: "2026-09-24T08:00:00Z",
          lastActivityAt: "2026-09-24T08:30:00Z",
          endedAt: "2026-09-24T08:31:00Z",
          tokens: {
            input: 1000,
            output: 2000,
            cacheCreation: 3000,
            cacheRead: 4000,
          },
          costUsd: 0.5,
          summary: {
            text: "Oldest summary.",
            model: "m",
            generatedAt: "2026-09-25T09:00:00Z",
          },
        },
      ]),
      AFTER_FIXTURE,
    );
    // Her second Mac: the same folder name under another key, and another folder.
    await applySync(
      db,
      "ann",
      sessionsFrom(SECOND_MAC, [
        {
          sessionId: S3,
          project: { key: SECOND_KEY, name: "agentnotch" },
          startedAt: "2026-09-25T10:00:00Z",
          lastActivityAt: "2026-09-25T10:30:00Z",
          endedAt: "2026-09-25T10:31:00Z",
          tokens: { input: 1, output: 2, cacheCreation: 3, cacheRead: 4 },
          costUsd: 1.5,
          summary: {
            text: "Newest summary.",
            model: "m",
            generatedAt: "2026-09-25T11:00:00Z",
          },
        },
        {
          sessionId: S5,
          project: { key: SECOND_KEY, name: "agentnotch" },
          startedAt: "2026-09-25T06:00:00Z",
          lastActivityAt: "2026-09-25T06:30:00Z",
          endedAt: "2026-09-25T06:31:00Z",
          tokens: { input: 10, output: 20, cacheCreation: 30, cacheRead: 40 },
          costUsd: null,
          summary: {
            text: "Middle summary.",
            model: "m",
            generatedAt: "2026-09-25T10:30:00Z",
          },
        },
        {
          sessionId: S4,
          project: { key: OTHER_KEY, name: "other-folder" },
          startedAt: "2026-09-25T07:00:00Z",
          lastActivityAt: "2026-09-25T08:00:00Z",
          endedAt: "2026-09-25T08:01:00Z",
        },
      ]),
      AFTER_FIXTURE,
    );
    // Bob has an agentnotch of his own on K1, and shares K1 with Ann.
    await applySync(
      db,
      "bob",
      requestFor({
        deviceId: BOBS_MAC,
        accountKey: K1,
        sessionId: BOB_SESSION,
        projectKey: "b".repeat(64),
        projectName: "agentnotch",
      }),
      AFTER_FIXTURE,
    );
    const { code } = await createPoolCode(db, "ann", K1);
    await joinPool(db, "bob", code);
  });

  it("list one row per person and folder name, adding up every Mac's sessions", async () => {
    const rows = await db.project.findMany({
      where: { userId: "ann", accountKey: K1, name: "agentnotch" },
      select: { id: true },
    });
    expect(rows).toHaveLength(2); // the rows stay per Mac

    const projects = await listProjects(
      db,
      await scope("ann"),
      K1,
      AFTER_FIXTURE,
    );
    expect(
      projects.map((p) => [
        p.name,
        p.owner.displayName,
        p.sessionCount,
        p.macCount,
      ]),
    ).toEqual([
      ["agentnotch", "ann@example.com", 4, 2],
      ["agentnotch", "bob@example.com", 1, 1],
      ["other-folder", "ann@example.com", 1, 1],
    ]);

    const [anns] = projects;
    expect(anns!.projectIds).toEqual(rows.map((r) => r.id).sort());
    expect(anns!.id).toBe(anns!.projectIds[0]);
    expect(anns!.tokens).toEqual({
      input: 18234n + 1000n + 1n + 10n,
      output: 96512n + 2000n + 2n + 20n,
      cacheCreation: 402118n + 3000n + 3n + 30n,
      cacheRead: 12873120n + 4000n + 4n + 40n,
      total: 18234n + 96512n + 402118n + 12873120n + 10000n + 10n + 100n,
    });
    expect(anns!.costUsd).toBeCloseTo(14.82 + 0.5 + 1.5, 6);
    expect(anns!.firstUsedAt).toEqual(new Date("2026-09-24T08:00:00Z"));
    expect(anns!.lastUsedAt).toEqual(new Date("2026-09-25T10:30:00Z"));
    // The newest three summaries across both Macs.
    expect(anns!.latestSummaries.map((s) => s.text)).toEqual([
      "Newest summary.",
      "Middle summary.",
      expect.stringContaining("stays 'working'"),
    ]);
    // Nobody is ever shown a project key.
    for (const project of projects) expect(project).not.toHaveProperty("key");

    // Bob sees the same groups, each person's apart.
    const bobs = await listProjects(db, await scope("bob"), K1, AFTER_FIXTURE);
    expect(bobs.map((p) => [p.id, p.sessionCount])).toEqual(
      projects.map((p) => [p.id, p.sessionCount]),
    );
  });

  it("filter sessions by the whole group, from any of its rows", async () => {
    const annScope = await scope("ann");
    const [anns, bobs] = await listProjects(db, annScope, K1, AFTER_FIXTURE);
    for (const projectId of anns!.projectIds) {
      const page = await listSessions(db, annScope, {
        accountKey: K1,
        projectId,
        limit: 50,
      });
      expect(ids(page.items)).toEqual([S1, S3, S5, S6].sort());
    }
    // Another person's project of the same name is another group.
    const bobsOnly = await listSessions(db, annScope, {
      accountKey: K1,
      projectId: bobs!.id,
      limit: 50,
    });
    expect(ids(bobsOnly.items)).toEqual([BOB_SESSION]);
    // A project someone can't see matches nothing.
    await createUser("dan");
    const dans = await listSessions(db, await scope("dan"), {
      projectId: anns!.id,
      limit: 50,
    });
    expect(dans.items).toEqual([]);

    // One project, looked up by any of its rows, is the group.
    for (const projectId of anns!.projectIds) {
      expect(await getProject(db, annScope, projectId, AFTER_FIXTURE)).toEqual(
        anns,
      );
    }
  });

  it("date a group by its sessions only, never by a project row without any", async () => {
    const annScope = await scope("ann");
    const [before] = await listProjects(db, annScope, K1, AFTER_FIXTURE);
    // A row of the same folder name with no session left (as a key change could leave one),
    // and a folder whose only row has none.
    const empty = await db.project.create({
      data: {
        userId: "ann",
        accountKey: K1,
        key: "d".repeat(64),
        name: "agentnotch",
      },
    });
    const lone = await db.project.create({
      data: {
        userId: "ann",
        accountKey: K1,
        key: "e".repeat(64),
        name: "no-sessions",
      },
    });

    const projects = await listProjects(db, annScope, K1, AFTER_FIXTURE);
    const anns = projects.find(
      (p) =>
        p.name === "agentnotch" && p.owner.displayName === "ann@example.com",
    )!;
    expect(anns.projectIds).toContain(empty.id);
    expect({
      firstUsedAt: anns.firstUsedAt,
      lastUsedAt: anns.lastUsedAt,
      sessionCount: anns.sessionCount,
      macCount: anns.macCount,
    }).toEqual({
      firstUsedAt: before!.firstUsedAt,
      lastUsedAt: before!.lastUsedAt,
      sessionCount: before!.sessionCount,
      macCount: before!.macCount,
    });
    expect(anns.lastUsedAt).toEqual(new Date("2026-09-25T10:30:00Z"));

    const noSessions = projects.find((p) => p.name === "no-sessions")!;
    expect(noSessions).toMatchObject({
      projectIds: [lone.id],
      sessionCount: 0,
      macCount: 0,
      firstUsedAt: null,
      lastUsedAt: null,
      latestSummaries: [],
    });
    // Never used, so listed last.
    expect(projects.at(-1)!.name).toBe("no-sessions");
    expect(await getProject(db, annScope, lone.id, AFTER_FIXTURE)).toEqual(
      noSessions,
    );
  });
});

describe("dates from the future", () => {
  // The fixture's S1 (K1) ran 08:02–09:48 and S2 (K2) started at 11:00, still running: from
  // 09:00, both reach into the future.
  const NOW = new Date("2026-09-25T09:00:00Z");

  beforeEach(async () => {
    await applySync(db, "ann", request(), AFTER_FIXTURE);
  });

  it("date no account's last activity, or project's last use, after now", async () => {
    const accounts = await listAccounts(db, await scope("ann"), NOW);
    expect(accounts.map((a) => [a.key, a.lastActivityAt])).toEqual([
      // Both are "now", so they tie and sort by key.
      [K1, NOW],
      [K2, NOW],
    ]);
    // A session started in the future counts in the periods as one started now.
    const k2 = accounts.find((a) => a.key === K2)!;
    expect(k2.last7Days.sessions).toBe(1);
    expect(k2.last30Days.tokens.input).toBe(5120n);

    const [project] = await listProjects(db, await scope("ann"), K1, NOW);
    expect(project!.lastUsedAt).toEqual(NOW);
    expect(project!.firstUsedAt).toEqual(new Date("2026-09-25T08:02:11.482Z"));
  });

  it("show a session's times no later than now, and keep a running one running", async () => {
    const page = await listSessions(db, await scope("ann"), { limit: 10 }, NOW);
    const [s2, s1] = page.items;
    expect(s2).toMatchObject({
      sessionId: S2,
      startedAt: NOW,
      lastActivityAt: NOW,
      endedAt: null,
    });
    expect(s1).toMatchObject({
      sessionId: S1,
      startedAt: new Date("2026-09-25T08:02:11.482Z"),
      lastActivityAt: NOW,
      endedAt: NOW,
    });
    expect(await getSession(db, await scope("ann"), s1!.id, NOW)).toEqual(s1);
  });

  it("date no summary after now, in sessions or in projects", async () => {
    // S1's summary was written at 10:05, an hour after this now.
    const annScope = await scope("ann");
    const page = await listSessions(db, annScope, { limit: 10 }, NOW);
    const s1 = page.items.find((s) => s.sessionId === S1)!;
    expect(s1.summary).toMatchObject({ generatedAt: NOW });
    expect((await getSession(db, annScope, s1.id, NOW)).summary).toEqual(
      s1.summary,
    );
    const [project] = await listProjects(db, annScope, K1, NOW);
    expect(project!.latestSummaries.map((s) => s.generatedAt)).toEqual([NOW]);

    // Once it has passed, the time shows as written.
    const written = new Date("2026-09-25T10:05:00Z");
    const later = await listSessions(
      db,
      annScope,
      { limit: 10 },
      AFTER_FIXTURE,
    );
    expect(
      later.items.find((s) => s.sessionId === S1)!.summary!.generatedAt,
    ).toEqual(written);
    const [laterProject] = await listProjects(db, annScope, K1, AFTER_FIXTURE);
    expect(laterProject!.latestSummaries[0]!.generatedAt).toEqual(written);
  });

  it("sort sessions dated in the future as now, tied by id", async () => {
    await applySync(
      db,
      "ann",
      sessionsFrom(FIRST_MAC, [
        {
          sessionId: S3,
          project: { key: OTHER_KEY, name: "later" },
          startedAt: "2026-09-25T12:00:00Z",
          lastActivityAt: "2026-09-25T12:10:00Z",
        },
        {
          sessionId: S4,
          project: { key: OTHER_KEY, name: "later" },
          startedAt: "2026-09-25T10:00:00Z",
          lastActivityAt: "2026-09-25T10:10:00Z",
        },
      ]),
      AFTER_FIXTURE,
    );
    // Row ids that sort the other way round from the start times.
    for (const [sessionId, id] of [
      [S3, "session-a"],
      [S4, "session-z"],
    ] as const) {
      await db.session.update({
        where: {
          userId_accountKey_sessionId: {
            userId: "ann",
            accountKey: K1,
            sessionId,
          },
        },
        data: { id },
      });
    }

    const annScope = await scope("ann");
    const page = await listSessions(
      db,
      annScope,
      { accountKey: K1, limit: 10 },
      NOW,
    );
    // Both count as now: the later id first, then the rest by start.
    expect(page.items.map((s) => s.sessionId)).toEqual([S4, S3, S1]);

    // Paging walks the same order without gaps or repeats.
    const walked: string[] = [];
    let cursor: string | undefined;
    do {
      const next = await listSessions(
        db,
        annScope,
        { accountKey: K1, limit: 1, cursor },
        NOW,
      );
      walked.push(...next.items.map((s) => s.sessionId));
      cursor = next.nextCursor ?? undefined;
    } while (cursor);
    expect(walked).toEqual([S4, S3, S1]);
  });

  it("page through one snapshot, however much later the next page loads", async () => {
    await applySync(
      db,
      "ann",
      sessionsFrom(FIRST_MAC, [
        {
          sessionId: S3,
          project: { key: OTHER_KEY, name: "later" },
          startedAt: "2026-09-25T12:00:00Z",
          lastActivityAt: "2026-09-25T12:10:00Z",
        },
        {
          sessionId: S4,
          project: { key: OTHER_KEY, name: "later" },
          startedAt: "2026-09-25T10:00:00Z",
          lastActivityAt: "2026-09-25T10:10:00Z",
        },
      ]),
      AFTER_FIXTURE,
    );
    // At 09:00 both start "now" and tie, so the later id (S4's) comes first. By 13:00 both have
    // started, S3 after S4: clamped against 13:00, S3 would sort before the cursor and be lost.
    for (const [sessionId, id] of [
      [S3, "session-a"],
      [S4, "session-z"],
    ] as const) {
      await db.session.update({
        where: {
          userId_accountKey_sessionId: {
            userId: "ann",
            accountKey: K1,
            sessionId,
          },
        },
        data: { id },
      });
    }
    const annScope = await scope("ann");
    const afterwards = new Date("2026-09-25T13:00:00Z");

    const first = await listSessions(
      db,
      annScope,
      { accountKey: K1, limit: 1 },
      NOW,
    );
    expect(first.items.map((s) => s.sessionId)).toEqual([S4]);
    expect(parseSessionCursor(first.nextCursor!)).toEqual({
      asOf: NOW,
      id: "session-z",
    });

    const walked = first.items.map((s) => s.sessionId);
    let cursor = first.nextCursor ?? undefined;
    while (cursor) {
      const next = await listSessions(
        db,
        annScope,
        { accountKey: K1, limit: 1, cursor },
        afterwards,
      );
      walked.push(...next.items.map((s) => s.sessionId));
      // The same snapshot, all the way: its times are as of the first page.
      for (const item of next.items) {
        expect(item.startedAt.getTime()).toBeLessThanOrEqual(NOW.getTime());
      }
      cursor = next.nextCursor ?? undefined;
      if (cursor) expect(parseSessionCursor(cursor)!.asOf).toEqual(NOW);
    }
    expect(walked).toEqual([S4, S3, S1]);
  });

  it("never date a page after now, whatever time its cursor claims", async () => {
    const annScope = await scope("ann");
    const all = await listSessions(db, annScope, { limit: 10 }, NOW);
    // A cursor claiming tomorrow, after the first row: the rest is still as of now.
    const tomorrow = new Date(NOW.getTime() + 24 * 60 * 60 * 1000);
    const page = await listSessions(
      db,
      annScope,
      { limit: 10, cursor: sessionCursor(tomorrow, all.items[0]!.id) },
      NOW,
    );
    expect(page.items.map((s) => s.id)).toEqual(
      all.items.slice(1).map((s) => s.id),
    );
    for (const item of page.items) {
      expect(item.lastActivityAt.getTime()).toBeLessThanOrEqual(NOW.getTime());
    }
    // Something that isn't a cursor is refused, not read as the start.
    for (const cursor of ["", "session-a", "abc.session-a", "123."]) {
      await expect(
        listSessions(db, annScope, { limit: 10, cursor }, NOW),
        cursor,
      ).rejects.toMatchObject({ code: "BAD_REQUEST" });
    }
  });

  it("leave readings dated after now out of the latest reading", async () => {
    // The fixture's K1 reading is from 09:50; an earlier one is from 09:00.
    await applySync(
      db,
      "ann",
      request((r) => {
        r.sessions = [];
        r.usage = [
          {
            ...r.usage[0]!,
            observedAt: "2026-09-25T09:00:00Z",
            windows: r.usage[0]!.windows.map((w) => ({ ...w, utilization: 5 })),
          },
        ];
      }),
      AFTER_FIXTURE,
    );
    // Two minutes before the 09:50 reading: it isn't the latest yet.
    const latest = async (now: string) => {
      const accounts = await listAccounts(
        db,
        await scope("ann"),
        new Date(now),
      );
      const k1 = accounts.find((a) => a.key === K1)!;
      return k1.usage.find((u) => u.windowId === "session")!;
    };
    expect(await latest("2026-09-25T09:48:00Z")).toMatchObject({
      observedAt: new Date("2026-09-25T09:00:00Z"),
      utilization: 5,
    });
    // At 09:50 it is.
    expect(await latest("2026-09-25T09:50:00Z")).toMatchObject({
      observedAt: new Date("2026-09-25T09:50:00Z"),
      utilization: 42,
    });
  });
});
