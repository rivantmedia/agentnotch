/**
 * Query inputs the usage page prefetches on the server and its client components read back.
 * Both sides build them here from the address bar (period, `?accounts=`), so they match exactly
 * and the browser doesn't fetch again.
 *
 * A selection of no accounts asks for nothing: the page says so instead (the API takes at least
 * one key). Keys the viewer doesn't see are sent as they are, and the server leaves them out.
 */
import { type AccountSelection } from "~/lib/account-selection";
import { PIE_SLOTS } from "~/lib/pie-chart";
import { type UsagePeriod } from "~/lib/usage-period";

/** Projects the page lists by name, one per slice of its pie; the rest are added up. */
export const USAGE_PROJECTS_LIMIT = PIE_SLOTS;

/** Whether a selection names any account to ask about (every account counts). */
export function asksForAccounts(selection: AccountSelection): boolean {
  return selection === null || selection.length > 0;
}

function accounts(selection: AccountSelection) {
  return selection === null ? {} : { accountKeys: [...selection] };
}

export function combinedUsageInput(
  period: UsagePeriod,
  selection: AccountSelection,
) {
  return { period, ...accounts(selection) };
}

/** Over time in the viewer's calendar: only the browser knows its zone, so it alone asks. */
export function usageTimelineInput(
  period: UsagePeriod,
  selection: AccountSelection,
  timeZone: string,
) {
  return { period, ...accounts(selection), timeZone };
}

export function usageProjectsInput(
  period: UsagePeriod,
  selection: AccountSelection,
) {
  return { period, ...accounts(selection), limit: USAGE_PROJECTS_LIMIT };
}
