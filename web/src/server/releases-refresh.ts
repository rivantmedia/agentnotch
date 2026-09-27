/**
 * POST /api/releases/refresh (src/app/api/releases/refresh/route.ts): the Release workflow calls
 * it once it has published, with its job's GitHub OIDC token, so /download offers the new
 * release at once instead of within the 5 minutes the site caches GitHub's release list
 * (src/server/releases.ts). Only a run of this repository's release workflow on main may
 * (src/server/auth/verify-release-run.ts); anyone else gets 401 and clears nothing.
 */
import "server-only";

import { errorResponse, NO_STORE } from "~/server/app-api/errors";
import { TokenCheckUnavailable } from "~/server/auth/token-errors";
import { bearerToken } from "~/server/auth/verify-bearer";
import {
  createReleaseRunVerifier,
  type ReleaseRunVerifier,
} from "~/server/auth/verify-release-run";
import { RELEASES_CACHE_TAG } from "~/server/releases";

export type RefreshDeps = {
  /** Null when the site can't tell its own origin (NEXT_PUBLIC_SITE_URL), the tokens' audience. */
  verify: ReleaseRunVerifier | null;
  revalidate: (tag: string) => void;
};

export async function refreshReleases(
  request: Request,
  deps: RefreshDeps,
): Promise<Response> {
  if (!deps.verify) {
    return errorResponse(
      "INTERNAL",
      "The website doesn't know its own address (NEXT_PUBLIC_SITE_URL), so it can't check who is asking.",
      NO_STORE,
      503,
    );
  }
  const token = bearerToken(request.headers);
  if (!token) {
    return errorResponse(
      "UNAUTHORIZED",
      "Send the Release workflow's GitHub OIDC token as a bearer token.",
      NO_STORE,
    );
  }
  let accepted: boolean;
  try {
    accepted = await deps.verify(token);
  } catch (error) {
    if (!(error instanceof TokenCheckUnavailable)) throw error;
    return errorResponse(
      "INTERNAL",
      "GitHub's signing keys couldn't be checked just now.",
      { ...NO_STORE, "Retry-After": "30" },
      503,
    );
  }
  if (!accepted) {
    return errorResponse(
      "UNAUTHORIZED",
      "Only this repository's Release workflow, running on main, can refresh the releases.",
      NO_STORE,
    );
  }
  deps.revalidate(RELEASES_CACHE_TAG);
  return Response.json({ refreshed: true }, { headers: NO_STORE });
}

let verifier: { key: string; verify: ReleaseRunVerifier } | null = null;

/**
 * One verifier per origin and repository, kept across requests so GitHub's key set is fetched
 * once, not per call. Null when `siteUrl` isn't a web address: another scheme's origin is the
 * string "null", an audience any GitHub Actions job could ask for.
 */
export function releaseRunVerifier(
  siteUrl: string | undefined,
  repo: string,
): ReleaseRunVerifier | null {
  let audience: string;
  try {
    const url = new URL(siteUrl ?? "");
    if (url.protocol !== "https:" && url.protocol !== "http:") return null;
    audience = url.origin;
  } catch {
    return null;
  }
  const key = `${audience} ${repo}`;
  if (verifier?.key !== key) {
    verifier = { key, verify: createReleaseRunVerifier({ audience, repo }) };
  }
  return verifier.verify;
}
