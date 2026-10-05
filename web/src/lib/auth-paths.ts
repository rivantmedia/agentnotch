/**
 * Which pages need a signed-in user, and where a sign-in may send the browser afterwards.
 * Pure, so the middleware, the login page and the OAuth callback all agree (and tests can say so).
 */

/** Page trees that need a signed-in user. Each covers the path and everything below it. */
export const PROTECTED_PREFIXES = [
  "/dashboard",
  "/accounts",
  "/usage",
  "/projects",
  "/pools",
  "/settings",
] as const;

export const DEFAULT_AFTER_LOGIN = "/dashboard";

export function isProtectedPath(pathname: string): boolean {
  return PROTECTED_PREFIXES.some(
    (prefix) => pathname === prefix || pathname.startsWith(`${prefix}/`),
  );
}

/**
 * `next` if it is a path on this site, else the fallback.
 *
 * Only a single leading slash followed by a path is accepted: `//evil.example`, `/\evil.example`
 * (browsers read both as another host), absolute URLs, control characters and anything that
 * resolves to another origin are refused, so a crafted sign-in link can't redirect off the site.
 */
export function safeNextPath(
  next: string | null | undefined,
  fallback: string = DEFAULT_AFTER_LOGIN,
): string {
  if (typeof next !== "string" || next.length === 0 || next.length > 2048) {
    return fallback;
  }
  if (
    !next.startsWith("/") ||
    next.startsWith("//") ||
    next.startsWith("/\\")
  ) {
    return fallback;
  }
  if (/[\u0000-\u001f\u007f\\]/.test(next)) return fallback;
  const base = "https://agentnotch.invalid";
  let url: URL;
  try {
    url = new URL(next, base);
  } catch {
    return fallback;
  }
  if (url.origin !== base) return fallback;
  const path = `${url.pathname}${url.search}${url.hash}`;
  // Dot segments can normalise into `//host` ("/..//evil.example"); a relative redirect there
  // would leave the site.
  return path.startsWith("//") ? fallback : path;
}
