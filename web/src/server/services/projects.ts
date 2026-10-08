/**
 * Projects (working directories, by name only) on one account, with their totals.
 *
 * Project keys are made with a secret each install keeps to itself (contract/README.md), so the
 * same folder synced from two Macs is two `Project` rows. For display, one person's rows on an
 * account are grouped by folder name: the group sums their sessions and tokens, takes the latest
 * use and counts the Macs its sessions came from. Two different folders of the same name are
 * grouped too; the website can't tell them apart. The rows themselves stay as they are.
 *
 * A Claude session that reached the website from more than one person (one Mac signed in as each
 * in turn) counts once, from the copy `canonicalSessionSql` (sql.ts) picks, and a group whose
 * sessions are all other people's copies is not listed: it is the same folder, listed under the
 * person whose copies count.
 */
import { Prisma, type Db } from "~/server/db-types";
import {
  AccessDenied,
  canSeeAccount,
  canSeeRow,
  ownedRowWhere,
  type AccessScope,
} from "~/server/services/access";
import {
  canonicalSessionSql,
  clampedTime,
  ownedRowSql,
  sessionSummarySql,
  sharedAccounts,
  sqlTime,
} from "~/server/services/sql";
import {
  decimalToNumber,
  peopleById,
  personOrUnknown,
  tokenTotals,
  type Person,
  type TokenTotals,
} from "~/server/services/totals";

/**
 * A project as the website shows it: one person's folders of one name on one account, from any
 * of their Macs. Project keys are left out on purpose: they identify nothing a page needs, and
 * nobody is ever shown one (contract/README.md).
 */
export type ProjectSummary = {
  /** The group's id: the first of `projectIds`. Filtering sessions by it covers the group. */
  id: string;
  /** Every project row in the group, sorted (at least one per Mac that used the name). */
  projectIds: string[];
  name: string;
  accountKey: string;
  owner: Person;
  sessionCount: number;
  /** How many Macs the group's sessions were synced from. */
  macCount: number;
  tokens: TokenTotals;
  costUsd: number | null;
  /**
   * The earliest start and the latest activity, never later than the server's clock. Null when
   * the group has no session: a project row can be left without any until its owner's next sync
   * with sessions deletes it (sync.ts), and rows stored before that cleanup existed may be.
   */
  firstUsedAt: Date | null;
  lastUsedAt: Date | null;
  /**
   * The most recent session summaries, newest first (only when summaries were turned on). Their
   * times are never later than the server's clock either.
   */
  latestSummaries: Array<{
    sessionRowId: string;
    title: string | null;
    text: string;
    model: string | null;
    generatedAt: Date | null;
  }>;
};

const SUMMARIES_PER_PROJECT = 3;

export async function listProjects(
  db: Db,
  scope: AccessScope,
  accountKey: string,
  now: Date = new Date(),
): Promise<ProjectSummary[]> {
  if (!canSeeAccount(scope, accountKey)) {
    throw new AccessDenied("NOT_FOUND", "No such account.");
  }
  const groups = await summarizeGroups(
    db,
    scope,
    accountKey,
    Prisma.sql`TRUE`,
    now,
  );
  return groups.sort(
    (a, b) =>
      (b.lastUsedAt?.getTime() ?? 0) - (a.lastUsedAt?.getTime() ?? 0) ||
      a.name.localeCompare(b.name) ||
      a.id.localeCompare(b.id),
  );
}

/**
 * The group the project row `id` belongs to, or, when that group's sessions are all other
 * people's copies, the group they count in (`countedProjectRow`).
 */
export async function getProject(
  db: Db,
  scope: AccessScope,
  id: string,
  now: Date = new Date(),
): Promise<ProjectSummary> {
  const row = await countedProjectRow(db, scope, id, { acrossAccounts: false });
  if (!row) throw new AccessDenied("NOT_FOUND", "No such project.");
  const [summary] = await summarizeGroups(
    db,
    scope,
    row.accountKey,
    Prisma.sql`p."userId" = ${row.userId} AND p."name" = ${row.name}`,
    now,
  );
  if (!summary) throw new AccessDenied("NOT_FOUND", "No such project.");
  return summary;
}

