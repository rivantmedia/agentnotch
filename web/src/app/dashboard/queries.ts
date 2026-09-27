/**
 * Query inputs a page prefetches on the server and its client components read back. They must
 * be identical on both sides, or the client fetches again.
 */
import { PIE_SLOTS } from "~/lib/pie-chart";
import { type UsagePeriod } from "~/lib/usage-period";

export const RECENT_SESSIONS_INPUT = { limit: 8 } as const;

/**
 * Projects the dashboard lists by name, one per slice of its pie; the rest are added up in one
 * row, the pie's other slice.
 */
export const DASHBOARD_PROJECTS_LIMIT = PIE_SLOTS;

export function dashboardProjectsInput(period: UsagePeriod) {
  return { period, limit: DASHBOARD_PROJECTS_LIMIT };
}
