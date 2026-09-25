/**
 * The tRPC routers over the real services: sign-in is required, access refusals become the
 * matching tRPC codes, and outputs survive superjson (BigInt, Date).
 */
import { TRPCError } from "@trpc/server";
import superjson from "superjson";
import { beforeEach, describe, expect, it } from "vitest";

import { createCaller } from "~/server/api/root";
import { type TRPCContext } from "~/server/api/trpc";
import { applySync } from "~/server/services/sync";

import { keysFixture } from "../support/fixtures";
import { createUser, db, resetDb } from "./db";
import { AFTER_FIXTURE, K1, K2, request, S1, S2 } from "./seed";

function caller(
  viewer: { id: string; email: string; name: string | null } | null,
) {
  const ctx: TRPCContext = {
    db,
    viewer: viewer ? { ...viewer, via: "cookie" } : null,
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
      anon.sessions.list({}),
      anon.sessions.get({ id: "x" }),
      anon.projects.list({ accountKey: K1 }),
      anon.projects.get({ id: "x" }),
      anon.usage.history({ accountKey: K1 }),
      anon.pools.list(),
      anon.pools.create({ accountKey: K1 }),
      anon.pools.join({ code: "7K3M9QX2H4TB" }),
      anon.pools.revoke({ poolId: "x" }),
      anon.pools.leave({ poolId: "x" }),
      anon.pools.removeMember({ poolId: "x", userId: "y" }),
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
        await api.sessions.list({}),
        await api.accounts.list(),
        await api.accounts.get({ accountKey: K1 }),
        await api.pools.list(),
        await api.usage.history({ accountKey: K1 }),
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

  it("produce outputs that survive superjson", async () => {
    const accounts = await caller(ann).accounts.list();
    const back = superjson.parse<typeof accounts>(
      superjson.stringify(accounts),
    );
    expect(back).toEqual(accounts);
    const k1 = back.find((a) => a.key === K1)!;
    expect(typeof k1.last30Days.tokens.total).toBe("bigint");
    expect(k1.lastActivityAt).toBeInstanceOf(Date);
  });
});
