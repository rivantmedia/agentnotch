/**
 * Usage by project and a project's usage by account, on Postgres: a person's folders of one name
 * grouped across Macs and accounts, the periods, pooled visibility, and the sessions filter that
 * spans accounts.
 */
import { beforeEach, describe, expect, it } from "vitest";

import { AccessDenied, loadAccessScope } from "~/server/services/access";
import { listAccounts } from "~/server/services/accounts";
import { createPoolCode, joinPool } from "~/server/services/pools";
import {
  canSeeProject,
  projectDetail,
  projectUsage,
  type ProjectUsage,
} from "~/server/services/project-usage";
import { listSessions } from "~/server/services/sessions";
import { applySync } from "~/server/services/sync";

import { type SyncFixture } from "../support/fixtures";
import { createUser, db, resetDb } from "./db";
import { AFTER_FIXTURE, K1, K2, request, requestFor, S1, S2 } from "./seed";

const scope = (userId: string) => loadAccessScope(db, userId);

const FIRST_MAC = "0e6f0b4c-2f7a-4e53-9d1b-6a2c7f9e1d35";
const SECOND_MAC = "22222222-2222-4333-8444-555555555555";
const BOBS_MAC = "11111111-2222-4333-8444-555555555555";
/** Ann's agentnotch on K2 (a project key is per account, so another key). */
const K2_KEY = "a".repeat(64);
/** Ann's agentnotch on K1 from her second Mac (another install secret, so another key). */
const SECOND_MAC_KEY = "9".repeat(64);
const OLD_KEY = "e".repeat(64);
const S3 = "c3c3c3c3-0000-4000-8000-000000000003";
const S4 = "c4c4c4c4-0000-4000-8000-000000000004";
const S5 = "c5c5c5c5-0000-4000-8000-000000000005";
const BOB_K1 = "b0b0b0b0-0000-4000-8000-000000000001";
const BOB_K2 = "b0b0b0b0-0000-4000-8000-000000000002";

/** The fixture's sessions, as tokens. */
const S1_TOKENS = 18234n + 96512n + 402118n + 12873120n;
const S2_TOKENS = 5120n + 8840n + 64000n + 910000n;

type SessionEdit = Partial<SyncFixture["sessions"][number]> & {
  sessionId: string;
  project: { key: string; name: string };
};

