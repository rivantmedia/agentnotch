/**
 * Usage by project, and a project's usage by account: the tokens, cost and sessions of the
 * sessions started in a period (usage-period.ts), added up per project.
 *
 * A project here is one person's folders of one name, as on the account page (projects.ts), but
 * across every account the viewer can see: the same folder name on a person's other Claude
 * accounts joins the group, and the group's usage splits by account. Project keys are made with
 * the account's key and a secret per install (contract/README.md), so the website can't tell
 * that two accounts (or two Macs) worked in the same folder; grouping by name is all it can do,
 * and two different folders of one name are grouped too. A pool member's projects count only on
 * the accounts shared with the viewer, like everything else of theirs.
 *
 * Usage limits are per account, and nothing reports them per project: these are shares of the
 * tokens, never of a limit.
 */
import { periodDays, type UsagePeriod } from "~/lib/usage-period";
import { Prisma, type Db } from "~/server/db-types";
import {
  AccessDenied,
  canSeeAccount,
  canSeeRow,
  ownedRowWhere,
  type AccessScope,
} from "~/server/services/access";
import { clampedTime, ownedRowSql, sqlTime } from "~/server/services/sql";
import {
  daysBefore,
  decimalToNumber,
  peopleById,
  personOrUnknown,
  tokenTotals,
  type Person,
  type TokenTotals,
} from "~/server/services/totals";

/** What a period's sessions added up to. */
export type UsageTotals = {
  sessions: number;
  tokens: TokenTotals;
  /** The sum of the costs Claude Code reported; null when none did. */
  costUsd: number | null;
};

/** One project's usage on one account. */
export type ProjectAccountUsage = UsageTotals & {
  accountKey: string;
  /**
   * The first of the project's rows on this account: the account page's `?project=` filter
   * picks the project's whole group there with it.
   */
  projectId: string;
  /** The latest activity of any of its sessions on this account, whatever the period. */
  lastUsedAt: Date | null;
};

/** One person's folders of one name, on every account the viewer sees them on. */
export type ProjectUsage = UsageTotals & {
  /** The group's id: the first of `projectIds`. The project page takes it (or any of them). */
  id: string;
  /** Every project row in the group, sorted: one per Mac and account that used the name. */
  projectIds: string[];
  name: string;
  owner: Person;
  /** How many Macs the period's sessions were synced from. */
  macCount: number;
  /** The latest activity of any of its sessions, whatever the period. Never after now. */
  lastUsedAt: Date | null;
  /**
   * Its usage per account, most tokens first. In a report, only the accounts with sessions in
   * the period; in a project's detail, every account it has rows on.
   */
  accounts: ProjectAccountUsage[];
};

export type ProjectUsageReport = {
  period: UsagePeriod;
  /** Sessions started at or after this count; null for all time. */
  from: Date | null;
  /** Every project with sessions in the period, added up: the shares' whole. */
  total: UsageTotals & { projects: number };
  /** The projects with sessions in the period, most tokens first, at most `limit` of them. */
  projects: ProjectUsage[];
  /** The projects past `limit`, added up; null when none were left out. */
  rest: (UsageTotals & { projects: number }) | null;
  /**
   * Whether sessions started before the period (on the same accounts): a longer period would
   * show more. Always false for all time.
   */
  earlierSessions: boolean;
};

export type ProjectDetail = ProjectUsage & {
  period: UsagePeriod;
  from: Date | null;
  /** The earliest start of any of its sessions, whatever the period. Never after now. */
  firstUsedAt: Date | null;
};

export type ProjectUsageFilter = {
  period: UsagePeriod;
  /** Only this account's projects (and only their usage on it). */
  accountKey?: string;
  /** How many projects to list; the rest are added up in `rest`. */
  limit?: number;
};

/**
 * The projects with sessions in the period on the accounts the viewer sees (or on one of them),
 * most tokens first.
 */
export async function projectUsage(
  db: Db,
  scope: AccessScope,
  filter: ProjectUsageFilter,
  now: Date = new Date(),
): Promise<ProjectUsageReport> {
  if (
    filter.accountKey !== undefined &&
    !canSeeAccount(scope, filter.accountKey)
  ) {
    throw new AccessDenied("NOT_FOUND", "No such account.");
  }
  const from = periodStart(filter.period, now);
  const rows = await usageRows(
    db,
    scope,
    filter.accountKey,
    Prisma.sql`TRUE`,
    from,
    now,
  );
  const projects = (await toProjects(db, scope, rows))
    .map((project) => ({
      ...project,
      accounts: project.accounts.filter((a) => a.sessions > 0),
    }))
    .filter((project) => project.sessions > 0)
    .sort(byUsage);

  const kept =
    filter.limit === undefined ? projects : projects.slice(0, filter.limit);
  const left = projects.slice(kept.length);
  return {
    period: filter.period,
    from,
    total: { ...addUp(projects), projects: projects.length },
    projects: kept,
    rest: left.length > 0 ? { ...addUp(left), projects: left.length } : null,
    // Each row's first use is its earliest session whatever the period, clamped as the period's
    // own test is, so one before `from` is exactly a session the period leaves out.
    earlierSessions:
      from !== null &&
      rows.some((row) => row.firstUsedAt !== null && row.firstUsedAt < from),
  };
}