/** A project row as `countedProjectRow` resolves it. */
export type ProjectRow = {
  id: string;
  userId: string;
  accountKey: string;
  name: string;
};

/**
 * The project row `id` names, when the viewer may see it (null otherwise). When the sessions of
 * its group (its owner's folders of its name, on its account or with `acrossAccounts` on every
 * account) are all other people's copies, the row of the folder the copies that count are in
 * instead: the same Mac's folder, synced by the person whose copies count.
 *
 * A group like that isn't listed (`summarizeGroups`, project-usage.ts), but its row is still
 * visible, and a bookmark, an open tab or the other person's next sync can lead to it; resolving
 * it shows the folder rather than an error. A group with a session that counts, or with no
 * session, is itself; so is every group of a viewer who shares no account, without a query.
 */
export async function countedProjectRow(
  db: Db,
  scope: AccessScope,
  id: string,
  options: { acrossAccounts: boolean },
): Promise<ProjectRow | null> {
  const row = await db.project.findUnique({
    where: { id },
    select: { id: true, userId: true, accountKey: true, name: true },
  });
  if (!row || !canSeeRow(scope, row)) return null;
  // Another person's copy of a session is only ever visible on an account shared with them: on
  // the row's own account alone, unless the group spans accounts.
  const shared = sharedAccounts(scope);
  if (
    shared.length === 0 ||
    (!options.acrossAccounts &&
      !shared.some((a) => a.accountKey === row.accountKey))
  ) {
    return row;
  }
  const sameAccount = options.acrossAccounts
    ? Prisma.empty
    : Prisma.sql`AND p."accountKey" = ${row.accountKey}`;
  const visible = ownedRowWhere(scope);
  // The copy that counts of each of the group's sessions, the group's own first: when it has
  // one, the group stands. Otherwise its latest session's counted copy names the folder.
  const [counted] = await db.$queryRaw<ProjectRow[]>`
    SELECT t."id", t."userId", t."accountKey", t."name"
    FROM "Project" p
    JOIN "Session" s ON s."projectId" = p."id" AND s."userId" = p."userId"
    JOIN "Session" c ON c."accountKey" = s."accountKey" AND c."sessionId" = s."sessionId"
    JOIN "Project" t ON t."id" = c."projectId"
    WHERE p."userId" = ${row.userId} AND p."name" = ${row.name} ${sameAccount}
      AND ${ownedRowSql(visible, "p")}
      AND ${ownedRowSql(visible, "c")}
      AND ${canonicalSessionSql(scope, "c")}
    ORDER BY (c."userId" = p."userId") DESC, s."lastActivityAt" DESC, c."id"
    LIMIT 1`;
  return counted && counted.userId !== row.userId ? counted : row;
}

type GroupRow = {
  userId: string;
  name: string;
  projectIds: string[];
  sessionCount: number;
  macCount: number;
  inputTokens: bigint;
  outputTokens: bigint;
  cacheCreationTokens: bigint;
  cacheReadTokens: bigint;
  costUsd: Prisma.Decimal | null;
  firstUsedAt: Date | null;
  lastUsedAt: Date | null;
};

type SummaryRow = {
  userId: string;
  name: string;
  id: string;
  title: string | null;
  summaryText: string;
  summaryModel: string | null;
  summaryAt: Date | null;
};

/**
 * The visible project groups on an account that `where` (on `p`, the Project row) selects. Only
 * the copy of each Claude session that counts adds to a group (`canonicalSessionSql`, worked out
 * once per session in the derived table), and a group with sessions but none that count is left
 * out (`countedProjectRow` resolves its id to the group that counts its sessions). A session's
 * summary may be another visible copy's (`sessionSummarySql`). Each session's times are clamped
 * to the server's clock in the SQL, so one dated in the future makes its project "last used"
 * now, never later. A project row without sessions joins as one row of NULLs, which the clamp
 * keeps NULL, so it adds nothing to the group's times.
 */
