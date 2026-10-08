/**
 * One Claude session, counted once, on Postgres:
 * - copies: a Mac signed in to the website as one person and later as another sends its sessions
 *   again, so two people hold the same (account, session id) with the same project key. Pooled
 *   on that account, every view counts and lists one copy (sql.ts `canonicalSessionSql`), and a
 *   folder that holds only the other person's copies isn't listed. A copy the viewer can't see
 *   never changes what they see.
 * - parts: a session resumed under another account is stored once per account with that
 *   account's share. It counts once per account, and once in totals across accounts.
 */
import { beforeEach, describe, expect, it } from "vitest";

import {
  AccessDenied,
  loadAccessScope,
  type AccessScope,
} from "~/server/services/access";
import { listAccounts } from "~/server/services/accounts";
import { combinedUsage, usageTimeline } from "~/server/services/combined-usage";
import { removeSummaries } from "~/server/services/my-data";
import { createPoolCode, joinPool } from "~/server/services/pools";
import {
  projectDetail,
  projectUsage,
  type ProjectUsage,
} from "~/server/services/project-usage";
import { getProject, listProjects } from "~/server/services/projects";
import { getSession, listSessions } from "~/server/services/sessions";
import { applySync } from "~/server/services/sync";

import { type SyncFixture } from "../support/fixtures";
import { createUser, db, resetDb } from "./db";
import { AFTER_FIXTURE, K1, K2, request } from "./seed";

const scope = (userId: string) => loadAccessScope(db, userId);

/** The Mac Ann and Bob both signed in on, one after the other. */
const SHARED_MAC = "0e6f0b4c-2f7a-4e53-9d1b-6a2c7f9e1d35";
const BOBS_MAC = "11111111-2222-4333-8444-555555555555";
const CAROLS_MAC = "33333333-2222-4333-8444-555555555555";
/** The shared Mac's key for its folder "shared-repo" on K1 (one install secret, so one key). */
const SHARED_KEY = "5".repeat(64);
const BOBS_KEY = "b".repeat(64);
const CAROLS_KEY = "c".repeat(64);

/** Synced from the shared Mac by both: Bob's copy is fresher (later activity, more tokens). */
const FRESHER = "d1d1d1d1-0000-4000-8000-000000000001";
/** Synced identically by both, Ann last. */
const IDENTICAL = "d2d2d2d2-0000-4000-8000-000000000002";
/** The same last activity, but Bob's copy saw the end and Ann's (synced later) didn't. */
const ENDED = "d3d3d3d3-0000-4000-8000-000000000003";
/** Bob's own session on his own Mac. */
const BOBS_OWN = "d4d4d4d4-0000-4000-8000-000000000004";

const tokens = (input: number) => ({
  input,
  output: input * 2,
  cacheCreation: input * 3,
  cacheRead: input * 4,
});
const total = (input: number) => BigInt(input * 10);

type SessionEdit = Partial<SyncFixture["sessions"][number]> & {
  sessionId: string;
};

/** A batch of sessions on one account from one Mac, built on the fixture's session there. */
function sessionsOn(
  deviceId: string,
  accountKey: string,
  project: { key: string; name: string },
  sessions: SessionEdit[],
) {
  return request((r) => {
    r.device.id = deviceId;
    r.accounts = r.accounts.filter((a) => a.key === accountKey);
    const base = r.sessions.find((s) => s.accountKey === accountKey)!;
    r.sessions = sessions.map((edit) => {
      const session = { ...base, project, ...edit, accountKey };
      if (!("summary" in edit)) delete session.summary;
      return session;
    });
    r.usage = [];
  });
}

const HOUR = 60 * 60 * 1000;
const at = (offsetHours: number) =>
  new Date(AFTER_FIXTURE.getTime() + offsetHours * HOUR);

const fresher = (lastActivityAt: string, input: number): SessionEdit => ({
  sessionId: FRESHER,
  startedAt: "2026-09-20T08:00:00Z",
  lastActivityAt,
  endedAt: null,
  messageCount: 10,
  tokens: tokens(input),
  costUsd: input / 100,
});
const identical: SessionEdit = {
  sessionId: IDENTICAL,
  startedAt: "2026-09-21T08:00:00Z",
  lastActivityAt: "2026-09-21T09:00:00Z",
  endedAt: "2026-09-21T09:01:00Z",
  messageCount: 4,
  tokens: tokens(20),
  costUsd: 0.2,
};
const ended = (endedAt: string | null): SessionEdit => ({
  sessionId: ENDED,
  startedAt: "2026-09-22T08:00:00Z",
  lastActivityAt: "2026-09-22T09:00:00Z",
  endedAt,
  messageCount: 6,
  tokens: tokens(30),
  costUsd: 0.3,
});

