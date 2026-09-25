/**
 * The Claude accounts a viewer can see, with their latest usage and recent totals.
 */
import { compareWindowIds, isFixedWindow } from "~/lib/format";
import { usageSourceSchema } from "~/server/app-api/schema";
import { type Db } from "~/server/db-types";
import {
  AccessDenied,
  canSeeAccount,
  isPooledWithOthers,
  ownedRowWhere,
  visibleAccountKeys,
  visibleOwnerIds,
  type AccessScope,
} from "~/server/services/access";
import { SYNC_QUOTAS } from "~/server/services/sync";
import {
  daysBefore,
  decimalToNumber,
  peopleById,
  personOrUnknown,
  SESSION_SUMS,
  tokenTotals,
  type Person,
  type TokenTotals,
} from "~/server/services/totals";

export type UsageWindowReading = {
  windowId: string;
  /** Percent of the limit used; may exceed 100. */
  utilization: number;
  resetsAt: Date | null;
  observedAt: Date;
  source: string;
};

export type PeriodTotals = {
  sessions: number;
  tokens: TokenTotals;
  /** Sum of the costs Claude Code reported; null when none did. */
  costUsd: number | null;
};

export type AccountSummary = {
  key: string;
  /**
   * What the viewer's own app reported about the account; on an account they only see through a
   * pool, what the pool's creator reported. Never another member's report.
   */
  email: string | null;
  organizationName: string | null;
  plan: string | null;
  /** The viewer's own name for this account, from their app. */
  label: string | null;
  /** Whether the viewer's own app has synced this account. */
  syncedByViewer: boolean;
  /** When the viewer's app last reported it; null if it never did. */
  lastSyncedAt: Date | null;
  /** Shared with at least one other person through a pool. */
  pooled: boolean;
  /** The pools on this account the viewer is in. */
  poolIds: string[];
  /** People whose sessions and usage are included here, the viewer counted. */
  memberCount: number;
  /**
   * The most recent reading of each usage window (from the last 35 days), at most
   * MAX_WINDOWS_PER_ACCOUNT, in the order `rankMeterWindows` gives.
   */
  usage: UsageWindowReading[];
  last7Days: PeriodTotals;
  last30Days: PeriodTotals;
  lastActivityAt: Date | null;
};

export type AccountDetail = AccountSummary & { members: Person[] };

/** Readings older than this say nothing about a 5-hour or weekly window any more. */
const USAGE_LOOKBACK_DAYS = 35;
/**
 * Readings dated later than this past the server's clock are left out of "latest": a Mac whose
 * clock runs ahead must not pin everyone's meters (sync accepts up to a day ahead, for skew).
 */
const USAGE_FUTURE_SKEW_MS = 5 * 60 * 1000;
/** Meters shown per account at most (a reading carries at most 20 windows). */
export const MAX_WINDOWS_PER_ACCOUNT = 20;

/**
 * The meters an account shows, in order, at most MAX_WINDOWS_PER_ACCOUNT: the fixed windows
 * (5-hour, weekly, extra usage), then per-model windows the viewer or the creator of a pool
 * they're in on it reported (`trusted`), then everyone else's, each group by id. Freshness plays
 * no part, so a pool member's made-up windows, however recent, can't push real ones off the card.
 */
export function rankMeterWindows(
  windows: ReadonlyArray<{ windowId: string; trusted: boolean }>,
): string[] {
  const group = (w: { windowId: string; trusted: boolean }) =>
    isFixedWindow(w.windowId) ? 0 : w.trusted ? 1 : 2;
  return [...windows]
    .sort(
      (a, b) => group(a) - group(b) || compareWindowIds(a.windowId, b.windowId),
    )
    .slice(0, MAX_WINDOWS_PER_ACCOUNT)
    .map((w) => w.windowId);
}

export async function listAccounts(
  db: Db,
  scope: AccessScope,
  now: Date = new Date(),
): Promise<AccountSummary[]> {
  const keys = visibleAccountKeys(scope);
  if (keys.length === 0) return [];
  const summaries = await summarize(db, scope, keys, now);
  return summaries.sort(
    (a, b) =>
      (b.lastActivityAt?.getTime() ?? 0) - (a.lastActivityAt?.getTime() ?? 0) ||
      a.key.localeCompare(b.key),
  );
}

export async function getAccount(
  db: Db,
  scope: AccessScope,
  accountKey: string,
  now: Date = new Date(),
): Promise<AccountDetail> {
  if (!canSeeAccount(scope, accountKey)) {
    throw new AccessDenied("NOT_FOUND", "No such account.");
  }
  const [summary] = await summarize(db, scope, [accountKey], now);
  const ids = visibleOwnerIds(scope, accountKey);
  const people = await peopleById(db, scope.viewerId, ids);
  return {
    ...summary!,
    members: ids.map((id) => personOrUnknown(people, id, scope.viewerId)),
  };
}

