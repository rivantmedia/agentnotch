/**
 * Verifies the OIDC token the Release workflow sends to POST /api/releases/refresh, so that only
 * a publishing run can clear the site's cached copy of GitHub's release list. GitHub's token
 * service signs one per job on request; nothing is shared with the website ahead of time.
 *
 * The token must be signed by GitHub (its key set), be meant for this site (`aud`, the site's
 * origin), and name this repository, its main branch, the release workflow and the `release`
 * environment, which only main can deploy to. Outcomes follow verify-bearer.ts: a bad token is
 * false (401); a check that couldn't happen throws TokenCheckUnavailable (503).
 */
import {
  createRemoteJWKSet,
  decodeProtectedHeader,
  jwtVerify,
  type JWTPayload,
  type JWTVerifyGetKey,
} from "jose";

import { rejectOrUnavailable } from "~/server/auth/verify-bearer";

export const GITHUB_ACTIONS_ISSUER =
  "https://token.actions.githubusercontent.com";
const RELEASE_WORKFLOW = ".github/workflows/release.yml";
const RELEASE_ENVIRONMENT = "release";
const MAIN = "refs/heads/main";

export type ReleaseRunVerifier = (token: string) => Promise<boolean>;

export type ReleaseRunVerifierOptions = {
  /** The site's origin, e.g. https://agentnotch.rivant.in: the audience the run asks for. */
  audience: string;
  /** `owner/name` of the repository whose releases the site offers. */
  repo: string;
  /** Key source; defaults to GitHub's (see verify-bearer.ts on `coolingDown`). */
  jwks?: JWTVerifyGetKey & { readonly coolingDown?: boolean };
  clockToleranceSeconds?: number;
};

export function createReleaseRunVerifier(
  options: ReleaseRunVerifierOptions,
): ReleaseRunVerifier {
  const jwks =
    options.jwks ??
    createRemoteJWKSet(new URL(`${GITHUB_ACTIONS_ISSUER}/.well-known/jwks`));
  return async (token) => {
    // GitHub signs RS256 with a key id and no critical extensions. Anything else is no release
    // run's token and never reaches the key set, whose failures would read as "try again".
    try {
      const header = decodeProtectedHeader(token);
      if (
        header.alg !== "RS256" ||
        typeof header.kid !== "string" ||
        header.crit !== undefined
      ) {
        return false;
      }
    } catch {
      return false;
    }
    const keySetWasCurrent = jwks.coolingDown !== true;
    try {
      const { payload } = await jwtVerify(token, jwks, {
        issuer: GITHUB_ACTIONS_ISSUER,
        audience: options.audience,
        algorithms: ["RS256"],
        clockTolerance: options.clockToleranceSeconds ?? 30,
        requiredClaims: ["exp", "iat"],
      });
      return isReleaseRun(payload, options.repo);
    } catch (error) {
      return rejectOrUnavailable(error, { keySetWasCurrent }) ?? false;
    }
  };
}

/**
 * Whether verified claims describe a run of `repo`'s release workflow on main, in its release
 * environment. The repository compares without case, as GitHub's names do; the workflow must be
 * exactly `<repository>/.github/workflows/release.yml@refs/heads/main`, for the run and for the
 * job (which is no reusable workflow's).
 */
export function isReleaseRun(
  claims: JWTPayload | Record<string, unknown>,
  repo: string,
): boolean {
  const text = (value: unknown) => (typeof value === "string" ? value : "");
  const repository = text(claims.repository);
  if (repository === "" || repository.toLowerCase() !== repo.toLowerCase()) {
    return false;
  }
  const workflow = `${repository}/${RELEASE_WORKFLOW}@${MAIN}`;
  return (
    text(claims.ref) === MAIN &&
    text(claims.workflow_ref) === workflow &&
    text(claims.job_workflow_ref) === workflow &&
    text(claims.environment) === RELEASE_ENVIRONMENT
  );
}