async function sessionRowId(userId: string, sessionId: string) {
  const row = await db.session.findFirstOrThrow({
    where: { userId, sessionId },
    select: { id: true },
  });
  return row.id;
}

async function projectRowId(userId: string, name: string) {
  const row = await db.project.findFirstOrThrow({
    where: { userId, name },
    select: { id: true },
  });
  return row.id;
}

async function denied(promise: Promise<unknown>): Promise<string> {
  try {
    await promise;
  } catch (error) {
    if (error instanceof AccessDenied) return error.code;
    throw error;
  }
  throw new Error("expected AccessDenied");
}

/** Every view's session figures for one viewer, side by side. */
async function views(viewer: AccessScope) {
  const [accounts, usage, timeline, byProject, projects, sessions] =
    await Promise.all([
      listAccounts(db, viewer, AFTER_FIXTURE),
      combinedUsage(db, viewer, { period: "30d" }, AFTER_FIXTURE),
      usageTimeline(
        db,
        viewer,
        { period: "30d", timeZone: "UTC" },
        AFTER_FIXTURE,
      ),
      projectUsage(db, viewer, { period: "30d" }, AFTER_FIXTURE),
      listProjects(db, viewer, K1, AFTER_FIXTURE),
      listSessions(db, viewer, { accountKey: K1, limit: 50 }, AFTER_FIXTURE),
    ]);
  return { accounts, usage, timeline, byProject, projects, sessions };
}

const owners = (projects: ProjectUsage[]) =>
  projects.map((p) => [p.name, p.owner.id, p.sessions, p.tokens.total]);