/**
 * The project (its whole group, on every account the viewer sees it on) that the project row
 * `id` belongs to, with its usage in the period per account. Accounts it had no session on in
 * the period are listed too, last.
 */
export async function projectDetail(
  db: Db,
  scope: AccessScope,
  id: string,
  period: UsagePeriod,
  now: Date = new Date(),
): Promise<ProjectDetail> {
  const row = await visibleProjectRow(db, scope, id);
  if (!row) throw new AccessDenied("NOT_FOUND", "No such project.");
  const from = periodStart(period, now);
  const rows = await usageRows(
    db,
    scope,
    undefined,
    Prisma.sql`p."userId" = ${row.userId} AND p."name" = ${row.name}`,
    from,
    now,
  );
  const [project] = await toProjects(db, scope, rows);
  if (!project) throw new AccessDenied("NOT_FOUND", "No such project.");
  const firstUsedAt = rows.reduce<Date | null>(
    (first, r) =>
      r.firstUsedAt !== null && (first === null || r.firstUsedAt < first)
        ? r.firstUsedAt
        : first,
    null,
  );
  return { ...project, period, from, firstUsedAt };
}

/** Whether the viewer may see the project row `id`: the project page's cheap check. */
export async function canSeeProject(
  db: Db,
  scope: AccessScope,
  id: string,
): Promise<boolean> {
  return (await visibleProjectRow(db, scope, id)) !== null;
}

async function visibleProjectRow(db: Db, scope: AccessScope, id: string) {
  const row = await db.project.findUnique({
    where: { id },
    select: { userId: true, accountKey: true, name: true },
  });
  return row && canSeeRow(scope, row) ? row : null;
}

function periodStart(period: UsagePeriod, now: Date): Date | null {
  const days = periodDays(period);
  return days === null ? null : daysBefore(now, days);
}

/** One person's rows of one name on one account, with the period's usage. */
type UsageRow = {
  userId: string;
  name: string;
  accountKey: string;
  projectIds: string[];
  sessions: number;
  deviceIds: string[];
  inputTokens: bigint;
  outputTokens: bigint;
  cacheCreationTokens: bigint;
  cacheReadTokens: bigint;
  costUsd: Prisma.Decimal | null;
  firstUsedAt: Date | null;
  lastUsedAt: Date | null;
};

/**
 * The visible project rows `where` (on `p`) selects, grouped by person, name and account, with
 * the usage of their sessions started in the period. Every session's times are clamped to the
 * server's clock first, so one dated in the future falls in the period as one started now would,
 * and dates nothing later than now. A row without sessions joins as one row of NULLs, which adds
 * nothing (and which the clamp keeps NULL).
 */
async function usageRows(
  db: Db,
  scope: AccessScope,
  accountKey: string | undefined,
  where: Prisma.Sql,
  from: Date | null,
  now: Date,
): Promise<UsageRow[]> {
  const at = sqlTime(now);
  const inPeriod =
    from === null
      ? Prisma.sql`TRUE`
      : Prisma.sql`LEAST(s."startedAt", ${at}) >= ${sqlTime(from)}`;
  const visible = ownedRowSql(ownedRowWhere(scope, { accountKey }), "p");
  // A session belongs to its owner's project on its own account (sync joins them so); the join
  // only restates that.
  return db.$queryRaw<UsageRow[]>`
    SELECT p."userId", p."name", p."accountKey",
           array_agg(DISTINCT p."id" ORDER BY p."id") AS "projectIds",
           COUNT(s."id") FILTER (WHERE ${inPeriod})::int AS "sessions",
           COALESCE(
             array_agg(DISTINCT s."deviceId")
               FILTER (WHERE ${inPeriod} AND s."deviceId" IS NOT NULL),
             '{}'
           ) AS "deviceIds",
           COALESCE(SUM(s."inputTokens") FILTER (WHERE ${inPeriod}), 0)::bigint AS "inputTokens",
           COALESCE(SUM(s."outputTokens") FILTER (WHERE ${inPeriod}), 0)::bigint AS "outputTokens",
           COALESCE(SUM(s."cacheCreationTokens") FILTER (WHERE ${inPeriod}), 0)::bigint
             AS "cacheCreationTokens",
           COALESCE(SUM(s."cacheReadTokens") FILTER (WHERE ${inPeriod}), 0)::bigint
             AS "cacheReadTokens",
           SUM(s."costUsd") FILTER (WHERE ${inPeriod}) AS "costUsd",
           MIN(${clampedTime(Prisma.sql`s."startedAt"`, at)}) AS "firstUsedAt",
           MAX(${clampedTime(Prisma.sql`s."lastActivityAt"`, at)}) AS "lastUsedAt"
    FROM "Project" p
    LEFT JOIN "Session" s
      ON s."projectId" = p."id" AND s."userId" = p."userId" AND s."accountKey" = p."accountKey"
    WHERE ${visible} AND ${where}
    GROUP BY p."userId", p."name", p."accountKey"`;
}

