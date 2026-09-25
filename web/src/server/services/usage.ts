/**
 * Usage-limit history for charts: one series per window.
 */
import { type Db, type Prisma } from "~/server/db-types";
import {
  AccessDenied,
  canSeeAccount,
  ownedRowWhere,
  type AccessScope,
} from "~/server/services/access";
import { daysBefore } from "~/server/services/totals";

export type UsageHistoryFilter = {
  accountKey: string;
  windowId?: string;
  source?: string;
  from?: Date;
  to?: Date;
};

export type UsagePoint = {
  observedAt: Date;
  utilization: number;
  resetsAt: Date | null;
  source: string;
};

export type UsageHistory = {
  accountKey: string;
  from: Date;
  to: Date;
  /** More readings matched than MAX_POINTS; the oldest were left out. */
  truncated: boolean;
  windows: Array<{ windowId: string; points: UsagePoint[] }>;
};

export const MAX_POINTS = 5000;
const DEFAULT_DAYS = 7;

export async function usageHistory(
  db: Db,
  scope: AccessScope,
  filter: UsageHistoryFilter,
  now: Date = new Date(),
): Promise<UsageHistory> {
  if (!canSeeAccount(scope, filter.accountKey)) {
    throw new AccessDenied("NOT_FOUND", "No such account.");
  }
  const to = filter.to ?? now;
  const from = filter.from ?? daysBefore(to, DEFAULT_DAYS);

  const conditions: Prisma.UsageReadingWhereInput[] = [
    ownedRowWhere(scope, { accountKey: filter.accountKey }),
    { observedAt: { gte: from, lte: to } },
  ];
  if (filter.windowId) conditions.push({ windowId: filter.windowId });
  if (filter.source) conditions.push({ source: filter.source });

  // Newest first so a cap drops the oldest points, then put back in time order.
  const rows = await db.usageReading.findMany({
    where: { AND: conditions },
    orderBy: [{ observedAt: "desc" }, { id: "desc" }],
    take: MAX_POINTS + 1,
    select: {
      windowId: true,
      observedAt: true,
      utilization: true,
      resetsAt: true,
      source: true,
    },
  });
  const truncated = rows.length > MAX_POINTS;
  const kept = rows.slice(0, MAX_POINTS).reverse();

  const series = new Map<string, UsagePoint[]>();
  const seen = new Set<string>();
  for (const row of kept) {
    // Pool members can each report the same window at the same instant; one point is enough.
    const id = `${row.windowId}@${row.observedAt.getTime()}`;
    if (seen.has(id)) continue;
    seen.add(id);
    const points = series.get(row.windowId) ?? [];
    points.push({
      observedAt: row.observedAt,
      utilization: row.utilization,
      resetsAt: row.resetsAt,
      source: row.source,
    });
    series.set(row.windowId, points);
  }

  return {
    accountKey: filter.accountKey,
    from,
    to,
    truncated,
    windows: [...series.entries()]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([windowId, points]) => ({ windowId, points })),
  };
}