async function summarize(
  db: Db,
  scope: AccessScope,
  keys: string[],
  now: Date,
): Promise<AccountSummary[]> {
  const visible = ownedRowWhere(scope);
  const inKeys = { accountKey: { in: keys } };
  const since7 = daysBefore(now, 7);
  const since30 = daysBefore(now, 30);

  // Whose reports may describe these accounts, and whose windows rank first: the viewer's, and
  // the creators' of the pools the viewer is in on them.
  const trustedOn = (key: string) =>
    new Set([scope.viewerId, ...(scope.poolCreators.get(key) ?? [])]);
  const reporters = [
    scope.viewerId,
    ...keys.flatMap((key) => scope.poolCreators.get(key) ?? []),
  ];
  // Sync keeps each person to SYNC_QUOTAS.windowsPerAccount window ids per account, so this is
  // how many (account, person, window) groups there can be; the limit holds it even for rows
  // stored before that cap.
  const groupLimit =
    keys.reduce((sum, key) => sum + visibleOwnerIds(scope, key).length, 0) *
    SYNC_QUOTAS.windowsPerAccount;

  const [reports, latestPerWindow, totals7, totals30, activity] =
    await Promise.all([
      db.userAccount.findMany({
        where: {
          accountKey: { in: keys },
          userId: { in: [...new Set(reporters)] },
        },
      }),
      db.usageReading.groupBy({
        by: ["accountKey", "userId", "windowId"],
        where: {
          AND: [
            visible,
            inKeys,
            {
              observedAt: {
                gte: daysBefore(now, USAGE_LOOKBACK_DAYS),
                lte: new Date(now.getTime() + USAGE_FUTURE_SKEW_MS),
              },
            },
          ],
        },
        _max: { observedAt: true },
        // Were the limit ever reached, the ids last in the alphabet would be the ones left out
        // (the fixed windows sort early).
        orderBy: [
          { windowId: "asc" },
          { accountKey: "asc" },
          { userId: "asc" },
        ],
        take: groupLimit,
      }),
      db.session.groupBy({
        by: ["accountKey"],
        where: { AND: [visible, inKeys, { startedAt: { gte: since7 } }] },
        _count: { _all: true },
        _sum: SESSION_SUMS,
      }),
      db.session.groupBy({
        by: ["accountKey"],
        where: { AND: [visible, inKeys, { startedAt: { gte: since30 } }] },
        _count: { _all: true },
        _sum: SESSION_SUMS,
      }),
      db.session.groupBy({
        by: ["accountKey"],
        where: { AND: [visible, inKeys] },
        _max: { lastActivityAt: true },
      }),
    ]);

  // Per account: which windows get a meter (rankMeterWindows), and for each, whose reading is
  // the newest. Then one lookup of exactly those readings: at most one per source for each.
  const meterOrder = new Map<string, string[]>();
  const latestWanted = keys.flatMap((key) => {
    const trusted = trustedOn(key);
    const newest = new Map<
      string,
      { userId: string; observedAt: Date; trusted: boolean }
    >();
    for (const row of latestPerWindow) {
      const observedAt = row._max.observedAt;
      if (row.accountKey !== key || !observedAt) continue;
      const seen = newest.get(row.windowId);
      const latest =
        seen && seen.observedAt >= observedAt
          ? seen
          : { userId: row.userId, observedAt };
      newest.set(row.windowId, {
        userId: latest.userId,
        observedAt: latest.observedAt,
        trusted: (seen?.trusted ?? false) || trusted.has(row.userId),
      });
    }
    const order = rankMeterWindows(
      [...newest].map(([windowId, w]) => ({ windowId, trusted: w.trusted })),
    );
    meterOrder.set(key, order);
    return order.map((windowId) => ({
      accountKey: key,
      windowId,
      userId: newest.get(windowId)!.userId,
      observedAt: newest.get(windowId)!.observedAt,
    }));
  });
  const latestRows =
    latestWanted.length === 0
      ? []
      : await db.usageReading.findMany({
          where: { AND: [visible, { OR: latestWanted }] },
          orderBy: [{ observedAt: "desc" }, { id: "desc" }],
          take: latestWanted.length * usageSourceSchema.options.length,
        });

  return keys.map((key) => {
    const own = reports.find(
      (r) => r.accountKey === key && r.userId === scope.viewerId,
    );
    // Prefer the viewer's own report; otherwise the creator's of the (oldest) pool they're in on
    // it. A label is one person's private name for the account, so only the viewer's own is shown.
    const creatorReport = (scope.poolCreators.get(key) ?? [])
      .map((creator) =>
        reports.find((r) => r.accountKey === key && r.userId === creator),
      )
      .find((r) => r !== undefined);
    const report = own ?? creatorReport;

    const usage = new Map<string, UsageWindowReading>();
    for (const row of latestRows) {
      if (row.accountKey !== key || usage.has(row.windowId)) continue;
      usage.set(row.windowId, {
        windowId: row.windowId,
        utilization: row.utilization,
        resetsAt: row.resetsAt,
        observedAt: row.observedAt,
        source: row.source,
      });
    }

    return {
      key,
      email: report?.email ?? null,
      organizationName: report?.organizationName ?? null,
      plan: report?.plan ?? null,
      label: own?.label ?? null,
      syncedByViewer: own !== undefined,
      lastSyncedAt: own?.lastSeenAt ?? null,
      pooled: isPooledWithOthers(scope, key),
      poolIds: [...(scope.poolIds.get(key) ?? [])],
      memberCount: visibleOwnerIds(scope, key).length,
      usage: (meterOrder.get(key) ?? []).flatMap((windowId) => {
        const reading = usage.get(windowId);
        return reading ? [reading] : [];
      }),
      last7Days: period(totals7.find((t) => t.accountKey === key)),
      last30Days: period(totals30.find((t) => t.accountKey === key)),
      lastActivityAt:
        activity.find((a) => a.accountKey === key)?._max.lastActivityAt ?? null,
    };
  });
}

function period(
  row:
    | {
        _count: { _all: number };
        _sum: Parameters<typeof tokenTotals>[0] & {
          costUsd: Parameters<typeof decimalToNumber>[0];
        };
      }
    | undefined,
): PeriodTotals {
  return {
    sessions: row?._count._all ?? 0,
    tokens: tokenTotals(row?._sum),
    costUsd: decimalToNumber(row?._sum.costUsd),
  };
}
