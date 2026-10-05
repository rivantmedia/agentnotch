/**
 * The tRPC routers over the real services: sign-in is required, access refusals become the
 * matching tRPC codes, and outputs survive superjson (BigInt, Date).
 */
import { TRPCError } from "@trpc/server";
import superjson from "superjson";
import { beforeEach, describe, expect, it } from "vitest";

import {
  DELETE_DATA_CONFIRMATION,
  REMOVE_SUMMARIES_CONFIRMATION,
} from "~/lib/data-controls";
import { createCaller } from "~/server/api/root";
import { type TRPCContext } from "~/server/api/trpc";
import { ipKey } from "~/server/client-ip";
import { applySync } from "~/server/services/sync";

import { keysFixture } from "../support/fixtures";
import { createUser, db, resetDb } from "./db";
import { AFTER_FIXTURE, K1, K2, request, S1, S2 } from "./seed";

function caller(
  viewer: { id: string; email: string; name: string | null } | null,
  headers: Record<string, string> = {},
) {
  const ctx: TRPCContext = {
    db,
    viewer: viewer ? { ...viewer, via: "cookie" } : null,
    headers: new Headers(headers),
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

const ann = { id: "ann", email: "ann@example.com", name: "Ann" };
const bob = { id: "bob", email: "bob@example.com", name: "Bob" };

beforeEach(async () => {
  await resetDb();
  await createUser("ann", "Ann");
  await createUser("bob", "Bob");
  await applySync(db, "ann", request(), AFTER_FIXTURE);
});

describe("tRPC routers", () => {
  it("refuse anonymous callers on every protected procedure", async () => {
    const anon = caller(null);
    const calls: Array<Promise<unknown>> = [
      anon.viewer.me(),
      anon.accounts.list(),
      anon.accounts.get({ accountKey: K1 }),
      anon.accounts.visible({ accountKey: K1 }),
      anon.sessions.list({}),
      anon.sessions.get({ id: "x" }),
      anon.projects.list({ accountKey: K1 }),
      anon.projects.get({ id: "x" }),
      anon.projects.usage({ period: "7d" }),
      anon.projects.detail({ id: "x", period: "7d" }),
      anon.projects.visible({ id: "x" }),
      anon.usage.history({ accountKey: K1 }),
      anon.usage.combined({ period: "7d" }),
      anon.usage.timeline({ period: "7d", timeZone: "UTC" }),
      anon.pools.list(),
      anon.pools.create({ accountKey: K1 }),
      anon.pools.join({ code: "7K3M9QX2H4TB" }),
      anon.pools.revoke({ poolId: "x" }),
      anon.pools.leave({ poolId: "x" }),
      anon.pools.removeMember({ poolId: "x", userId: "y" }),
      anon.myData.removeSummaries({ confirm: REMOVE_SUMMARIES_CONFIRMATION }),
      anon.myData.deleteAll({ confirm: DELETE_DATA_CONFIRMATION }),
    ];
    for (const call of calls) expect(await trpcCode(call)).toBe("UNAUTHORIZED");
    expect(await anon.viewer.current()).toBeNull();
  });

  it("serve the viewer's own data", async () => {
    const api = caller(ann);
    const me = await api.viewer.me();
    expect(me).toMatchObject({
      id: "ann",
      displayName: "Ann",
      memberLabel: { displayName: "ann@example.com", name: "Ann" },
    });
    expect(me.devices.map((d) => d.name)).toEqual(["Studio MacBook Pro"]);
    expect(await api.viewer.current()).toMatchObject({
      id: "ann",
      displayName: "Ann",
    });

    const accounts = await api.accounts.list();
    expect(accounts.map((a) => a.key).sort()).toEqual([K1, K2].sort());

    const page = await api.sessions.list({ accountKey: K1 });
    expect(page.items.map((s) => s.sessionId)).toEqual([S1]);
    expect(
      (await api.sessions.list({ running: true })).items.map(
        (s) => s.sessionId,
      ),
    ).toEqual([S2]);
    expect(
      (await api.sessions.list({ source: "cli" })).items.map(
        (s) => s.sessionId,
      ),
    ).toEqual([S2]);
    const session = await api.sessions.get({ id: page.items[0]!.id });
    expect(session.tokens.cacheRead).toBe(12873120n);

    const projects = await api.projects.list({ accountKey: K1 });
    expect(projects.map((p) => p.name)).toEqual(["agentnotch"]);
    expect((await api.projects.get({ id: projects[0]!.id })).sessionCount).toBe(
      1,
    );
    // Usage by project across both accounts, and one project's by account.
    const usage = await api.projects.usage({ period: "all" });
    expect(usage.projects.map((p) => [p.name, p.accounts.length])).toEqual([
      ["agentnotch", 1],
      ["billing-service", 1],
    ]);
    expect(
      (
        await api.projects.usage({ period: "all", accountKey: K2 })
      ).projects.map((p) => p.name),
    ).toEqual(["billing-service"]);
    expect(
      (await api.projects.usage({ period: "all", limit: 1 })).rest,
    ).toMatchObject({ projects: 1, sessions: 1 });
    const detail = await api.projects.detail({
      id: projects[0]!.id,
      period: "all",
    });
    expect(detail.accounts.map((a) => a.accountKey)).toEqual([K1]);
    expect(await api.projects.visible({ id: projects[0]!.id })).toBe(true);
    expect(
      (
        await api.sessions.list({
          projectId: projects[0]!.id,
          acrossAccounts: true,
        })
      ).items.map((s) => s.sessionId),
    ).toEqual([S1]);

    const history = await api.usage.history({
      accountKey: K1,
      from: new Date("2026-09-24T00:00:00Z"),
      to: AFTER_FIXTURE,
    });
    expect(history.windows.map((w) => w.windowId)).toEqual([
      "session",
      "weekly_all",
      "weekly_opus",
    ]);

    // Usage across accounts: both, or the ones named.
    const combined = await api.usage.combined({ period: "all" });
    expect(combined.accountKeys).toEqual([K1, K2].sort());
    expect(combined.total.sessions).toBe(2);
    expect(
      (await api.usage.combined({ period: "all", accountKeys: [K2] }))
        .accountKeys,
    ).toEqual([K2]);
    const timeline = await api.usage.timeline({
      period: "30d",
      accountKeys: [K1],
      timeZone: "Europe/Berlin",
    });
    expect(timeline).toMatchObject({ timeZone: "Europe/Berlin", unit: "day" });
    expect(timeline.buckets.reduce((n, b) => n + b.sessions, 0)).toBe(1);
    expect(
      (
        await api.projects.usage({ period: "all", accountKeys: [K2] })
      ).projects.map((p) => p.name),
    ).toEqual(["billing-service"]);
  });

  it("turn access refusals into NOT_FOUND / FORBIDDEN / BAD_REQUEST", async () => {
    const api = caller(bob);
    const annS1 = await db.session.findUniqueOrThrow({
      where: {
        userId_accountKey_sessionId: {
          userId: "ann",
          accountKey: K1,
          sessionId: S1,
        },
      },
    });
    expect(await trpcCode(api.sessions.get({ id: annS1.id }))).toBe(
      "NOT_FOUND",
    );
    expect(await trpcCode(api.sessions.list({ accountKey: K1 }))).toBe(
      "NOT_FOUND",
    );
    expect(await trpcCode(api.accounts.get({ accountKey: K1 }))).toBe(
      "NOT_FOUND",
    );
    expect(await trpcCode(api.projects.list({ accountKey: K1 }))).toBe(
      "NOT_FOUND",
    );
    expect(
      await trpcCode(api.projects.usage({ period: "7d", accountKey: K1 })),
    ).toBe("NOT_FOUND");
    const annProject = await db.project.findFirstOrThrow({
      where: { userId: "ann" },
    });
    expect(
      await trpcCode(api.projects.detail({ id: annProject.id, period: "7d" })),
    ).toBe("NOT_FOUND");
    expect(await api.projects.visible({ id: annProject.id })).toBe(false);
    // Bob's own view of usage by project has nothing of Ann's in it.
    expect((await api.projects.usage({ period: "all" })).projects).toEqual([]);
    expect(await trpcCode(api.usage.history({ accountKey: K1 }))).toBe(
      "NOT_FOUND",
    );
    expect(await trpcCode(api.pools.create({ accountKey: K1 }))).toBe(
      "FORBIDDEN",
    );
    expect(await trpcCode(api.pools.join({ code: "nope" }))).toBe(
      "BAD_REQUEST",
    );
    expect(await trpcCode(api.pools.join({ code: "ZZZZ-ZZZZ" }))).toBe(
      "BAD_REQUEST", // the old 8-character form
    );
    expect(await trpcCode(api.pools.join({ code: "ZZZZ-ZZZZ-ZZZZ" }))).toBe(
      "NOT_FOUND",
    );
  });

  it("answer TOO_MANY_REQUESTS once a user has guessed too many codes", async () => {
    const api = caller(bob);
    for (let i = 0; i < 10; i++) {
      expect(await trpcCode(api.pools.join({ code: "ZZZZ-ZZZZ-ZZZZ" }))).toBe(
        "NOT_FOUND",
      );
    }
    const { code } = await caller(ann).pools.create({ accountKey: K1 });
    expect(await trpcCode(api.pools.join({ code }))).toBe("TOO_MANY_REQUESTS");
  });

  it("say which accounts the viewer may see, for the account page's 404", async () => {
    expect(await caller(ann).accounts.visible({ accountKey: K1 })).toBe(true);
    expect(await caller(bob).accounts.visible({ accountKey: K1 })).toBe(false);
    expect(
      await caller(ann).accounts.visible({ accountKey: "e".repeat(64) }),
    ).toBe(false);
    const { code } = await caller(ann).pools.create({ accountKey: K1 });
    await caller(bob).pools.join({ code });
    expect(await caller(bob).accounts.visible({ accountKey: K1 })).toBe(true);
    expect(await caller(bob).accounts.visible({ accountKey: K2 })).toBe(false);
  });

  it("count failed codes against the client's address, stored as a hash", async () => {
    const fromOffice = caller(bob, {
      "x-forwarded-for": "10.0.0.1, 203.0.113.7",
    });
    expect(
      await trpcCode(fromOffice.pools.join({ code: "ZZZZ-ZZZZ-ZZZZ" })),
    ).toBe("NOT_FOUND");
    // No address known: only the per-user count.
    expect(
      await trpcCode(caller(bob).pools.join({ code: "ZZZZ-ZZZZ-ZZZZ" })),
    ).toBe("NOT_FOUND");
    expect(await db.poolJoinFailure.count({ where: { userId: "bob" } })).toBe(
      2,
    );
    const rows = await db.poolJoinIpFailure.findMany();
    expect(rows.map((r) => r.ipKey)).toEqual([ipKey("203.0.113.7", undefined)]);
  });

  it("refuse NUL in text inputs as bad input", async () => {
    const api = caller(ann);
    expect(await trpcCode(api.sessions.list({ search: "a\u0000b" }))).toBe(
      "BAD_REQUEST",
    );
    expect(await trpcCode(api.pools.revoke({ poolId: "\u0000" }))).toBe(
      "BAD_REQUEST",
    );
  });

  it("never return a project key, to anyone", async () => {
    const { code } = await caller(ann).pools.create({ accountKey: K1 });
    await caller(bob).pools.join({ code });
    const projectKeys = keysFixture().projects.map((p) => p.key);
    for (const viewer of [ann, bob]) {
      const api = caller(viewer);
      const projects = await api.projects.list({ accountKey: K1 });
      const outputs: unknown[] = [
        projects,
        await api.projects.get({ id: projects[0]!.id }),
        await api.projects.usage({ period: "all" }),
        await api.projects.usage({ period: "all", accountKey: K1 }),
        await api.projects.detail({ id: projects[0]!.id, period: "all" }),
        await api.sessions.list({}),
        await api.accounts.list(),
        await api.accounts.get({ accountKey: K1 }),
        await api.pools.list(),
        await api.usage.history({ accountKey: K1 }),
        await api.usage.combined({ period: "all" }),
        await api.usage.timeline({ period: "all", timeZone: "UTC" }),
        await api.projects.usage({ period: "all", accountKeys: [K1, K2] }),
      ];
      const text = superjson.stringify(outputs);
      for (const key of projectKeys) expect(text).not.toContain(key);
    }
  });

  it("validate inputs", async () => {
    const api = caller(ann);
    expect(await trpcCode(api.accounts.get({ accountKey: "not-a-key" }))).toBe(
      "BAD_REQUEST",
    );
    expect(
      await trpcCode(
        api.usage.history({
          accountKey: K1,
          from: new Date("2026-09-25T00:00:00Z"),
          to: new Date("2026-09-24T00:00:00Z"),
        }),
      ),
    ).toBe("BAD_REQUEST");
    expect(await trpcCode(api.sessions.list({ limit: 1000 }))).toBe(
      "BAD_REQUEST",
    );
    for (const period of ["90d", "", "ALL"]) {
      expect(
        await trpcCode(api.projects.usage({ period: period as "7d" })),
      ).toBe("BAD_REQUEST");
    }
    expect(await trpcCode(api.projects.usage({ period: "7d", limit: 0 }))).toBe(
      "BAD_REQUEST",
    );
    expect(
      await trpcCode(api.projects.usage({ period: "7d", limit: 101 })),
    ).toBe("BAD_REQUEST");
    expect(
      await trpcCode(
        api.projects.usage({ period: "7d", accountKey: "not-a-key" }),
      ),
    ).toBe("BAD_REQUEST");
    expect(
      await trpcCode(api.projects.detail({ id: "\u0000", period: "7d" })),
    ).toBe("BAD_REQUEST");
    // A selection names at least one account and at most a selection's worth, each a key, and
    // usage by project takes one account or several, never both.
    const many = Array.from({ length: 101 }, (_, i) =>
      i.toString(16).padStart(64, "0"),
    );
    for (const accountKeys of [[], ["not-a-key"], many]) {
      expect(
        await trpcCode(api.usage.combined({ period: "7d", accountKeys })),
      ).toBe("BAD_REQUEST");
      expect(
        await trpcCode(api.projects.usage({ period: "7d", accountKeys })),
      ).toBe("BAD_REQUEST");
    }
    expect(
      await trpcCode(
        api.projects.usage({ period: "7d", accountKey: K1, accountKeys: [K1] }),
      ),
    ).toBe("BAD_REQUEST");
    for (const timeZone of ["", "x".repeat(65), "Europe/\u0000"]) {
      expect(
        await trpcCode(api.usage.timeline({ period: "7d", timeZone })),
      ).toBe("BAD_REQUEST");
    }
  });

  it("run the pool flow and share sessions through it", async () => {
    const { code, poolId } = await caller(ann).pools.create({ accountKey: K1 });
    expect(await caller(bob).pools.join({ code })).toMatchObject({
      joined: true,
      poolId,
    });

    const bobsView = await caller(bob).sessions.list({ accountKey: K1 });
    expect(
      bobsView.items.map((s) => [
        s.sessionId,
        s.owner.displayName,
        s.owner.name,
      ]),
    ).toEqual([[S1, "ann@example.com", "Ann"]]);
    const [pool] = await caller(bob).pools.list();
    expect(pool).toMatchObject({ id: poolId, role: "member", share: null });

    expect(await trpcCode(caller(bob).pools.revoke({ poolId }))).toBe(
      "FORBIDDEN",
    );
    expect(
      await caller(ann).pools.removeMember({ poolId, userId: "bob" }),
    ).toEqual({
      removed: true,
      codeRevoked: true,
    });
    expect(await trpcCode(caller(bob).sessions.list({ accountKey: K1 }))).toBe(
      "NOT_FOUND",
    );
    // Removing Bob already revoked the code.
    expect(await caller(ann).pools.revoke({ poolId })).toEqual({
      revoked: false,
    });
    expect(await trpcCode(caller(bob).pools.join({ code }))).toBe("NOT_FOUND");
    expect(await caller(ann).pools.leave({ poolId })).toEqual({
      deletedPool: true,
    });
  });

  it("list a project's sessions across accounts, and only rows the viewer sees", async () => {
    // Ann works in agentnotch on K2 too.
    const S3 = "c3c3c3c3-0000-4000-8000-000000000003";
    await applySync(
      db,
      "ann",
      request((r) => {
        r.accounts = r.accounts.filter((a) => a.key === K2);
        const base = r.sessions.find((s) => s.accountKey === K2)!;
        const session = {
          ...base,
          sessionId: S3,
          project: { key: "a".repeat(64), name: "agentnotch" },
        };
        delete session.summary;
        r.sessions = [session];
        r.usage = [];
      }),
      AFTER_FIXTURE,
    );
    const row = async (accountKey: string) =>
      (
        await db.project.findFirstOrThrow({
          where: { userId: "ann", accountKey, name: "agentnotch" },
        })
      ).id;
    const [k1Row, k2Row] = [await row(K1), await row(K2)];
    const sessionIds = async (
      viewer: typeof ann,
      input: { projectId: string; acrossAccounts?: boolean },
    ) =>
      (await caller(viewer).sessions.list(input)).items
        .map((s) => s.sessionId)
        .sort();

    expect(
      await sessionIds(ann, { projectId: k1Row, acrossAccounts: true }),
    ).toEqual([S1, S3].sort());
    expect(await sessionIds(ann, { projectId: k1Row })).toEqual([S1]);
    const detail = await caller(ann).projects.detail({
      id: k2Row,
      period: "all",
    });
    expect(detail.accounts.map((a) => a.accountKey).sort()).toEqual(
      [K1, K2].sort(),
    );

    // Bob, pooled on K1 only, sees the K1 part, and nothing through the K2 row he can't see.
    const { code } = await caller(ann).pools.create({ accountKey: K1 });
    await caller(bob).pools.join({ code });
    expect(
      await sessionIds(bob, { projectId: k1Row, acrossAccounts: true }),
    ).toEqual([S1]);
    expect(
      await sessionIds(bob, { projectId: k2Row, acrossAccounts: true }),
    ).toEqual([]);
    expect(
      await trpcCode(caller(bob).projects.detail({ id: k2Row, period: "all" })),
    ).toBe("NOT_FOUND");
    expect(await caller(bob).projects.visible({ id: k2Row })).toBe(false);
  });

  it("produce outputs that survive superjson", async () => {
    const accounts = await caller(ann).accounts.list();
    const back = superjson.parse<typeof accounts>(
      superjson.stringify(accounts),
    );
    expect(back).toEqual(accounts);
    const k1 = back.find((a) => a.key === K1)!;
    expect(typeof k1.last30Days.tokens.total).toBe("bigint");
    expect(k1.lastActivityAt).toBeInstanceOf(Date);

    const combined = await caller(ann).usage.combined({ period: "all" });
    const combinedBack = superjson.parse<typeof combined>(
      superjson.stringify(combined),
    );
    expect(combinedBack).toEqual(combined);
    expect(typeof combinedBack.total.tokens.total).toBe("bigint");
    const timeline = await caller(ann).usage.timeline({
      period: "7d",
      timeZone: "UTC",
    });
    const timelineBack = superjson.parse<typeof timeline>(
      superjson.stringify(timeline),
    );
    expect(timelineBack).toEqual(timeline);
    expect(timelineBack.buckets[0]!.start).toBeInstanceOf(Date);

    const usage = await caller(ann).projects.usage({ period: "all" });
    const usageBack = superjson.parse<typeof usage>(superjson.stringify(usage));
    expect(usageBack).toEqual(usage);
    expect(typeof usageBack.total.tokens.total).toBe("bigint");
    expect(usageBack.projects[0]!.accounts[0]!.lastUsedAt).toBeInstanceOf(Date);
  });
});