/** The rows grouped into projects (person and name), each with its accounts. */
async function toProjects(
  db: Db,
  scope: AccessScope,
  rows: readonly UsageRow[],
): Promise<ProjectUsage[]> {
  if (rows.length === 0) return [];
  const people = await peopleById(
    db,
    scope.viewerId,
    rows.map((r) => r.userId),
  );
  const groups = new Map<string, UsageRow[]>();
  for (const row of rows) {
    // Neither part can hold a NUL (Postgres text can't), so this key is unambiguous.
    const key = `${row.userId}\u0000${row.name}`;
    groups.set(key, [...(groups.get(key) ?? []), row]);
  }
  return [...groups.values()].map((group) => {
    const first = group[0]!;
    const accounts = group
      .map((row): ProjectAccountUsage => ({
        accountKey: row.accountKey,
        projectId: row.projectIds[0]!,
        sessions: row.sessions,
        tokens: tokenTotals(row),
        costUsd: decimalToNumber(row.costUsd),
        lastUsedAt: row.lastUsedAt,
      }))
      .sort(
        (a, b) =>
          compareUsage(a, b) ||
          (a.accountKey < b.accountKey
            ? -1
            : a.accountKey > b.accountKey
              ? 1
              : 0),
      );
    const projectIds = group.flatMap((row) => row.projectIds).sort();
    return {
      id: projectIds[0]!,
      projectIds,
      name: first.name,
      owner: personOrUnknown(people, first.userId, scope.viewerId),
      ...addUp(accounts),
      macCount: new Set(group.flatMap((row) => row.deviceIds)).size,
      lastUsedAt: latest(accounts.map((a) => a.lastUsedAt)),
      accounts,
    };
  });
}

function addUp(parts: readonly UsageTotals[]): UsageTotals {
  let sessions = 0;
  let input = 0n;
  let output = 0n;
  let cacheCreation = 0n;
  let cacheRead = 0n;
  let cost: number | null = null;
  for (const part of parts) {
    sessions += part.sessions;
    input += part.tokens.input;
    output += part.tokens.output;
    cacheCreation += part.tokens.cacheCreation;
    cacheRead += part.tokens.cacheRead;
    if (part.costUsd !== null) cost = (cost ?? 0) + part.costUsd;
  }
  return {
    sessions,
    tokens: tokenTotals({
      inputTokens: input,
      outputTokens: output,
      cacheCreationTokens: cacheCreation,
      cacheReadTokens: cacheRead,
    }),
    costUsd: cost,
  };
}

function latest(dates: ReadonlyArray<Date | null>): Date | null {
  return dates.reduce<Date | null>(
    (max, d) => (d !== null && (max === null || d > max) ? d : max),
    null,
  );
}

/** Most tokens first, then the higher cost, then more sessions. */
function compareUsage(a: UsageTotals, b: UsageTotals): number {
  if (a.tokens.total !== b.tokens.total) {
    return a.tokens.total > b.tokens.total ? -1 : 1;
  }
  return (b.costUsd ?? -1) - (a.costUsd ?? -1) || b.sessions - a.sessions;
}

/** By usage, then the latest used, then by name and id, so the order is stable. */
function byUsage(a: ProjectUsage, b: ProjectUsage): number {
  return (
    compareUsage(a, b) ||
    (b.lastUsedAt?.getTime() ?? 0) - (a.lastUsedAt?.getTime() ?? 0) ||
    a.name.localeCompare(b.name) ||
    a.id.localeCompare(b.id)
  );
}
