/**
 * The address people paste into the Mac app (Settings > Claude Code > Cloud). The app calls
 * `<address>/api/app/v1/config`, so it is the site's origin (plus a base path, if the site has
 * one), without a trailing slash.
 */

const HOST = /^(?:[a-z0-9-]+(?:\.[a-z0-9-]+)*|\[[0-9a-f:.]+\])(?::\d{1,5})?$/i;
const LOCAL = /^(?:localhost|127\.0\.0\.1|\[::1\])(?::\d+)?$/i;

/**
 * `NEXT_PUBLIC_SITE_URL` when it is set (what the app API reports too); otherwise the address
 * this request came in on, from the Host / X-Forwarded-* headers. Null when neither is usable.
 */
export function siteAddressFrom(
  configured: string | null | undefined,
  headers: Pick<Headers, "get">,
): string | null {
  const fromEnv = configured?.trim();
  if (fromEnv) {
    try {
      const url = new URL(fromEnv);
      if (url.protocol === "https:" || url.protocol === "http:") {
        return `${url.origin}${url.pathname}`.replace(/\/+$/, "");
      }
    } catch {
      // Fall through to the request's own address.
    }
  }

  const host = firstValue(
    headers.get("x-forwarded-host") ?? headers.get("host"),
  );
  if (!host || !HOST.test(host)) return null;
  const forwarded = firstValue(headers.get("x-forwarded-proto"))?.toLowerCase();
  const proto =
    forwarded === "http" || forwarded === "https"
      ? forwarded
      : LOCAL.test(host)
        ? "http"
        : "https";
  return `${proto}://${host.toLowerCase()}`;
}

/**
 * The origin the browser used for this request, for building redirects back to it.
 *
 * Next.js gives middleware and route handlers the address the server listens on: right on
 * Vercel, but `http://localhost:3000` under `next start` behind a proxy, which would send people
 * off the site after signing in. The Host / X-Forwarded-* headers say what the browser asked
 * for; `fallback` (the request's own origin) is used when they don't hold a plain host. The
 * headers are the requester's own, so at worst a client redirects itself, and these responses
 * are never cached.
 */
export function requestOrigin(
  headers: Pick<Headers, "get">,
  fallback: string,
): string {
  const host = firstValue(
    headers.get("x-forwarded-host") ?? headers.get("host"),
  );
  if (!host || !HOST.test(host)) return fallback;
  const forwarded = firstValue(headers.get("x-forwarded-proto"))?.toLowerCase();
  let proto: string;
  if (forwarded === "http" || forwarded === "https") {
    proto = forwarded;
  } else {
    try {
      proto = new URL(fallback).protocol.replace(/:$/, "");
    } catch {
      proto = LOCAL.test(host) ? "http" : "https";
    }
  }
  return `${proto}://${host.toLowerCase()}`;
}

/** Proxies may send a comma-separated chain; the first entry is the client-facing one. */
function firstValue(value: string | null | undefined): string | null {
  const first = value?.split(",")[0]?.trim();
  return first === undefined || first === "" ? null : first;
}
