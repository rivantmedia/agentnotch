/**
 * Verifies a Supabase access token sent as `Authorization: Bearer …` (the Mac app).
 *
 * Asymmetric signing keys (ES256 / RS256) are checked against the project's JWKS. Projects still
 * on the legacy shared secret sign with HS256; that is accepted only when SUPABASE_JWT_SECRET is
 * configured. Nothing here trusts a claim before the signature, issuer, audience and expiry hold.
 *
 * Outcomes are kept apart on purpose, because the Mac app signs out on a 401:
 * - a bad, expired or foreign token resolves to null (401);
 * - a check that couldn't happen (the key set timed out or failed to load, or the token names a
 *   key we can't look up again yet) throws TokenCheckUnavailable (503), and the app retries.
 */
import {
  createRemoteJWKSet,
  decodeProtectedHeader,
  errors,
  jwtVerify,
  type JWTPayload,
  type JWTVerifyGetKey,
} from "jose";

import { isAcceptedSignIn } from "~/lib/auth-claims";
import { TokenCheckUnavailable } from "~/server/auth/token-errors";

/** Who a verified token says the user is. */
export type TokenIdentity = {
  id: string;
  email: string | null;
  name: string | null;
};

export type BearerVerifier = (token: string) => Promise<TokenIdentity | null>;

export type BearerVerifierOptions = {
  /** The project URL, e.g. https://abcdefghijklmnop.supabase.co */
  supabaseUrl: string;
  /**
   * Key source for asymmetric tokens. Defaults to the project's remote JWKS. A remote set exposes
   * `coolingDown` (jose's createRemoteJWKSet does); a source without it is treated as always
   * current, so a key it doesn't have is simply not a key of this project.
   */
  jwks?: JWTVerifyGetKey & { readonly coolingDown?: boolean };
  /** The legacy HS256 secret, when the project still uses one. */
  jwtSecret?: string;
  /** Allowed clock skew between the Mac, Supabase and this server, in seconds. */
  clockToleranceSeconds?: number;
};

const ASYMMETRIC_ALGORITHMS = ["ES256", "RS256"];
const AUDIENCE = "authenticated";

export function supabaseIssuer(supabaseUrl: string): string {
  return `${supabaseUrl.replace(/\/+$/, "")}/auth/v1`;
}

export function createBearerVerifier(
  options: BearerVerifierOptions,
): BearerVerifier {
  const issuer = supabaseIssuer(options.supabaseUrl);
  const jwks =
    options.jwks ??
    createRemoteJWKSet(new URL(`${issuer}/.well-known/jwks.json`));
  const secret = options.jwtSecret
    ? new TextEncoder().encode(options.jwtSecret)
    : null;
  const common = {
    issuer,
    audience: AUDIENCE,
    clockTolerance: options.clockToleranceSeconds ?? 30,
    requiredClaims: ["sub", "exp"],
  };

  return async (token) => {
    let alg: string | undefined;
    try {
      alg = decodeProtectedHeader(token).alg;
    } catch {
      return null;
    }
    if (alg === "HS256") {
      if (!secret) return null;
      try {
        const { payload } = await jwtVerify(token, secret, {
          ...common,
          algorithms: ["HS256"],
        });
        return identityFromClaims(payload);
      } catch (error) {
        return rejectOrUnavailable(error, { keySetWasCurrent: true });
      }
    }
    // jose refetches the key set when a token names an unknown key, except within 30 s of the
    // last fetch. Whether that refetch could happen decides what a miss means.
    const keySetWasCurrent = jwks.coolingDown !== true;
    try {
      const { payload } = await jwtVerify(token, jwks, {
        ...common,
        algorithms: ASYMMETRIC_ALGORITHMS,
      });
      return identityFromClaims(payload);
    } catch (error) {
      return rejectOrUnavailable(error, { keySetWasCurrent });
    }
  };
}

/** jose errors that are the token's own fault: the answer is "not signed in". */
const TOKEN_ERRORS = [
  errors.JWSSignatureVerificationFailed,
  errors.JWTExpired,
  errors.JWTClaimValidationFailed,
  errors.JWSInvalid,
  errors.JWTInvalid,
  errors.JOSEAlgNotAllowed,
  // Several keys fit a token without a key id and none verifies it.
  errors.JWKSMultipleMatchingKeys,
];

/**
 * null for a token that is bad; TokenCheckUnavailable for anything that stopped the check itself
 * (a key-set timeout, a failed or malformed fetch, or an unknown key id while jose won't refetch).
 * Also used for the Release workflow's tokens (verify-release-run.ts).
 */
export function rejectOrUnavailable(
  error: unknown,
  context: { keySetWasCurrent: boolean },
): null {
  if (TOKEN_ERRORS.some((type) => error instanceof type)) return null;
  // An unknown key id right after a fresh look at the key set: not one of this project's keys.
  if (error instanceof errors.JWKSNoMatchingKey && context.keySetWasCurrent) {
    return null;
  }
  throw new TokenCheckUnavailable(
    "The sign-in couldn't be checked right now.",
    { cause: error },
  );
}

/**
 * The user a verified Supabase access token (or `getClaims()` result) describes, or null when it
 * describes no one this site accepts: no `sub`, an anonymous sign-in, or a sign-in with any
 * provider other than Google (src/lib/auth-claims.ts).
 *
 * `name` comes from `user_metadata`, which the user can change at will; it is only ever shown
 * next to the email, never as the way someone is told apart (users.ts, `memberLabel`).
 */
export function identityFromClaims(
  claims: JWTPayload | Record<string, unknown>,
): TokenIdentity | null {
  if (!isAcceptedSignIn(claims)) return null;
  const sub = claims.sub as string;
  const email = typeof claims.email === "string" ? claims.email : null;
  const metadata = isRecord(claims.user_metadata) ? claims.user_metadata : {};
  const name = firstText(metadata.full_name, metadata.name);
  return { id: sub, email, name };
}

/** The token in `Authorization: Bearer <token>`, or null. */
export function bearerToken(headers: Headers): string | null {
  const value = headers.get("authorization");
  if (!value) return null;
  const match = /^Bearer\s+(\S+)\s*$/i.exec(value);
  return match?.[1] ?? null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function firstText(...values: unknown[]): string | null {
  for (const value of values) {
    if (typeof value === "string" && value.trim().length > 0) {
      return value.trim().slice(0, 200);
    }
  }
  return null;
}
