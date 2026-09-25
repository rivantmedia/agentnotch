/**
 * Which identity a request carries, before any database work. Pure over its inputs, so the
 * precedence rules can be tested without Supabase or Postgres.
 *
 * - `Authorization: Bearer <token>` wins. A bearer that fails verification is final: there is no
 *   fallback to cookies, so a stale app token can't be "rescued" by a browser session.
 * - Any other `Authorization` header is refused outright.
 * - Otherwise the Supabase session cookie, when the caller accepts cookies at all (the Mac app's
 *   API does not).
 */
import {
  bearerToken,
  type BearerVerifier,
  type TokenIdentity,
} from "~/server/auth/verify-bearer";

export type IdentityVia = "bearer" | "cookie";

export type IdentitySources = {
  verifyBearer: BearerVerifier;
  /** Absent: cookies are not accepted. */
  cookieIdentity?: (headers: Headers) => Promise<TokenIdentity | null>;
};

export async function resolveIdentity(
  headers: Headers,
  sources: IdentitySources,
): Promise<{ identity: TokenIdentity; via: IdentityVia } | null> {
  const token = bearerToken(headers);
  if (token) {
    const identity = await sources.verifyBearer(token);
    return identity ? { identity, via: "bearer" } : null;
  }
  if (headers.has("authorization")) return null;
  if (!sources.cookieIdentity) return null;
  const identity = await sources.cookieIdentity(headers);
  return identity ? { identity, via: "cookie" } : null;
}
