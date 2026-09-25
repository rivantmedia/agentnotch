/**
 * The real dependencies of the app API handlers.
 */
import "server-only";

import { env } from "~/env";
import { getViewer } from "~/server/auth/viewer";
import { db } from "~/server/db";
import { createDbRateLimiter } from "~/server/services/rate-limits";
import { applySync } from "~/server/services/sync";

import { type AppApiDeps } from "./handlers";
import { SYNC_RATE } from "./rate-limit";

// Kept in the database, so every server instance shares each user's bucket.
const syncLimiter = createDbRateLimiter(db, { name: "sync", ...SYNC_RATE });

export function appApiDeps(): AppApiDeps {
  return {
    // The contract takes bearer tokens only; a browser's cookies never authorize these routes.
    resolveViewer: (headers) => getViewer(headers, { allowCookie: false }),
    applySync: (viewer, request) => applySync(db, viewer.id, request),
    limiter: syncLimiter,
    siteUrl: env.NEXT_PUBLIC_SITE_URL,
    logError: (context, error) => {
      console.error(`[app-api] ${context} failed`, error);
    },
  };
}
