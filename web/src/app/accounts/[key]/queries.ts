/**
 * Query inputs the account page prefetches on the server and its client components read back.
 * Both sides build them here, so they match exactly and the browser doesn't fetch again.
 */

export const SESSIONS_PAGE_SIZE = 20;

/** How far back the usage charts go. */
export const USAGE_HISTORY_DAYS = 30;

export type SessionFilter = {
  projectId?: string;
  ownerId?: string;
};

const ID = /^[A-Za-z0-9_-]{1,128}$/;

/** A filter from the page's search params (`?project=…&member=…`); anything malformed is dropped. */
export function filterFromParams(params: {
  project?: string | string[];
  member?: string | string[];
}): SessionFilter {
  const project = first(params.project);
  const member = first(params.member);
  return {
    projectId: project && ID.test(project) ? project : undefined,
    ownerId: member && ID.test(member) ? member : undefined,
  };
}

export function sessionsInput(accountKey: string, filter: SessionFilter) {
  return {
    accountKey,
    ...(filter.projectId ? { projectId: filter.projectId } : {}),
    ...(filter.ownerId ? { ownerId: filter.ownerId } : {}),
    limit: SESSIONS_PAGE_SIZE,
  };
}

/** The start of the usage charts: USAGE_HISTORY_DAYS ago, on the hour so reloads share it. */
export function usageFrom(now: number): number {
  const hour = 60 * 60 * 1000;
  return Math.floor((now - USAGE_HISTORY_DAYS * 24 * hour) / hour) * hour;
}

export function usageInput(accountKey: string, fromMs: number) {
  return { accountKey, from: new Date(fromMs) };
}

function first(value: string | string[] | undefined): string | undefined {
  return Array.isArray(value) ? value[0] : value;
}
