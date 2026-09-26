/**
 * Raw SQL pieces for the reads Prisma's query builder can't express: times clamped to the
 * server's clock, so data dated in the future (a Mac whose clock runs ahead) counts as now and
 * never later, and the visibility rule of access.ts spelled as SQL.
 */
import { Prisma } from "~/server/db-types";
import { type OwnedRowWhere } from "~/server/services/access";

/**
 * A moment as the database compares it with the `timestamp(3)` columns, which hold UTC. Passed as
 * an ISO string, so the session's time zone never matters.
 */
export function sqlTime(date: Date): Prisma.Sql {
  return Prisma.sql`(${date.toISOString()}::timestamptz AT TIME ZONE 'UTC')`;
}

/** A timestamp column no later than `now`. NULL stays NULL (LEAST alone would turn it into now). */
export function clampedTime(column: Prisma.Sql, now: Prisma.Sql): Prisma.Sql {
  return Prisma.sql`CASE WHEN ${column} IS NULL THEN NULL ELSE LEAST(${column}, ${now}) END`;
}

/** The table aliases the raw queries use. A fixed set, so an alias is never user input. */
export type SqlAlias = "s" | "p";

/**
 * The SQL twin of `ownedRowWhere` (access.ts): it matches exactly the rows that `where` matches,
 * on the table aliased `alias`. Built from that `where`, so the two can't drift apart.
 */
export function ownedRowSql(where: OwnedRowWhere, alias: SqlAlias): Prisma.Sql {
  const column = (name: "userId" | "accountKey") =>
    Prisma.raw(`${alias}."${name}"`);
  const onAccount = (clause: {
    accountKey: string;
    userId: { in: string[] };
  }) =>
    clause.userId.in.length === 0
      ? Prisma.sql`FALSE`
      : Prisma.sql`(${column("accountKey")} = ${clause.accountKey} AND ${column("userId")} IN (${Prisma.join(clause.userId.in)}))`;
  if (!("OR" in where)) return onAccount(where);
  const clauses = where.OR.map((clause) =>
    "accountKey" in clause
      ? onAccount(clause)
      : Prisma.sql`${column("userId")} = ${clause.userId}`,
  );
  return clauses.length === 0
    ? Prisma.sql`FALSE`
    : Prisma.sql`(${Prisma.join(clauses, " OR ")})`;
}

/**
 * An ILIKE pattern that finds `term` anywhere, taking it literally: `%`, `_` and the escape
 * character itself (Postgres' default, a backslash) are escaped.
 */
export function containsPattern(term: string): string {
  return `%${term.replace(/[\\%_]/g, (c) => `\\${c}`)}%`;
}
