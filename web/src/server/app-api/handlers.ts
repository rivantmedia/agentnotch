/**
 * The Mac app's API (contract/README.md), as plain functions of a Request and their
 * dependencies. The route files under src/app/api/app/v1 pass the real ones; tests pass fakes.
 */
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
  limiter: RateLimiter;
  siteUrl: string;
  now?: () => Date;
  /** Where unexpected errors go (they reach the client only as INTERNAL). */
  logError?: (context: string, error: unknown) => void;
};

export function dashboardUrl(siteUrl: string): string {
  return `${siteUrl.replace(/\/+$/, "")}/dashboard`;
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

export async function handleMe(
  request: Request,
  deps: AppApiDeps,
): Promise<Response> {
  return guard("me", deps, async () => {
    const viewer = await requireViewer(request, deps);
    const body: MeResponse = {
      user: { id: viewer.id, email: viewer.email, name: viewer.name },
      dashboardUrl: dashboardUrl(deps.siteUrl),
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

    // Before reading the body, so a runaway client costs as little as possible.
    const limit = await deps.limiter.take(viewer.id);
    if (!limit.ok) {
      throw new AppApiError(
        "RATE_LIMITED",
        `Too many syncs. Try again in ${limit.retryAfterSeconds} s.`,
        { "Retry-After": String(limit.retryAfterSeconds) },
      );
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
