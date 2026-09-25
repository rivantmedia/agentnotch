/**
 * Who is making this request: the one place the website resolves a user.
 *
 * - `Authorization: Bearer <Supabase access token>` (the Mac app) is verified locally against the
 *   project's keys (verify-bearer.ts). A bearer that fails verification is final.
 * - Otherwise the Supabase session cookie (the website) is verified with `getClaims()`. The
 *   middleware refreshes that session before any page, route or tRPC call runs, so nothing here
 *   writes cookies. `getSession()` is never trusted on the server.
 *
 * Either way the user's row is created or refreshed (id = the token's `sub`).
 */
import "server-only";

import { createServerClient, parseCookieHeader } from "@supabase/ssr";

import { env } from "~/env";
import {
  resolveIdentity,
  type IdentityVia,
} from "~/server/auth/resolve-identity";
import {
  createBearerVerifier,
  identityFromClaims,
  type BearerVerifier,
  type TokenIdentity,
} from "~/server/auth/verify-bearer";
import { db } from "~/server/db";
import { ensureUser, type ViewerUser } from "~/server/services/users";

export type Viewer = ViewerUser & { via: IdentityVia };

let verifier: BearerVerifier | undefined;
function defaultVerifier(): BearerVerifier {
  verifier ??= createBearerVerifier({
    supabaseUrl: env.NEXT_PUBLIC_SUPABASE_URL,
    jwtSecret: env.SUPABASE_JWT_SECRET,
  });
  return verifier;
}

async function cookieIdentity(headers: Headers): Promise<TokenIdentity | null> {
  const cookieHeader = headers.get("cookie");
  if (!cookieHeader) return null;
  const supabase = createServerClient(
    env.NEXT_PUBLIC_SUPABASE_URL,
    env.NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY,
    {
      cookies: {
        getAll: () => parseCookieHeader(cookieHeader),
        setAll: () => {
          // Read-only: the middleware already refreshed the session for this request.
        },
      },
    },
  );
  const { data, error } = await supabase.auth.getClaims();
  if (error || !data) return null;
  return identityFromClaims(data.claims);
}

/**
 * The signed-in user, or null. `allowCookie: false` accepts bearer tokens only (the Mac app's
 * API, whose contract takes nothing else).
 */
export async function getViewer(
  headers: Headers,
  options: { allowCookie?: boolean } = {},
): Promise<Viewer | null> {
  const resolved = await resolveIdentity(headers, {
    verifyBearer: defaultVerifier(),
    cookieIdentity: options.allowCookie === false ? undefined : cookieIdentity,
  });
  if (!resolved) return null;
  const user = await ensureUser(db, resolved.identity);
  return { ...user, via: resolved.via };
}
