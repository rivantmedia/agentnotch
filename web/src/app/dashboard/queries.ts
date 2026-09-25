/**
 * Query inputs a page prefetches on the server and its client components read back. They must
 * be identical on both sides, or the client fetches again.
 */
export const RECENT_SESSIONS_INPUT = { limit: 8 } as const;
