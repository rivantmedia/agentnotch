/**
 * Headers every response of the site carries (next.config.js, `headers()`).
 *
 * - No page may be framed by another, so its buttons (Leave, Revoke, Remove on /pools) can't be
 *   clickjacked: CSP `frame-ancestors 'none'` for current browsers, X-Frame-Options for old ones.
 * - Browsers must not guess content types, and links out of the site carry only the origin.
 *
 * Plain JS, so next.config.js can import it and the unit tests can check it.
 */

/** @type {ReadonlyArray<{ key: string; value: string }>} */
export const SECURITY_HEADERS = Object.freeze([
  { key: "X-Frame-Options", value: "DENY" },
  { key: "Content-Security-Policy", value: "frame-ancestors 'none'" },
  { key: "X-Content-Type-Options", value: "nosniff" },
  { key: "Referrer-Policy", value: "strict-origin-when-cross-origin" },
]);

/** Every path: pages, route handlers, the Mac app's API and static files. */
export const SECURITY_HEADERS_SOURCE = "/:path*";

/** What `headers()` in next.config.js returns. */
export function securityHeaderRules() {
  return [
    {
      source: SECURITY_HEADERS_SOURCE,
      headers: SECURITY_HEADERS.map((header) => ({ ...header })),
    },
  ];
}
