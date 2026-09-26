/**
 * The raw SQL pieces (src/server/services/sql.ts): visibility spelled exactly as `ownedRowWhere`
 * spells it for Prisma, times clamped to the server's clock without turning NULL into now, and
 * search terms taken literally. tests/integration runs them on Postgres.
 */
import { describe, expect, it } from "vitest";

import { Prisma } from "~/server/db-types";
import { buildAccessScope, ownedRowWhere } from "~/server/services/access";
import {
  clampedTime,
  containsPattern,
  ownedRowSql,
  sqlTime,
} from "~/server/services/sql";

const K1 = "1".repeat(64);
const K2 = "2".repeat(64);

describe("ownedRowSql", () => {
  const pools = [
    { poolId: "p1", accountKey: K1, memberIds: ["ann", "bob"] },
    { poolId: "p2", accountKey: K2, memberIds: ["bob", "cat", "dan"] },
  ];

  it("matches the viewer's own rows, and each pooled account's members' rows", () => {
    const where = ownedRowWhere(buildAccessScope("bob", [K1], pools));
    const sql = ownedRowSql(where, "s");
    expect(sql.text).toBe(
      '(s."userId" = $1 OR (s."accountKey" = $2 AND s."userId" IN ($3,$4)) OR (s."accountKey" = $5 AND s."userId" IN ($6,$7,$8)))',
    );
    expect(sql.values).toEqual([
      "bob",
      K1,
      "ann",
      "bob",
      K2,
      "bob",
      "cat",
      "dan",
    ]);
  });

  it("narrows to one account, on the table alias it is given", () => {
    const where = ownedRowWhere(buildAccessScope("dan", [], pools), {
      accountKey: K2,
    });
    const sql = ownedRowSql(where, "p");
    expect(sql.text).toBe('(p."accountKey" = $1 AND p."userId" IN ($2,$3,$4))');
    expect(sql.values).toEqual([K2, "bob", "cat", "dan"]);
  });

  it("matches only the viewer's rows when they are in no pool", () => {
    const sql = ownedRowSql(
      ownedRowWhere(buildAccessScope("eve", [], [])),
      "s",
    );
    expect(sql.text).toBe('(s."userId" = $1)');
    expect(sql.values).toEqual(["eve"]);
  });

  it("matches nothing for an empty owner list", () => {
    const sql = ownedRowSql({ accountKey: K1, userId: { in: [] } }, "s");
    expect(sql.text).toBe("FALSE");
  });
});

describe("times", () => {
  const now = new Date("2026-09-25T12:00:00.000Z");

  it("passes a moment as UTC text", () => {
    const sql = sqlTime(now);
    expect(sql.text).toBe("($1::timestamptz AT TIME ZONE 'UTC')");
    expect(sql.values).toEqual(["2026-09-25T12:00:00.000Z"]);
  });

  it("clamps a nullable column without turning NULL into now", () => {
    // LEAST ignores NULLs: on its own it would make every running session "ended now".
    const sql = clampedTime(Prisma.sql`s."endedAt"`, sqlTime(now));
    expect(sql.text).toBe(
      `CASE WHEN s."endedAt" IS NULL THEN NULL ELSE LEAST(s."endedAt", ($1::timestamptz AT TIME ZONE 'UTC')) END`,
    );
  });
});

describe("containsPattern", () => {
  it("finds the term anywhere, taking wildcards and backslashes literally", () => {
    expect(containsPattern("agent")).toBe("%agent%");
    expect(containsPattern("50%_off\\")).toBe("%50\\%\\_off\\\\%");
  });
});
