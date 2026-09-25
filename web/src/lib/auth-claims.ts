/**
 * Which verified Supabase sessions this site accepts: a real (not anonymous) user who signed in
 * with Google. Pure, so the middleware, the login page and the server's identity check all agree.
 *
 * The provider comes from `app_metadata`, which only Supabase itself can write (a user can edit
 * `user_metadata`, never `app_metadata`). Only Google is accepted because the email it vouches
 * for is what pool members are shown to each other by; a Supabase project that also has email
 * sign-up turned on would otherwise let anyone in under any address.
 */
export const ACCEPTED_PROVIDER = "google";

export function signInProviderIsAccepted(
  claims: Record<string, unknown>,
): boolean {
  const meta = claims.app_metadata;
  if (typeof meta !== "object" || meta === null || Array.isArray(meta)) {
    return false;
  }
  const { provider, providers } = meta as Record<string, unknown>;
  if (provider === ACCEPTED_PROVIDER) return true;
  return Array.isArray(providers) && providers.includes(ACCEPTED_PROVIDER);
}

/** Whether a verified token's claims describe a person this site lets in. */
export function isAcceptedSignIn(
  claims: Record<string, unknown> | null | undefined,
): boolean {
  if (!claims) return false;
  const sub = claims.sub;
  if (typeof sub !== "string" || sub.length === 0 || sub.length > 128)
    return false;
  if (claims.is_anonymous === true) return false;
  return signInProviderIsAccepted(claims);
}
