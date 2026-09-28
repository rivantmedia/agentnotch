/**
 * The Mac app's API (contract/README.md), as plain functions of a Request and their
 * dependencies. The route files under src/app/api/app/v1 pass the real ones; tests pass fakes.
 */
import { requestOrigin } from "~/lib/site-address";
import { TokenCheckUnavailable } from "~/server/auth/token-errors";

import { readJsonBody } from "./body";
import {
  AppApiError,
  describeZodError,
  NO_STORE,
  toErrorResponse,
  tokenCheckUnavailable,
} from "./errors";
import { type RateLimiter } from "./rate-limit";
import {
  APP_REDIRECT_URL,
  LIMITS,
  makeSyncRequestSchema,
  type ConfigResponse,
  type MeResponse,
  type SyncRequest,
  type SyncResponse,
} from "./schema";

export type AppViewer = { id: string; email: string; name: string | null };

export type AppApiDeps = {
  /**
   * Bearer-token users only. Null for a bad token (401); throws TokenCheckUnavailable when the
   * token couldn't be checked right now (503).
   */
  resolveViewer: (headers: Headers) => Promise<AppViewer | null>;
  applySync: (
    viewer: AppViewer,
    request: SyncRequest,
  ) => Promise<{ sessions: number; usage: number }>;
  /** Per user, keyed by their id. */
  limiter: RateLimiter;
  /** Per client IP address, keyed by `clientIpKey`. */
  ipLimiter: RateLimiter;
  /** The key of the caller's IP address (a hash, never the address), or null when unknown. */
  clientIpKey: (headers: Headers) => string | null;
  /** The site's address (src/env.js); unset, the address the request came in on. */
  siteUrl: string | undefined;
  now?: () => Date;
  /** Where unexpected errors go (they reach the client only as INTERNAL). */
  logError?: (context: string, error: unknown) => void;
};

export function dashboardUrl(siteUrl: string): string {
  return `${siteUrl.replace(/\/+$/, "")}/dashboard`;
}

/**
 * The site's address, or, when src/env.js has none (off Vercel without NEXT_PUBLIC_SITE_URL),
 * the one this request came in on: the address the app reached the site at.
 */
export function siteUrlFor(
  siteUrl: string | undefined,
  request: Request,
): string {
  return siteUrl ?? requestOrigin(request.headers, new URL(request.url).origin);
}

export function buildConfig(options: {
  supabaseUrl: string;
  supabasePublishableKey: string;
  siteUrl: string;
}): ConfigResponse {
  return {
    supabaseUrl: options.supabaseUrl.replace(/\/+$/, ""),
    supabasePublishableKey: options.supabasePublishableKey,
    redirectUrl: APP_REDIRECT_URL,
    dashboardUrl: dashboardUrl(options.siteUrl),
  };
}

/** GET /api/app/v1/config: what the app needs to start a sign-in. No auth. */
export function handleConfig(
  request: Request,
  options: {
    supabaseUrl: string;
    supabasePublishableKey: string;
    /** The site's address (src/env.js); unset, the address the request came in on. */
    siteUrl: string | undefined;
  },
): Response {
  return Response.json(
    buildConfig({ ...options, siteUrl: siteUrlFor(options.siteUrl, request) }),
    {
      headers: {
        // An answer built from the request's own Host / X-Forwarded-* headers is only for the
        // client that sent them, never for a shared cache.
        "Cache-Control": `${options.siteUrl ? "public" : "private"}, max-age=300`,
      },
    },
  );
}

export async function handleMe(
  request: Request,
  deps: AppApiDeps,
): Promise<Response> {
  return guard("me", deps, async () => {
    const viewer = await requireViewer(request, deps);
    const body: MeResponse = {
      user: { id: viewer.id, email: viewer.email, name: viewer.name },
      dashboardUrl: dashboardUrl(siteUrlFor(deps.siteUrl, request)),
    };
    return Response.json(body, { headers: NO_STORE });
  });
}

export async function handleSync(
  request: Request,
  deps: AppApiDeps,
): Promise<Response> {
  return guard("sync", deps, async () => {
    const viewer = await requireViewer(request, deps);

    // Before reading the body, so a runaway client costs as little as possible. The user's own
    // limit first: someone over it doesn't use up what their network shares with others.
    const limit = await deps.limiter.take(viewer.id);
    if (!limit.ok) throw tooManySyncs(limit.retryAfterSeconds, "");
    // Without a known address, only the user's limit applies.
    const ipKey = deps.clientIpKey(request.headers);
    if (ipKey !== null) {
      const ipLimit = await deps.ipLimiter.take(ipKey);
      if (!ipLimit.ok) {
        throw tooManySyncs(ipLimit.retryAfterSeconds, " from this network");
      }
    }

    const raw = await readJsonBody(request, LIMITS.bodyBytes);
    // Dates are bounded by this server's clock (contract/README.md).
    const schema = makeSyncRequestSchema(() => now(deps).getTime());
    const parsed = schema.safeParse(raw);
    if (!parsed.success) {
      throw new AppApiError("BAD_REQUEST", describeZodError(parsed.error));
    }

    const accepted = await deps.applySync(viewer, parsed.data);
    const body: SyncResponse = {
      accepted,
      serverTime: now(deps).toISOString(),
    };
    return Response.json(body, { headers: NO_STORE });
  });
}

function tooManySyncs(retryAfterSeconds: number, from: string): AppApiError {
  return new AppApiError(
    "RATE_LIMITED",
    `Too many syncs${from}. Try again in ${retryAfterSeconds} s.`,
    { "Retry-After": String(retryAfterSeconds) },
  );
}

function now(deps: AppApiDeps): Date {
  return deps.now?.() ?? new Date();
}

async function requireViewer(
  request: Request,
  deps: AppApiDeps,
): Promise<AppViewer> {
  let viewer: AppViewer | null;
  try {
    viewer = await deps.resolveViewer(request.headers);
  } catch (error) {
    // Not the token's fault: never answer 401 for it, or the app would sign out.
    if (error instanceof TokenCheckUnavailable) {
      deps.logError?.("token check", error);
      throw tokenCheckUnavailable();
    }
    throw error;
  }
  if (!viewer) throw new AppApiError("UNAUTHORIZED", "Sign in again.");
  return viewer;
}

async function guard(
  context: string,
  deps: AppApiDeps,
  run: () => Promise<Response>,
): Promise<Response> {
  try {
    return await run();
  } catch (error) {
    if (!(error instanceof AppApiError)) deps.logError?.(context, error);
    return toErrorResponse(error);
  }
}
