/**
 * The raw SQL pieces (src/server/services/sql.ts): visibility spelled exactly as `ownedRowWhere`
 * spells it for Prisma, times clamped to the server's clock without turning NULL into now, the
 * copy of a Claude session that counts, and search terms taken literally. tests/integration runs
 * them on Postgres (session-copies.test.ts for the copies).
 */
import { describe, expect, it } from "vitest";

import { Prisma } from "~/server/db-types";
import { buildAccessScope, ownedRowWhere } from "~/server/services/access";
import {
  canonicalSessionSql,
  clampedTime,
  containsPattern,
  ownedRowSql,
  sessionSummarySql,
  sharedAccounts,
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

describe("canonicalSessionSql", () => {
  const flat = (sql: Prisma.Sql) => sql.text.replace(/\s+/g, " ");

  it("is plain TRUE for a viewer who shares no account with anyone", () => {
    expect(
      canonicalSessionSql(buildAccessScope("eve", [K1], []), "s").text,
    ).toBe("TRUE");
    // A pool nobody else joined shares nothing either.
    const alone = buildAccessScope(
      "ann",
      [K1],
      [{ poolId: "p1", accountKey: K1, memberIds: ["ann"] }],
    );
    expect(canonicalSessionSql(alone, "p").text).toBe("TRUE");
  });

  it("checks only the shared accounts, against the copies their members hold", () => {
    const scope = buildAccessScope(
      "bob",
      [K1],
      [
        { poolId: "p2", accountKey: K2, memberIds: ["dan", "bob", "cat"] },
        { poolId: "p1", accountKey: K1, memberIds: ["ann", "bob"] },
        { poolId: "p3", accountKey: "3".repeat(64), memberIds: ["bob"] },
      ],
    );
    const sql = canonicalSessionSql(scope, "s");
    expect(flat(sql)).toBe(
      '(s."accountKey" NOT IN ($1,$2) OR NOT EXISTS ( SELECT 1 FROM "Session" o ' +
        'WHERE o."accountKey" = s."accountKey" AND o."sessionId" = s."sessionId" ' +
        'AND o."userId" <> s."userId" ' +
        'AND ((o."accountKey" = $3 AND o."userId" IN ($4,$5)) OR (o."accountKey" = $6 AND o."userId" IN ($7,$8,$9))) ' +
        'AND (o."lastActivityAt", o."endedAt" IS NOT NULL, o."messageCount", o."updatedAt", s."userId") ' +
        '> (s."lastActivityAt", s."endedAt" IS NOT NULL, s."messageCount", s."updatedAt", o."userId")))',
    );
    // Sorted, so the same scope always gives the same statement.
    expect(sql.values).toEqual([
      K1,
      K2,
      K1,
      "ann",
      "bob",
      K2,
      "bob",
      "cat",
      "dan",
    ]);
  });
});

describe("sessionSummarySql", () => {
  const flat = (sql: Prisma.Sql) => sql.text.replace(/\s+/g, " ").trim();

  it("is the row's own summary, with no join, when nothing is shared", () => {
    const summary = sessionSummarySql(buildAccessScope("eve", [K1], []), "s");
    expect(summary.join.text).toBe("");
    expect(summary.text.text).toBe('s."summaryText"');
    expect(summary.model.text).toBe('s."summaryModel"');
    expect(summary.at.text).toBe('s."summaryAt"');
  });

  it("borrows the latest summary of another visible copy when the row has none", () => {
    const scope = buildAccessScope(
      "ann",
      [K1],
      [{ poolId: "p1", accountKey: K1, memberIds: ["ann", "bob"] }],
    );
    const summary = sessionSummarySql(scope, "s");
    expect(flat(summary.join)).toBe(
      'LEFT JOIN LATERAL ( SELECT x."summaryText", x."summaryModel", x."summaryAt" ' +
        'FROM "Session" x WHERE s."summaryText" IS NULL AND s."accountKey" IN ($1) ' +
        'AND x."accountKey" = s."accountKey" AND x."sessionId" = s."sessionId" ' +
        'AND x."userId" <> s."userId" AND ((x."accountKey" = $2 AND x."userId" IN ($3,$4))) ' +
        'AND x."summaryText" IS NOT NULL ORDER BY x."summaryAt" DESC NULLS LAST, x."id" LIMIT 1 ) sb ON TRUE',
    );
    expect(summary.join.values).toEqual([K1, K1, "ann", "bob"]);
    expect(summary.text.text).toBe(
      'CASE WHEN sb."summaryText" IS NULL THEN s."summaryText" ELSE sb."summaryText" END',
    );
  });
});

describe("sharedAccounts", () => {
  it("lists only accounts shared with someone else, sorted, with their members", () => {
    const scope = buildAccessScope(
      "bob",
      [],
      [
        { poolId: "p2", accountKey: K2, memberIds: ["dan", "bob"] },
        { poolId: "p1", accountKey: K1, memberIds: ["bob"] },
      ],
    );
    expect(sharedAccounts(scope)).toEqual([
      { accountKey: K2, userId: { in: ["bob", "dan"] } },
    ]);
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