describe("copies of one session from two people", () => {
  beforeEach(async () => {
    await resetDb();
    await createUser("ann", "Ann");
    await createUser("bob", "Bob");
    await createUser("carol", "Carol");
    const shared = { key: SHARED_KEY, name: "shared-repo" };

    // The shared Mac, signed in as Bob, sends the identical and ended copies first.
    await applySync(
      db,
      "bob",
      sessionsOn(SHARED_MAC, K1, shared, [
        fresher("2026-09-20T10:00:00Z", 200),
        identical,
        ended("2026-09-22T09:01:00Z"),
      ]),
      at(0),
    );
    // Then, signed in as Ann, the same ledger: her FRESHER copy is staler (Bob's had more
    // activity), IDENTICAL is the same, and ENDED hasn't seen the end yet.
    await applySync(
      db,
      "ann",
      sessionsOn(SHARED_MAC, K1, shared, [
        fresher("2026-09-20T09:00:00Z", 100),
        identical,
        ended(null),
      ]),
      at(1),
    );
    // Bob's own work, on his own Mac.
    await applySync(
      db,
      "bob",
      sessionsOn(BOBS_MAC, K1, { key: BOBS_KEY, name: "bobs-repo" }, [
        {
          sessionId: BOBS_OWN,
          startedAt: "2026-09-23T08:00:00Z",
          lastActivityAt: "2026-09-23T09:00:00Z",
          endedAt: "2026-09-23T09:01:00Z",
          messageCount: 2,
          tokens: tokens(7),
          costUsd: 0.07,
        },
      ]),
      at(0),
    );
    // Carol, outside the pool, holds FRESHER too, fresher than anyone's.
    await applySync(
      db,
      "carol",
      sessionsOn(CAROLS_MAC, K1, { key: CAROLS_KEY, name: "shared-repo" }, [
        fresher("2026-09-20T23:00:00Z", 999),
      ]),
      at(2),
    );
  });

  async function pool() {
    const { code } = await createPoolCode(db, "ann", K1);
    await joinPool(db, "bob", code);
  }

  it("count before pooling: each person sees their own rows only", async () => {
    const ann = await views(await scope("ann"));
    expect(ann.accounts[0]!.last30Days.sessions).toBe(3);
    expect(ann.accounts[0]!.last30Days.tokens.total).toBe(
      total(100) + total(20) + total(30),
    );
    expect(ann.usage.total.sessions).toBe(3);
    expect(owners(ann.byProject.projects)).toEqual([
      ["shared-repo", "ann", 3, total(100) + total(20) + total(30)],
    ]);
    expect(ann.projects.map((p) => [p.owner.id, p.sessionCount])).toEqual([
      ["ann", 3],
    ]);
    expect(ann.sessions.items.map((s) => s.sessionId).sort()).toEqual(
      [ENDED, FRESHER, IDENTICAL].sort(),
    );
  });

  it("count each session once, from the fresher, ended or last-synced copy", async () => {
    await pool();
    const [annRow, bobRow] = await Promise.all([
      Promise.all(
        [FRESHER, IDENTICAL, ENDED].map((s) => sessionRowId("ann", s)),
      ),
      Promise.all(
        [FRESHER, IDENTICAL, ENDED].map((s) => sessionRowId("bob", s)),
      ),
    ]);
    // FRESHER: Bob's (later activity). IDENTICAL: Ann's (synced last). ENDED: Bob's (ended).
    const counted = [bobRow[0]!, annRow[1]!, bobRow[2]!];
    const sessions = 4;
    const tokensSeen = total(200) + total(20) + total(30) + total(7);

    for (const viewer of ["ann", "bob"]) {
      const v = await views(await scope(viewer));

      const card = v.accounts.find((a) => a.key === K1)!;
      expect(card.last30Days.sessions).toBe(sessions);
      expect(card.last30Days.tokens.total).toBe(tokensSeen);
      expect(card.last30Days.costUsd).toBeCloseTo(2 + 0.2 + 0.3 + 0.07);
      expect(card.lastActivityAt).toEqual(new Date("2026-09-23T09:00:00Z"));

      expect(v.usage.total.sessions).toBe(sessions);
      expect(v.usage.total.tokens.total).toBe(tokensSeen);
      expect(v.usage.accounts.map((a) => [a.accountKey, a.sessions])).toEqual([
        [K1, sessions],
      ]);
      expect(
        v.timeline.buckets
          .filter((b) => b.sessions > 0)
          .map((b) => [b.start.toISOString().slice(0, 10), b.sessions]),
      ).toEqual([
        ["2026-09-20", 1],
        ["2026-09-21", 1],
        ["2026-09-22", 1],
        ["2026-09-23", 1],
      ]);

      // The shared Mac's folder is listed once, under the person whose copies count; Ann's
      // holds only copies of Bob's FRESHER and ENDED plus the IDENTICAL that counts, so it
      // stays with one session.
      expect(owners(v.byProject.projects)).toEqual([
        ["shared-repo", "bob", 2, total(200) + total(30)],
        ["shared-repo", "ann", 1, total(20)],
        ["bobs-repo", "bob", 1, total(7)],
      ]);
      expect(v.byProject.total.sessions).toBe(sessions);
      expect(v.byProject.total.tokens.total).toBe(tokensSeen);

      expect(
        v.projects.map((p) => [p.name, p.owner.id, p.sessionCount, p.macCount]),
      ).toEqual([
        ["bobs-repo", "bob", 1, 1],
        ["shared-repo", "bob", 2, 1],
        ["shared-repo", "ann", 1, 1],
      ]);

      expect(v.sessions.items.map((s) => s.id).sort()).toEqual(
        [...counted, await sessionRowId("bob", BOBS_OWN)].sort(),
      );
    }

    const ann = await scope("ann");
    // Filters apply to the copies that count: Ann's own list holds only IDENTICAL now.
    const annsOwn = await listSessions(
      db,
      ann,
      { accountKey: K1, ownerId: "ann", limit: 50 },
      AFTER_FIXTURE,
    );
    expect(annsOwn.items.map((s) => s.sessionId)).toEqual([IDENTICAL]);
    const inAnnsFolder = await listSessions(
      db,
      ann,
      { projectId: await projectRowId("ann", "shared-repo"), limit: 50 },
      AFTER_FIXTURE,
    );
    expect(inAnnsFolder.items.map((s) => s.sessionId)).toEqual([IDENTICAL]);
    // A copy that doesn't count still opens by its id.
    expect((await getSession(db, ann, annRow[0]!, AFTER_FIXTURE)).id).toBe(
      annRow[0],
    );
  });

  it("leave out a folder that holds only someone else's copies", async () => {
    await pool();
    // Ann's IDENTICAL copy loses too once Bob's is synced later.
    await applySync(
      db,
      "bob",
      sessionsOn(SHARED_MAC, K1, { key: SHARED_KEY, name: "shared-repo" }, [
        identical,
      ]),
      at(3),
    );
    const ann = await scope("ann");
    const v = await views(ann);
    expect(owners(v.byProject.projects)).toEqual([
      ["shared-repo", "bob", 3, total(200) + total(20) + total(30)],
      ["bobs-repo", "bob", 1, total(7)],
    ]);
    expect(v.byProject.total).toMatchObject({ sessions: 4, projects: 2 });
    expect(v.projects.map((p) => [p.name, p.owner.id])).toEqual([
      ["bobs-repo", "bob"],
      ["shared-repo", "bob"],
    ]);
    // Its id (a bookmark, an open tab) leads to the folder under Bob, whose copies count: its
    // pages and its sessions filter, on its account and across accounts.
    const annsFolder = await projectRowId("ann", "shared-repo");
    const bobsFolderId = await projectRowId("bob", "shared-repo");
    expect(await getProject(db, ann, annsFolder, AFTER_FIXTURE)).toMatchObject({
      id: bobsFolderId,
      owner: { id: "bob" },
      sessionCount: 3,
    });
    expect(
      await projectDetail(db, ann, annsFolder, "30d", AFTER_FIXTURE),
    ).toMatchObject({ id: bobsFolderId, owner: { id: "bob" }, sessions: 3 });
    for (const acrossAccounts of [false, true]) {
      const page = await listSessions(
        db,
        ann,
        { projectId: annsFolder, acrossAccounts, limit: 50 },
        AFTER_FIXTURE,
      );
      expect(page.items.map((s) => s.id).sort()).toEqual(
        (
          await Promise.all(
            [FRESHER, IDENTICAL, ENDED].map((s) => sessionRowId("bob", s)),
          )
        ).sort(),
      );
    }
    // A folder the viewer can't see stays missing.
    const carolsFolder = await projectRowId("carol", "shared-repo");
    expect(await denied(getProject(db, ann, carolsFolder, AFTER_FIXTURE))).toBe(
      "NOT_FOUND",
    );
    expect(
      await denied(projectDetail(db, ann, carolsFolder, "30d", AFTER_FIXTURE)),
    ).toBe("NOT_FOUND");
    const bobsFolder = await projectDetail(
      db,
      ann,
      await projectRowId("bob", "shared-repo"),
      "30d",
      AFTER_FIXTURE,
    );
    expect(bobsFolder.sessions).toBe(3);
    expect(bobsFolder.firstUsedAt).toEqual(new Date("2026-09-20T08:00:00Z"));
    expect(bobsFolder.accounts.map((a) => [a.accountKey, a.sessions])).toEqual([
      [K1, 3],
    ]);
    // Its summaries list (none here) and dates come from the copies that count.
    const summary = await getProject(
      db,
      ann,
      await projectRowId("bob", "shared-repo"),
      AFTER_FIXTURE,
    );
    expect(summary).toMatchObject({
      sessionCount: 3,
      firstUsedAt: new Date("2026-09-20T08:00:00Z"),
      lastUsedAt: new Date("2026-09-22T09:00:00Z"),
    });
  });

  it("show the summary only the copy that doesn't count carries", async () => {
    await pool();
    const text = "Taught the notch to count every session once.";
    // Ann's IDENTICAL copy has a summary; Bob's, sent again later with summaries off (a new
    // sign-in starts with them off), has none, and counts.
    await applySync(
      db,
      "ann",
      sessionsOn(SHARED_MAC, K1, { key: SHARED_KEY, name: "shared-repo" }, [
        {
          ...identical,
          summary: {
            text,
            model: "claude-haiku-4-5-20251001",
            generatedAt: "2026-09-21T10:00:00Z",
          },
        },
      ]),
      at(1),
    );
    await applySync(
      db,
      "bob",
      sessionsOn(SHARED_MAC, K1, { key: SHARED_KEY, name: "shared-repo" }, [
        identical,
      ]),
      at(3),
    );
    const bobsCopy = await sessionRowId("bob", IDENTICAL);
    const expected = {
      text,
      model: "claude-haiku-4-5-20251001",
      generatedAt: new Date("2026-09-21T10:00:00Z"),
    };

    for (const viewer of ["ann", "bob"]) {
      const v = await scope(viewer);
      const list = await listSessions(
        db,
        v,
        { accountKey: K1, limit: 50 },
        AFTER_FIXTURE,
      );
      expect(list.items.find((s) => s.sessionId === IDENTICAL)).toMatchObject({
        id: bobsCopy,
        summary: expected,
      });
      const found = await listSessions(
        db,
        v,
        { search: "every session once", limit: 50 },
        AFTER_FIXTURE,
      );
      expect(found.items.map((s) => s.id)).toEqual([bobsCopy]);
      const projects = await listProjects(db, v, K1, AFTER_FIXTURE);
      expect(
        projects.find((p) => p.owner.id === "bob" && p.name === "shared-repo")!
          .latestSummaries,
      ).toEqual([
        {
          sessionRowId: bobsCopy,
          title: expect.anything() as unknown,
          ...expected,
        },
      ]);
      expect(
        (await getSession(db, v, bobsCopy, AFTER_FIXTURE)).summary,
      ).toEqual(expected);
    }

    // Nobody else's summary reaches a viewer outside the pool: Carol's FRESHER has none.
    const carol = await listSessions(
      db,
      await scope("carol"),
      { search: "every session once", limit: 50 },
      AFTER_FIXTURE,
    );
    expect(carol.items).toEqual([]);

    // Removing Ann's summaries leaves which copy counts alone: Bob's, synced last, still does.
    const before = await db.session.findFirstOrThrow({
      where: { userId: "ann", sessionId: IDENTICAL },
      select: { updatedAt: true },
    });
    expect(await removeSummaries(db, "ann")).toEqual({ sessions: 1 });
    const after = await db.session.findFirstOrThrow({
      where: { userId: "ann", sessionId: IDENTICAL },
      select: { updatedAt: true, summaryText: true },
    });
    expect(after).toEqual({ updatedAt: before.updatedAt, summaryText: null });
    const list = await listSessions(
      db,
      await scope("ann"),
      { accountKey: K1, limit: 50 },
      AFTER_FIXTURE,
    );
    expect(list.items.find((s) => s.sessionId === IDENTICAL)).toMatchObject({
      id: bobsCopy,
      summary: null,
    });
  });

  it("never let a copy the viewer can't see change what they see", async () => {
    await pool();
    // Carol's FRESHER is the freshest of all, but neither Ann nor Bob sees it: Bob's still
    // counts for them.
    const ann = await views(await scope("ann"));
    expect(ann.sessions.items.map((s) => s.id)).toContain(
      await sessionRowId("bob", FRESHER),
    );
    expect(ann.accounts[0]!.last30Days.tokens.total).toBe(
      total(200) + total(20) + total(30) + total(7),
    );
    // And Carol, who shares nothing, sees her own row exactly as before.
    const carol = await views(await scope("carol"));
    expect(carol.accounts[0]!.last30Days).toMatchObject({ sessions: 1 });
    expect(carol.accounts[0]!.last30Days.tokens.total).toBe(total(999));
    expect(carol.usage.total.sessions).toBe(1);
    expect(owners(carol.byProject.projects)).toEqual([
      ["shared-repo", "carol", 1, total(999)],
    ]);
    expect(carol.sessions.items.map((s) => s.id)).toEqual([
      await sessionRowId("carol", FRESHER),
    ]);
  });

  it("never lend a summary from a copy the viewer can't see", async () => {
    await pool();
    const text = "Carol's own words about the session.";
    // Carol, outside the pool, has a summary of FRESHER; neither Ann's nor Bob's copy has one.
    await applySync(
      db,
      "carol",
      sessionsOn(CAROLS_MAC, K1, { key: CAROLS_KEY, name: "shared-repo" }, [
        {
          ...fresher("2026-09-20T23:00:00Z", 999),
          summary: {
            text,
            model: "claude-haiku-4-5-20251001",
            generatedAt: "2026-09-21T10:00:00Z",
          },
        },
      ]),
      at(3),
    );
    const bobsCopy = await sessionRowId("bob", FRESHER);
    for (const viewer of ["ann", "bob"]) {
      const v = await scope(viewer);
      const list = await listSessions(
        db,
        v,
        { accountKey: K1, limit: 50 },
        AFTER_FIXTURE,
      );
      expect(list.items.find((s) => s.sessionId === FRESHER)).toMatchObject({
        id: bobsCopy,
        summary: null,
      });
      const found = await listSessions(
        db,
        v,
        { search: "Carol's own words", limit: 50 },
        AFTER_FIXTURE,
      );
      expect(found.items).toEqual([]);
      const projects = await listProjects(db, v, K1, AFTER_FIXTURE);
      expect(projects.flatMap((p) => p.latestSummaries)).toEqual([]);
      expect((await getSession(db, v, bobsCopy, AFTER_FIXTURE)).summary).toBe(
        null,
      );
    }
    // Carol herself sees it on her own row.
    const carols = await listSessions(
      db,
      await scope("carol"),
      { search: "Carol's own words", limit: 50 },
      AFTER_FIXTURE,
    );
    expect(carols.items.map((s) => s.id)).toEqual([
      await sessionRowId("carol", FRESHER),
    ]);
  });
});

