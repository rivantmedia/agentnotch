/**
 * Usage across accounts, for the usage page (app/usage): the tokens, cost and sessions of the
 * sessions started in a period on the accounts the viewer picked (every account they see unless
 * they name some), added up, split by account, and over time.
 *
 * Only sessions add up. A usage limit is a percentage of one account's own plan, so two accounts'
 * limits never make a total; the page shows each account's beside the others instead.
 *
 * A session counts in a period by its start, a start dated in the future counting as now, as in
 * the account cards' totals (accounts.ts) and usage by project (project-usage.ts), so the same
 * period reads the same everywhere.
 */
import {
  bucketStarts,
  FALLBACK_TIME_ZONE,
  knownTimeZone,
  startOfUnit,
  unitForSpan,
  type BucketUnit,
} from "~/lib/calendar";
import { periodDays, type UsagePeriod } from "~/lib/usage-period";
import { Prisma, type Db } from "~/server/db-types";
import {
  coveredAccountKeys,
  ownedRowWhere,
  visibleAccountKeys,
  type AccessScope,
} from "~/server/services/access";
import { ownedRowSql, sqlTime } from "~/server/services/sql";
import {
  addUp,
  daysBefore,
  decimalToNumber,
  tokenTotals,
  type UsageTotals,
} from "~/server/services/totals";

const DAY_MS = 24 * 60 * 60 * 1000;

export type CombinedUsageFilter = {
  period: UsagePeriod;
  /** Only these accounts; every account the viewer sees when left out. */
  accountKeys?: readonly string[];
};

/** One account's part of the period. */
export type AccountUsage = UsageTotals & {
  accountKey: string;
  /** Its latest session activity, whatever the period. Never after now. */
  lastUsedAt: Date | null;
};

export type CombinedUsage = {
  period: UsagePeriod;
  /** Sessions started at or after this count; null for all time. */
  from: Date | null;
  to: Date;
  /**
   * The accounts added up: the ones asked for that the viewer can see (one they can't is left
   * out, as a missing one is), or every one they see. Sorted.
   */
  accountKeys: string[];
  total: UsageTotals;
  /** Every account added up, most tokens first; those without sessions in the period last. */
  accounts: AccountUsage[];
  /** The earliest start of any session on these accounts, whatever the period. Never after now. */
  firstStartedAt: Date | null;
  /**
   * The days a daily average divides by: the period's, or fewer when the first session on these
   * accounts came later (all time: since it). A day begun counts whole. 0 with no sessions.
   */
  averageDays: number;
  /** Whether sessions started before the period: a longer one would show more. */
  earlierSessions: boolean;
};

/** A slice of time's usage, split by account. */
export type UsageBucket = UsageTotals & {
  /** Where it starts: a local day, week or month, except a rolling period's first bucket. */
  start: Date;
  /** Where the next bucket starts; the last ends now. */
  end: Date;
  /** The accounts with sessions in it, most tokens first. */
  accounts: Array<UsageTotals & { accountKey: string }>;
};

export type UsageTimeline = {
  period: UsagePeriod;
  /** The zone the buckets follow: the one asked for, or UTC when this server doesn't know it. */
  timeZone: string;
  unit: BucketUnit;
  from: Date | null;
  to: Date;
  accountKeys: string[];
  /** In time order, the empty ones included; none when nothing ever started (all time). */
  buckets: UsageBucket[];
};

function accountsFor(scope: AccessScope, filter: CombinedUsageFilter) {
  return filter.accountKeys === undefined
    ? visibleAccountKeys(scope)
    : coveredAccountKeys(scope, filter.accountKeys);
}

function periodStart(period: UsagePeriod, now: Date): Date | null {
  const days = periodDays(period);
  return days === null ? null : daysBefore(now, days);
}

/**
 * The sessions the viewer sees on `keys`, aliased `t`, with their times clamped to the server's
 * clock: one dated in the future counts as now, never later.
 */
function visibleSessions(
  scope: AccessScope,
  keys: readonly string[],
  at: Prisma.Sql,
): Prisma.Sql {
  return Prisma.sql`(
      SELECT s."accountKey",
             LEAST(s."startedAt", ${at}) AS "startedAt",
             LEAST(s."lastActivityAt", ${at}) AS "lastActivityAt",
             s."inputTokens", s."outputTokens", s."cacheCreationTokens", s."cacheReadTokens",
             s."costUsd"
      FROM "Session" s
      WHERE s."accountKey" IN (${Prisma.join(keys)})
        AND ${ownedRowSql(ownedRowWhere(scope), "s")}
    ) t`;
}

/** Token and cost sums of the rows `where` keeps (on `t`). */
function sums(where: Prisma.Sql): Prisma.Sql {
  return Prisma.sql`COUNT(*) FILTER (WHERE ${where})::int AS "sessions",
       COALESCE(SUM(t."inputTokens") FILTER (WHERE ${where}), 0)::bigint AS "inputTokens",
       COALESCE(SUM(t."outputTokens") FILTER (WHERE ${where}), 0)::bigint AS "outputTokens",
       COALESCE(SUM(t."cacheCreationTokens") FILTER (WHERE ${where}), 0)::bigint
         AS "cacheCreationTokens",
       COALESCE(SUM(t."cacheReadTokens") FILTER (WHERE ${where}), 0)::bigint
         AS "cacheReadTokens",
       SUM(t."costUsd") FILTER (WHERE ${where}) AS "costUsd"`;
}

type SumRow = {
  sessions: number;
  inputTokens: bigint;
  outputTokens: bigint;
  cacheCreationTokens: bigint;
  cacheReadTokens: bigint;
  costUsd: Prisma.Decimal | null;
};

