/**
 * Query inputs a page prefetches on the server and its client components read back. They must
 * be identical on both sides, or the client fetches again.
 */
import { type UsagePeriod } from "~/lib/usage-period";

export const RECENT_SESSIONS_INPUT = { limit: 8 } as const;

/** Projects the dashboard lists by name; the rest are added up in one row. */
export const DASHBOARD_PROJECTS_LIMIT = 8;

export function dashboardProjectsInput(period: UsagePeriod) {
  return { period, limit: DASHBOARD_PROJECTS_LIMIT };
}