/** A batch of sessions on one account from one Mac, each built on the fixture's session there. */
function sessionsOn(
  deviceId: string,
  accountKey: string,
  sessions: SessionEdit[],
) {
  return request((r) => {
    r.device.id = deviceId;
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

const tokens = (input: number) => ({
  input,
  output: input * 2,
  cacheCreation: input * 3,
  cacheRead: input * 4,
});

/** A project in a report by name and whose it is. */
function find(projects: ProjectUsage[], name: string, owner: string) {
  const project = projects.find((p) => p.name === name && p.owner.id === owner);
  if (!project) throw new Error(`no ${owner}'s ${name}`);
  return project;
}

const summary = (projects: ProjectUsage[]) =>
  projects.map((p) => ({
    name: p.name,
    owner: p.owner.id,
    sessions: p.sessions,
    tokens: p.tokens.total,
    accounts: p.accounts.map((a) => [a.accountKey, a.sessions, a.tokens.total]),
  }));

async function rowIds(userId: string, accountKey: string, name: string) {
  const rows = await db.project.findMany({
    where: { userId, accountKey, name },
    select: { id: true },
    orderBy: { id: "asc" },
  });
  return rows.map((r) => r.id);
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

beforeEach(async () => {
  await resetDb();
  await createUser("ann", "Ann");
  await createUser("bob", "Bob");
  await createUser("dan", "Dan");

  // Ann's first Mac: S1 in agentnotch on K1 and S2 in billing-service on K2 (the fixture), both
  // on 2026-09-25; then agentnotch on K2 too, 16 days before AFTER_FIXTURE.
  await applySync(db, "ann", request(), AFTER_FIXTURE);
  await applySync(
    db,
    "ann",
    sessionsOn(FIRST_MAC, K2, [
      {
        sessionId: S3,
        project: { key: K2_KEY, name: "agentnotch" },
        startedAt: "2026-09-10T08:00:00Z",
        lastActivityAt: "2026-09-10T09:00:00Z",
        endedAt: "2026-09-10T09:01:00Z",
        tokens: tokens(1000),
        costUsd: 0.5,
      },
    ]),
    AFTER_FIXTURE,
  );
  // Her second Mac: agentnotch on K1 under another key, and a project last used in July.
  await applySync(
    db,
    "ann",
    sessionsOn(SECOND_MAC, K1, [
      {
        sessionId: S4,
        project: { key: SECOND_MAC_KEY, name: "agentnotch" },
        startedAt: "2026-09-24T08:00:00Z",
        lastActivityAt: "2026-09-24T08:30:00Z",
        endedAt: "2026-09-24T08:31:00Z",
        tokens: tokens(1),
        costUsd: 1.5,
      },
      {
        sessionId: S5,
        project: { key: OLD_KEY, name: "old-project" },
        startedAt: "2026-07-01T08:00:00Z",
        lastActivityAt: "2026-07-01T08:30:00Z",
        endedAt: "2026-07-01T08:31:00Z",
        tokens: tokens(10),
        costUsd: null,
      },
    ]),
    AFTER_FIXTURE,
  );
  // Bob has an agentnotch of his own on K1 and on K2. Ann shares K1 with him, not K2.
  for (const [accountKey, sessionId, input] of [
    [K1, BOB_K1, 100],
    [K2, BOB_K2, 7],
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

describe("usage by project", () => {
  it("add up a person's folders of one name across Macs and accounts, most tokens first", async () => {
    const report = await projectUsage(
      db,
      await scope("ann"),
      { period: "30d" },
      AFTER_FIXTURE,
    );
    expect(report.period).toBe("30d");
    expect(report.from).toEqual(new Date("2026-08-27T00:00:00Z"));
    expect(summary(report.projects)).toEqual([
      {
        name: "agentnotch",
        owner: "ann",
        sessions: 3,
        tokens: S1_TOKENS + 10n + 10_000n,
        accounts: [
          [K1, 2, S1_TOKENS + 10n],
          [K2, 1, 10_000n],
        ],
      },
      {
        name: "billing-service",
        owner: "ann",
        sessions: 1,
        tokens: S2_TOKENS,
        accounts: [[K2, 1, S2_TOKENS]],
      },
      // Bob's own agentnotch is another project, and only its K1 part is shared with Ann.
      {
        name: "agentnotch",
        owner: "bob",
        sessions: 1,
        tokens: 1000n,
        accounts: [[K1, 1, 1000n]],
      },
    ]);

    const anns = find(report.projects, "agentnotch", "ann");
    const ids = [
      ...(await rowIds("ann", K1, "agentnotch")),
      ...(await rowIds("ann", K2, "agentnotch")),
    ].sort();
    expect(ids).toHaveLength(3);
    expect(anns.projectIds).toEqual(ids);
    expect(anns.id).toBe(ids[0]);
    // Each account's part names the first of its rows there, for the account page's filter.
    expect(anns.accounts.map((a) => a.projectId)).toEqual([
      (await rowIds("ann", K1, "agentnotch"))[0],
      (await rowIds("ann", K2, "agentnotch"))[0],
    ]);
    expect(anns.macCount).toBe(2);
    expect(anns.costUsd).toBeCloseTo(14.82 + 1.5 + 0.5, 6);
    expect(anns.tokens).toEqual({
      input: 18234n + 1n + 1000n,
      output: 96512n + 2n + 2000n,
      cacheCreation: 402118n + 3n + 3000n,
      cacheRead: 12873120n + 4n + 4000n,
      total: S1_TOKENS + 10n + 10_000n,
    });
    expect(anns.lastUsedAt).toEqual(new Date("2026-09-25T09:47:03Z"));
    expect(anns.owner).toMatchObject({ isViewer: true });
    // No cost reported at all stays unknown, not zero.
    expect(find(report.projects, "billing-service", "ann").costUsd).toBeNull();

    expect(report.total).toEqual({
      projects: 3,
      sessions: 5,
      tokens: expect.objectContaining({
        total: S1_TOKENS + 10n + 10_000n + S2_TOKENS + 1000n,
      }) as unknown,
      costUsd: expect.closeTo(14.82 + 1.5 + 0.5 + 14.82, 6) as unknown,
    });
    expect(report.rest).toBeNull();
    // Nobody is ever shown a project key.
    expect(
      JSON.stringify(report, (_, v: unknown) =>
        typeof v === "bigint" ? `${v}` : v,
      ),
    ).not.toMatch(/"key"/);
  });

  it("count sessions by their start in the period", async () => {
    const annScope = await scope("ann");
    const week = await projectUsage(
      db,
      annScope,
      { period: "7d" },
      AFTER_FIXTURE,
    );
    expect(week.from).toEqual(new Date("2026-09-19T00:00:00Z"));
    // agentnotch's K2 session started 16 days back: outside the week, and so is K2.
    const anns = find(week.projects, "agentnotch", "ann");
    expect(anns.accounts.map((a) => a.accountKey)).toEqual([K1]);
    expect(anns.sessions).toBe(2);
    expect(anns.macCount).toBe(2);
    expect(anns.costUsd).toBeCloseTo(14.82 + 1.5, 6);
    expect(week.projects.map((p) => p.name)).not.toContain("old-project");

    const ever = await projectUsage(
      db,
      annScope,
      { period: "all" },
      AFTER_FIXTURE,
    );
    expect(ever.from).toBeNull();
    // The week leaves sessions out (the K2 one, the July one); all time leaves nothing out.
    expect(week.earlierSessions).toBe(true);
    expect(ever.earlierSessions).toBe(false);
    const old = find(ever.projects, "old-project", "ann");
    expect(old).toMatchObject({
      sessions: 1,
      macCount: 1,
      costUsd: null,
      lastUsedAt: new Date("2026-07-01T08:30:00Z"),
    });
    expect(old.tokens.total).toBe(100n);
    // The smallest comes last.
    expect(ever.projects.at(-1)!.name).toBe("old-project");
  });

  it("add up to the account's own totals, one account at a time", async () => {
    const annScope = await scope("ann");
    const accounts = await listAccounts(db, annScope, AFTER_FIXTURE);
    for (const [period, name] of [
      ["7d", "last7Days"],
      ["30d", "last30Days"],
    ] as const) {
      for (const account of accounts) {
        const report = await projectUsage(
          db,
          annScope,
          { period, accountKey: account.key },
          AFTER_FIXTURE,
        );
        expect(report.total.sessions).toBe(account[name].sessions);
        expect(report.total.tokens).toEqual(account[name].tokens);
        // Only this account's part of each project.
        for (const project of report.projects) {
          expect(project.accounts.map((a) => a.accountKey)).toEqual([
            account.key,
          ]);
        }
      }
    }
    const k1 = await projectUsage(
      db,
      annScope,
      { period: "30d", accountKey: K1 },
      AFTER_FIXTURE,
    );
    expect(summary(k1.projects)).toEqual([
      {
        name: "agentnotch",
        owner: "ann",
        sessions: 2,
        tokens: S1_TOKENS + 10n,
        accounts: [[K1, 2, S1_TOKENS + 10n]],
      },
      {
        name: "agentnotch",
        owner: "bob",
        sessions: 1,
        tokens: 1000n,
        accounts: [[K1, 1, 1000n]],
      },
    ]);
  });

  it("list the first projects and add up the rest", async () => {
    const report = await projectUsage(
      db,
      await scope("ann"),
      { period: "30d", limit: 1 },
      AFTER_FIXTURE,
    );
    expect(report.projects.map((p) => p.name)).toEqual(["agentnotch"]);
    expect(report.total.projects).toBe(3);
    expect(report.rest).toMatchObject({ projects: 2, sessions: 2 });
    expect(report.rest!.tokens.total).toBe(S2_TOKENS + 1000n);
    expect(report.rest!.costUsd).toBeCloseTo(14.82, 6);
    expect(report.total.tokens.total).toBe(
      report.projects[0]!.tokens.total + report.rest!.tokens.total,
    );
  });

  it("say whether a longer period would show more", async () => {
    const annScope = await scope("ann");
    // Two months on, the last week has nothing, and everything lies before it.
    const later = new Date("2026-11-26T00:00:00Z");
    const quiet = await projectUsage(db, annScope, { period: "7d" }, later);
    expect(quiet.projects).toEqual([]);
    expect(quiet.total).toMatchObject({ projects: 0, sessions: 0 });
    expect(quiet.earlierSessions).toBe(true);
    // On one account too, and only its own sessions count.
    const k2 = await projectUsage(
      db,
      annScope,
      { period: "30d", accountKey: K2 },
      later,
    );
    expect(k2.projects).toEqual([]);
    expect(k2.earlierSessions).toBe(true);
    // Someone with no sessions at all has nothing earlier either.
    const dan = await projectUsage(
      db,
      await scope("dan"),
      { period: "7d" },
      later,
    );
    expect(dan).toMatchObject({ projects: [], earlierSessions: false });
    // A project row left without sessions (a key change can leave one) is no earlier session.
    await db.session.deleteMany({ where: { userId: "bob" } });
    const bob = await projectUsage(
      db,
      await scope("bob"),
      { period: "7d" },
      later,
    );
    expect(bob.projects).toEqual([]);
    // Bob still sees Ann's K1 sessions through the pool, all before the week.
    expect(bob.earlierSessions).toBe(true);
    await db.session.deleteMany({ where: { userId: "ann", accountKey: K1 } });
    const bare = await projectUsage(
      db,
      await scope("bob"),
      { period: "7d" },
      later,
    );
    expect(await db.project.count({ where: { userId: "bob" } })).toBe(2);
    expect(bare).toMatchObject({ projects: [], earlierSessions: false });
  });

  it("show a pool member only what is shared with them", async () => {
    const report = await projectUsage(
      db,
      await scope("bob"),
      { period: "30d" },
      AFTER_FIXTURE,
    );
    expect(summary(report.projects)).toEqual([
      // Ann's agentnotch on K1 only: her K2 isn't shared, and billing-service is on K2.
      {
        name: "agentnotch",
        owner: "ann",
        sessions: 2,
        tokens: S1_TOKENS + 10n,
        accounts: [[K1, 2, S1_TOKENS + 10n]],
      },
      {
        name: "agentnotch",
        owner: "bob",
        sessions: 2,
        tokens: 1000n + 70n,
        accounts: [
          [K1, 1, 1000n],
          [K2, 1, 70n],
        ],
      },
    ]);
    expect(find(report.projects, "agentnotch", "ann").projectIds).toEqual(
      await rowIds("ann", K1, "agentnotch"),
    );
    // Someone who sees nothing gets nothing, and an account they can't see is "not found".
    const dan = await scope("dan");
    expect(
      (await projectUsage(db, dan, { period: "all" }, AFTER_FIXTURE)).projects,
    ).toEqual([]);
    expect(
      await denied(
        projectUsage(db, dan, { period: "all", accountKey: K1 }, AFTER_FIXTURE),
      ),
    ).toBe("NOT_FOUND");
  });

  it("count a session dated in the future as started now, and date nothing later", async () => {
    // At 09:00 on the 25th, S2 (started 11:00) lies ahead, and S1 is still going till 09:47.
    const NOW = new Date("2026-09-25T09:00:00Z");
    const report = await projectUsage(
      db,
      await scope("ann"),
      { period: "7d" },
      NOW,
    );
    const billing = find(report.projects, "billing-service", "ann");
    expect(billing.sessions).toBe(1);
    expect(billing.lastUsedAt).toEqual(NOW);
    expect(find(report.projects, "agentnotch", "ann").lastUsedAt).toEqual(NOW);
    const detail = await projectDetail(
      db,
      await scope("ann"),
      billing.id,
      "7d",
      NOW,
    );
    expect(detail.firstUsedAt).toEqual(NOW);
  });
});

describe("a project's usage by account", () => {
  it("find the whole group from any of its rows, every account listed", async () => {
    const annScope = await scope("ann");
    const ids = [
      ...(await rowIds("ann", K1, "agentnotch")),
      ...(await rowIds("ann", K2, "agentnotch")),
    ];
    for (const id of ids) {
      const detail = await projectDetail(db, annScope, id, "7d", AFTER_FIXTURE);
      expect(detail).toMatchObject({
        id: [...ids].sort()[0],
        projectIds: [...ids].sort(),
        name: "agentnotch",
        period: "7d",
        from: new Date("2026-09-19T00:00:00Z"),
        sessions: 2,
        macCount: 2,
        firstUsedAt: new Date("2026-09-10T08:00:00Z"),
        lastUsedAt: new Date("2026-09-25T09:47:03Z"),
      });
      // K2 had no session this week: still listed, last, with nothing.
      expect(
        detail.accounts.map((a) => [a.accountKey, a.sessions, a.tokens.total]),
      ).toEqual([
        [K1, 2, S1_TOKENS + 10n],
        [K2, 0, 0n],
      ]);
      expect(detail.accounts[1]).toMatchObject({
        costUsd: null,
        lastUsedAt: new Date("2026-09-10T09:00:00Z"),
      });
    }
    const ever = await projectDetail(
      db,
      annScope,
      ids[0]!,
      "all",
      AFTER_FIXTURE,
    );
    expect(ever.accounts.map((a) => [a.accountKey, a.sessions])).toEqual([
      [K1, 2],
      [K2, 1],
    ]);
    expect(ever.tokens.total).toBe(S1_TOKENS + 10n + 10_000n);
  });

  it("show a pool member the shared accounts' part, and nobody else anything", async () => {
    const [k1Row] = await rowIds("ann", K1, "agentnotch");
    const [k2Row] = await rowIds("ann", K2, "agentnotch");
    const bob = await scope("bob");
    const detail = await projectDetail(db, bob, k1Row!, "all", AFTER_FIXTURE);
    expect(detail.accounts.map((a) => a.accountKey)).toEqual([K1]);
    expect(detail.projectIds).toEqual(await rowIds("ann", K1, "agentnotch"));
    expect(detail.firstUsedAt).toEqual(new Date("2026-09-24T08:00:00Z"));
    expect(detail.owner).toMatchObject({
      displayName: "ann@example.com",
      isViewer: false,
    });
    expect(await canSeeProject(db, bob, k1Row!)).toBe(true);

    // Ann's K2 row isn't Bob's to see, nor is any of hers Dan's.
    expect(await canSeeProject(db, bob, k2Row!)).toBe(false);
    expect(
      await denied(projectDetail(db, bob, k2Row!, "all", AFTER_FIXTURE)),
    ).toBe("NOT_FOUND");
    const dan = await scope("dan");
    expect(await canSeeProject(db, dan, k1Row!)).toBe(false);
    expect(
      await denied(projectDetail(db, dan, k1Row!, "7d", AFTER_FIXTURE)),
    ).toBe("NOT_FOUND");
    expect(await canSeeProject(db, dan, "no-such-project")).toBe(false);
  });
});

describe("a project's sessions across accounts", () => {
  const ids = (items: Array<{ sessionId: string }>) =>
    items.map((s) => s.sessionId).sort();

  it("span every account the viewer sees the project on", async () => {
    const [k1Row] = await rowIds("ann", K1, "agentnotch");
    const [k2Row] = await rowIds("ann", K2, "agentnotch");
    const ann = await scope("ann");
    for (const projectId of [k1Row!, k2Row!]) {
      const across = await listSessions(db, ann, {
        projectId,
        acrossAccounts: true,
        limit: 50,
      });
      expect(ids(across.items)).toEqual([S1, S3, S4].sort());
    }
    // Without it, the project's group stays on the row's own account.
    const k1Only = await listSessions(db, ann, {
      projectId: k1Row!,
      limit: 50,
    });
    expect(ids(k1Only.items)).toEqual([S1, S4].sort());
    expect(ids(k1Only.items)).not.toContain(S2);

    const bob = await scope("bob");
    const shared = await listSessions(db, bob, {
      projectId: k1Row!,
      acrossAccounts: true,
      limit: 50,
    });
    expect(ids(shared.items)).toEqual([S1, S4].sort());
    // A row the viewer can't see selects nothing, even where the same name is shared.
    const hidden = await listSessions(db, bob, {
      projectId: k2Row!,
      acrossAccounts: true,
      limit: 50,
    });
    expect(hidden.items).toEqual([]);
  });
});