function totalsOf(row: SumRow): UsageTotals {
  return {
    sessions: row.sessions,
    tokens: tokenTotals(row),
    costUsd: decimalToNumber(row.costUsd),
  };
}

/** Most tokens first, then the higher cost, then more sessions, then by key, so it is stable. */
function byUsage(
  a: UsageTotals & { accountKey: string },
  b: UsageTotals & { accountKey: string },
): number {
  if (a.tokens.total !== b.tokens.total) {
    return a.tokens.total > b.tokens.total ? -1 : 1;
  }
  return (
    (b.costUsd ?? -1) - (a.costUsd ?? -1) ||
    b.sessions - a.sessions ||
    (a.accountKey < b.accountKey ? -1 : a.accountKey > b.accountKey ? 1 : 0)
  );
}

/** The period's usage on the chosen accounts: added up, and each account's part. */
export async function combinedUsage(
  db: Db,
  scope: AccessScope,
  filter: CombinedUsageFilter,
  now: Date = new Date(),
): Promise<CombinedUsage> {
  const keys = accountsFor(scope, filter);
  const from = periodStart(filter.period, now);
  const at = sqlTime(now);
  const inPeriod =
    from === null
      ? Prisma.sql`TRUE`
      : Prisma.sql`t."startedAt" >= ${sqlTime(from)}`;

  const rows =
    keys.length === 0
      ? []
      : await db.$queryRaw<
          Array<
            SumRow & {
              accountKey: string;
              firstStartedAt: Date | null;
              lastActivityAt: Date | null;
            }
          >
        >`
          SELECT t."accountKey", ${sums(inPeriod)},
                 MIN(t."startedAt") AS "firstStartedAt",
                 MAX(t."lastActivityAt") AS "lastActivityAt"
          FROM ${visibleSessions(scope, keys, at)}
          GROUP BY t."accountKey"`;

  const accounts = keys
    .map((accountKey): AccountUsage => {
      const row = rows.find((r) => r.accountKey === accountKey);
      return {
        accountKey,
        ...(row ? totalsOf(row) : addUp([])),
        lastUsedAt: row?.lastActivityAt ?? null,
      };
    })
    .sort(byUsage);
  const total = addUp(accounts);
  const firstStartedAt = rows.reduce<Date | null>(
    (first, row) =>
      row.firstStartedAt !== null &&
      (first === null || row.firstStartedAt < first)
        ? row.firstStartedAt
        : first,
    null,
  );
  // The average spans the period, or less of it when the first session came later.
  const since =
    firstStartedAt === null
      ? null
      : from === null || firstStartedAt > from
        ? firstStartedAt
        : from;

  return {
    period: filter.period,
    from,
    to: now,
    accountKeys: keys,
    total,
    accounts,
    firstStartedAt,
    averageDays:
      since === null || total.sessions === 0
        ? 0
        : Math.max(1, Math.ceil((now.getTime() - since.getTime()) / DAY_MS)),
    earlierSessions:
      from !== null && firstStartedAt !== null && firstStartedAt < from,
  };
}

/**
 * The period's usage on the chosen accounts over time, in buckets of the viewer's calendar: days
 * for 7 and 30 days, and for all time days, weeks or months by how far back the first session
 * goes. A rolling period's first bucket is the rest of its first day, so the buckets add up to
 * exactly the period's totals (combinedUsage).
 */
export async function usageTimeline(
  db: Db,
  scope: AccessScope,
  filter: CombinedUsageFilter & { timeZone: string },
  now: Date = new Date(),
): Promise<UsageTimeline> {
  const keys = accountsFor(scope, filter);
  const timeZone = knownTimeZone(filter.timeZone) ?? FALLBACK_TIME_ZONE;
  const from = periodStart(filter.period, now);
  const at = sqlTime(now);
  const empty = (unit: BucketUnit): UsageTimeline => ({
    period: filter.period,
    timeZone,
    unit,
    from,
    to: now,
    accountKeys: keys,
    buckets: [],
  });
  if (keys.length === 0) return empty("day");

  let start: number;
  let unit: BucketUnit;
  if (from !== null) {
    start = from.getTime();
    unit = "day";
  } else {
    const [first] = await db.$queryRaw<Array<{ firstStartedAt: Date | null }>>`
      SELECT MIN(t."startedAt") AS "firstStartedAt"
      FROM ${visibleSessions(scope, keys, at)}`;
    if (!first?.firstStartedAt) return empty("day");
    unit = unitForSpan(now.getTime() - first.firstStartedAt.getTime());
    start = startOfUnit(first.firstStartedAt.getTime(), unit, timeZone);
  }

  const starts = bucketStarts(start, now.getTime(), unit, timeZone);
  // width_bucket numbers the buckets from 1 by the starts (sorted ascending) a time has reached.
  const rows = await db.$queryRaw<
    Array<SumRow & { accountKey: string; bucket: number }>
  >`
    SELECT t."accountKey",
           width_bucket(
             t."startedAt",
             ARRAY[${Prisma.join(starts.map((s) => sqlTime(new Date(s))))}]
           ) AS "bucket",
           ${sums(Prisma.sql`TRUE`)}
    FROM ${visibleSessions(scope, keys, at)}
    WHERE t."startedAt" >= ${sqlTime(new Date(starts[0]!))}
    GROUP BY t."accountKey", 2`;

  return {
    ...empty(unit),
    buckets: starts.map((s, i) => {
      const parts = rows
        .filter((row) => row.bucket === i + 1)
        .map((row) => ({ accountKey: row.accountKey, ...totalsOf(row) }))
        .sort(byUsage);
      return {
        start: new Date(s),
        end: new Date(starts[i + 1] ?? now.getTime()),
        ...addUp(parts),
        accounts: parts,
      };
    }),
  };
}