describe("one session resumed under another account", () => {
  /** Resumed on K2 two days after it began on K1. */
  const SPLIT = "e1e1e1e1-0000-4000-8000-000000000001";
  /** Resumed on K2 the same day. */
  const SAME_DAY = "e2e2e2e2-0000-4000-8000-000000000002";
  const repo = (accountKey: string) => ({
    key: accountKey === K1 ? "1".repeat(64) : "2".repeat(64),
    name: "repo",
  });

  beforeEach(async () => {
    await resetDb();
    await createUser("ann", "Ann");
    for (const [accountKey, splitStart, input] of [
      [K1, "2026-09-18T08:00:00Z", 100],
      [K2, "2026-09-20T08:00:00Z", 50],
    ] as const) {
      await applySync(
        db,
        "ann",
        sessionsOn(SHARED_MAC, accountKey, repo(accountKey), [
          {
            sessionId: SPLIT,
            startedAt: splitStart,
            lastActivityAt: splitStart.replace("08:00", "09:00"),
            endedAt: null,
            tokens: tokens(input),
            costUsd: input / 100,
          },
          {
            sessionId: SAME_DAY,
            startedAt:
              accountKey === K1
                ? "2026-09-23T08:00:00Z"
                : "2026-09-23T10:00:00Z",
            lastActivityAt: "2026-09-23T11:00:00Z",
            endedAt: null,
            tokens: tokens(1),
            costUsd: 0.01,
          },
        ]),
        AFTER_FIXTURE,
      );
    }
  });

  it("counts once per account, and once across them", async () => {
    const ann = await scope("ann");
    const v = await views(ann);
    const both = total(100) + total(50) + total(1) * 2n;

    expect(
      v.accounts.map((a) => [a.key, a.last30Days.sessions]).sort(),
    ).toEqual([
      [K1, 2],
      [K2, 2],
    ]);
    expect(v.usage.accounts.map((a) => [a.accountKey, a.sessions])).toEqual([
      [K1, 2],
      [K2, 2],
    ]);
    expect(v.usage.total.sessions).toBe(2);
    expect(v.usage.total.tokens.total).toBe(both);
    expect(v.usage.total.costUsd).toBeCloseTo(1.5 + 0.02);

    // One project, its parts per account as they are.
    expect(owners(v.byProject.projects)).toEqual([["repo", "ann", 2, both]]);
    expect(
      v.byProject.projects[0]!.accounts.map((a) => [a.accountKey, a.sessions]),
    ).toEqual([
      [K1, 2],
      [K2, 2],
    ]);
    expect(v.byProject.total).toMatchObject({ sessions: 2, projects: 1 });
    // Folded away past the limit, the rest counts it once too.
    const folded = await projectUsage(
      db,
      ann,
      { period: "30d", limit: 0 },
      AFTER_FIXTURE,
    );
    expect(folded.rest).toMatchObject({ sessions: 2, projects: 1 });
    const detail = await projectDetail(
      db,
      ann,
      v.byProject.projects[0]!.id,
      "30d",
      AFTER_FIXTURE,
    );
    expect(detail.sessions).toBe(2);
    expect(detail.tokens.total).toBe(both);

    // The account page counts its own part only.
    expect(v.projects.map((p) => [p.name, p.sessionCount])).toEqual([
      ["repo", 2],
    ]);

    // A bucket counts the session once for each part that started in it.
    expect(
      v.timeline.buckets
        .filter((b) => b.sessions > 0)
        .map((b) => [
          b.start.toISOString().slice(0, 10),
          b.sessions,
          b.accounts.map((a) => [a.accountKey, a.sessions]),
        ]),
    ).toEqual([
      ["2026-09-18", 1, [[K1, 1]]],
      ["2026-09-20", 1, [[K2, 1]]],
      [
        "2026-09-23",
        1,
        [
          [K1, 1],
          [K2, 1],
        ],
      ],
    ]);
  });
});
