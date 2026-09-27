/**
 * The periods the usage breakdowns (by project, by account) add up. A session counts in a period
 * by its start, a start dated in the future counting as now, as in the accounts' 7- and 30-day
 * totals. Pure, so pages, client components and the server share it.
 */

export const USAGE_PERIODS = ["7d", "30d", "all"] as const;
export type UsagePeriod = (typeof USAGE_PERIODS)[number];

/** What a breakdown shows until the viewer picks another period: the account cards' week. */
export const DEFAULT_USAGE_PERIOD: UsagePeriod = "7d";

const DAYS: Record<UsagePeriod, number | null> = {
  "7d": 7,
  "30d": 30,
  all: null,
};

/** How many days back a period reaches; null for all time. */
export function periodDays(period: UsagePeriod): number | null {
  return DAYS[period];
}

/** The picker's words for a period. */
export function periodLabel(period: UsagePeriod): string {
  switch (period) {
    case "7d":
      return "7 days";
    case "30d":
      return "30 days";
    case "all":
      return "All time";
  }
}

/** A period inside a sentence: "in the last 7 days", "ever". */
export function periodPhrase(period: UsagePeriod): string {
  return period === "all" ? "ever" : `in the last ${periodLabel(period)}`;
}

export function isUsagePeriod(value: unknown): value is UsagePeriod {
  return (USAGE_PERIODS as readonly unknown[]).includes(value);
}

/** The period a page's `?period=` names; anything else is the default. */
export function periodFromParam(
  value: string | string[] | undefined,
): UsagePeriod {
  const first = Array.isArray(value) ? value[0] : value;
  return isUsagePeriod(first) ? first : DEFAULT_USAGE_PERIOD;
}

/** A page's address with `?period=`, left out for the default period. */
export function withPeriod(path: string, period: UsagePeriod): string {
  return period === DEFAULT_USAGE_PERIOD ? path : `${path}?period=${period}`;
}