async function summarizeGroups(
  db: Db,
  scope: AccessScope,
  accountKey: string,
  where: Prisma.Sql,
  now: Date,
): Promise<ProjectSummary[]> {
  const at = sqlTime(now);
  const visibleWhere = ownedRowWhere(scope, { accountKey });
  const visible = ownedRowSql(visibleWhere, "p");
  const counts = Prisma.sql`FILTER (WHERE s."canonical")`;
  const summary = sessionSummarySql(scope, "s");
  const [groups, summaries] = await Promise.all([
    // A session belongs to its owner's project (sync joins them by user), so the join by user
    // only restates that.
    db.$queryRaw<GroupRow[]>`
      SELECT p."userId", p."name",
             array_agg(DISTINCT p."id" ORDER BY p."id") AS "projectIds",
             COUNT(s."id") ${counts}::int AS "sessionCount",
             COUNT(DISTINCT s."deviceId") ${counts}::int AS "macCount",
             COALESCE(SUM(s."inputTokens") ${counts}, 0)::bigint AS "inputTokens",
             COALESCE(SUM(s."outputTokens") ${counts}, 0)::bigint AS "outputTokens",
             COALESCE(SUM(s."cacheCreationTokens") ${counts}, 0)::bigint
               AS "cacheCreationTokens",
             COALESCE(SUM(s."cacheReadTokens") ${counts}, 0)::bigint AS "cacheReadTokens",
             SUM(s."costUsd") ${counts} AS "costUsd",
             MIN(${clampedTime(Prisma.sql`s."startedAt"`, at)}) ${counts} AS "firstUsedAt",
             MAX(${clampedTime(Prisma.sql`s."lastActivityAt"`, at)}) ${counts} AS "lastUsedAt"
      FROM "Project" p
      LEFT JOIN (
        SELECT s."id", s."projectId", s."userId", s."deviceId", s."startedAt",
               s."lastActivityAt", s."inputTokens", s."outputTokens", s."cacheCreationTokens",
               s."cacheReadTokens", s."costUsd",
               ${canonicalSessionSql(scope, "s")} AS "canonical"
        FROM "Session" s
        WHERE ${ownedRowSql(visibleWhere, "s")}
      ) s ON s."projectId" = p."id" AND s."userId" = p."userId"
      WHERE ${visible} AND ${where}
      GROUP BY p."userId", p."name"
      -- No sessions at all, as before; sessions but only other people's copies: left out.
      HAVING COUNT(s."id") = 0 OR bool_or(s."canonical")`,
    db.$queryRaw<SummaryRow[]>`
      SELECT t."userId", t."name", t."id", t."title", t."summaryText", t."summaryModel",
             ${clampedTime(Prisma.sql`t."summaryAt"`, at)} AS "summaryAt"
      FROM (
        SELECT p."userId", p."name", s."id", s."title", ${summary.text} AS "summaryText",
               ${summary.model} AS "summaryModel", ${summary.at} AS "summaryAt",
               ROW_NUMBER() OVER (
                 PARTITION BY p."userId", p."name"
                 ORDER BY ${summary.at} DESC NULLS LAST, s."lastActivityAt" DESC, s."id" DESC
               ) AS "rank"
        FROM "Project" p
        JOIN "Session" s ON s."projectId" = p."id" AND s."userId" = p."userId"
        ${summary.join}
        WHERE ${visible} AND ${where} AND ${summary.text} IS NOT NULL
          AND ${canonicalSessionSql(scope, "s")}
      ) t
      WHERE t."rank" <= ${SUMMARIES_PER_PROJECT}
      ORDER BY t."rank"`,
  ]);
  if (groups.length === 0) return [];

  const people = await peopleById(
    db,
    scope.viewerId,
    groups.map((g) => g.userId),
  );
  return groups.map((group) => ({
    id: group.projectIds[0]!,
    projectIds: group.projectIds,
    name: group.name,
    accountKey,
    owner: personOrUnknown(people, group.userId, scope.viewerId),
    sessionCount: group.sessionCount,
    macCount: group.macCount,
    tokens: tokenTotals(group),
    costUsd: decimalToNumber(group.costUsd),
    firstUsedAt: group.firstUsedAt,
    lastUsedAt: group.lastUsedAt,
    latestSummaries: summaries
      .filter((s) => s.userId === group.userId && s.name === group.name)
      .map((s) => ({
        sessionRowId: s.id,
        title: s.title,
        text: s.summaryText,
        model: s.summaryModel,
        generatedAt: s.summaryAt,
      })),
  }));
}
