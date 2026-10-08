/**
 * Raw SQL pieces for the reads Prisma's query builder can't express: times clamped to the
 * server's clock, so data dated in the future (a Mac whose clock runs ahead) counts as now and
 * never later, the visibility rule of access.ts spelled as SQL, and which of the copies of one
 * Claude session a viewer sees is the one that counts.
 */
import { Prisma } from "~/server/db-types";
import { type AccessScope, type OwnedRowWhere } from "~/server/services/access";

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
export type SqlAlias = "s" | "p" | "c" | "o" | "x";

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
 * Whether the Session row aliased `alias` is the copy that counts of its Claude session, among
 * the Session rows the viewer sees (contract/README.md, "Pooling"). Callers AND it with the
 * row's own visibility (`ownedRowSql`).
 *
 * Why there can be copies: what a Mac has sent is remembered per website user, so a Mac signed
 * in as one person and later as another sends its whole session ledger again, and two people
 * then hold rows for the same (accountKey, sessionId), with the same or one staler set of
 * absolute totals. Pooled on that account, both rows are visible, and adding them up would count
 * the session (and its project) twice.
 *
 * The rule: among the visible rows with the same (accountKey, sessionId), the one that counts
 * has the latest `lastActivityAt`; then an ended one over one still running; then the larger
 * `messageCount`; then the latest `updatedAt` (sync sets it on every upsert, so it is the copy
 * synced last); then the smallest `userId`. Those columns are never NULL (`endedAt IS NOT NULL`
 * stands in for the end), and one user holds at most one row per (accountKey, sessionId) (sync
 * stores ids lowercased under a unique index), so this is a total order and exactly one row
 * counts. Only rows the viewer sees take part, so a row they can't see never hides one they can
 * (which would tell them it exists).
 *
 * Copies of another person's session are only visible on accounts the viewer shares with
 * someone, so only those are checked. With none, this is plain TRUE and every read is as it was.
 * On a shared account, the check is one probe of the (accountKey, sessionId) index
 * (schema.prisma) per session.
 */
export function canonicalSessionSql(
  scope: AccessScope,
  alias: Exclude<SqlAlias, "o">,
): Prisma.Sql {
  const shared = sharedAccounts(scope);
  if (shared.length === 0) return Prisma.sql`TRUE`;
  const column = (name: string) => Prisma.raw(`${alias}."${name}"`);
  // The preference as one row-value comparison: `o` is preferred when it is greater. The user
  // ids are swapped between the sides, so the smaller one wins the last tie.
  return Prisma.sql`(${column("accountKey")} NOT IN (${Prisma.join(shared.map((s) => s.accountKey))})
    OR NOT EXISTS (
      SELECT 1 FROM "Session" o
      WHERE o."accountKey" = ${column("accountKey")}
        AND o."sessionId" = ${column("sessionId")}
        AND o."userId" <> ${column("userId")}
        AND ${ownedRowSql({ OR: shared }, "o")}
        AND (o."lastActivityAt", o."endedAt" IS NOT NULL, o."messageCount", o."updatedAt",
             ${column("userId")})
          > (${column("lastActivityAt")}, ${column("endedAt")} IS NOT NULL,
             ${column("messageCount")}, ${column("updatedAt")}, o."userId")))`;
}

/**
 * The accounts the viewer shares with someone else, each with everyone whose rows they see on
 * it, sorted: the only accounts where they can see another person's copy of a session.
 */
export function sharedAccounts(
  scope: AccessScope,
): Array<{ accountKey: string; userId: { in: string[] } }> {
  return [...scope.pooledMembers]
    .filter(([, members]) => members.size > 1)
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([accountKey, members]) => ({
      accountKey,
      userId: { in: [...members].sort() },
    }));
}

/** A session's summary as a read shows it: a join to add to its FROM, and the three columns. */
export type SessionSummarySql = {
  join: Prisma.Sql;
  text: Prisma.Sql;
  model: Prisma.Sql;
  at: Prisma.Sql;
};

/**
 * The summary (text, model, time) of the Session row aliased `alias`, among the copies of its
 * Claude session the viewer sees (`canonicalSessionSql`): its own when it has one, otherwise the
 * latest (by `summaryAt`) another visible copy carries.
 *
 * Why: summaries are a switch per sign-in, and a new sign-in starts with it off, so the copy a
 * Mac sends again for its next user often has no summary while the first user's copy has one.
 * When the copy without one is the one that counts, the summary would otherwise vanish from the
 * pool's views. A copy outside the viewer's pools never lends one.
 *
 * The join adds a LATERAL subquery aliased `sb`, which finds a row only when the session has no
 * summary of its own and is on an account the viewer shares. With no account shared, there is no
 * join and the columns are the row's own, as before.
 */
export function sessionSummarySql(
  scope: AccessScope,
  alias: Exclude<SqlAlias, "x">,
): SessionSummarySql {
  const column = (name: string) => Prisma.raw(`${alias}."${name}"`);
  const shared = sharedAccounts(scope);
  if (shared.length === 0) {
    return {
      join: Prisma.empty,
      text: column("summaryText"),
      model: column("summaryModel"),
      at: column("summaryAt"),
    };
  }
  const borrowed = (name: string) =>
    Prisma.sql`CASE WHEN sb."summaryText" IS NULL THEN ${column(name)} ELSE sb.${Prisma.raw(`"${name}"`)} END`;
  return {
    join: Prisma.sql`LEFT JOIN LATERAL (
      SELECT x."summaryText", x."summaryModel", x."summaryAt"
      FROM "Session" x
      WHERE ${column("summaryText")} IS NULL
        AND ${column("accountKey")} IN (${Prisma.join(shared.map((s) => s.accountKey))})
        AND x."accountKey" = ${column("accountKey")}
        AND x."sessionId" = ${column("sessionId")}
        AND x."userId" <> ${column("userId")}
        AND ${ownedRowSql({ OR: shared }, "x")}
        AND x."summaryText" IS NOT NULL
      ORDER BY x."summaryAt" DESC NULLS LAST, x."id"
      LIMIT 1
    ) sb ON TRUE`,
    text: borrowed("summaryText"),
    model: borrowed("summaryModel"),
    at: borrowed("summaryAt"),
  };
}

/**
 * An ILIKE pattern that finds `term` anywhere, taking it literally: `%`, `_` and the escape
 * character itself (Postgres' default, a backslash) are escaped.
 */
export function containsPattern(term: string): string {
  return `%${term.replace(/[\\%_]/g, (c) => `\\${c}`)}%`;
}
