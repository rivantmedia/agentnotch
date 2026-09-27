/**
 * Query inputs the project pages prefetch on the server and their client components read back.
 * Both sides build them here, so they match exactly and the browser doesn't fetch again.
 */
import { DEFAULT_USAGE_PERIOD, type UsagePeriod } from "~/lib/usage-period";

export const PROJECT_SESSIONS_PAGE_SIZE = 20;

const PROJECT_ID = /^[A-Za-z0-9_-]{1,128}$/;

/** Whether a path segment can be a project row id at all. */
export function isProjectId(id: string): boolean {
  return PROJECT_ID.test(id);
}

/** Every project with sessions in the period, across the viewer's accounts. */
export function allProjectsInput(period: UsagePeriod) {
  return { period };
}

export function projectDetailInput(id: string, period: UsagePeriod) {
  return { id, period };
}

/** The project's sessions on every account the viewer sees it on, newest first. */
export function projectSessionsInput(id: string) {
  return {
    projectId: id,
    acrossAccounts: true,
    limit: PROJECT_SESSIONS_PAGE_SIZE,
  };
}

/**
 * A project's sessions on one account: the account page filtered to the project's rows there,
 * scrolled to its sessions, over the same period.
 */
export function accountProjectHref(
  accountKey: string,
  projectId: string,
  period: UsagePeriod,
): string {
  const query = new URLSearchParams({ project: projectId });
  if (period !== DEFAULT_USAGE_PERIOD) query.set("period", period);
  return `/accounts/${accountKey}?${query.toString()}#sessions`;
}
