/**
 * The site's own address, as src/env.js reports it (`env.NEXT_PUBLIC_SITE_URL`): what the app
 * API names as the dashboard, and whose origin the Release workflow's tokens must be meant for.
 *
 * Plain JavaScript because src/env.js imports it, and next.config.js loads src/env.js without
 * compiling TypeScript.
 */

/**
 * `configured` (NEXT_PUBLIC_SITE_URL) when it is set: an override for local development,
 * self-hosting, or a Vercel project whose production domain isn't the site's address. It is
 * passed on as given, only trimmed and without trailing slashes, so a wrong value still fails
 * src/env.js's check instead of being replaced quietly.
 *
 * Otherwise `https://` and the first usable production domain Vercel gives. With the project's
 * "Enable access to System Environment Variables" on (the default), Vercel sets
 * `VERCEL_PROJECT_PRODUCTION_URL` on every deployment, previews included, at build and at run
 * time, and a Next.js build also gets it as `NEXT_PUBLIC_VERCEL_PROJECT_PRODUCTION_URL` (the copy
 * that reaches browser code). It is the project's shortest production custom domain, else its
 * vercel.app domain, without a scheme (`my-site.com`). A value that isn't a plain domain is
 * skipped.
 *
 * Otherwise undefined: the app API then answers with the address each request came in on.
 *
 * @param {string | undefined} configured NEXT_PUBLIC_SITE_URL
 * @param {...(string | undefined)} productionDomains Vercel's production domain, from each
 *   variable that may hold it, in order of preference
 * @returns {string | undefined}
 */
export function siteUrlFrom(configured, ...productionDomains) {
  const set = configured?.trim();
  // Slashes alone stay as they are, so the check refuses them rather than finding the value blank.
  if (set) return set.replace(/\/+$/, "") || set;
  for (const domain of productionDomains) {
    const origin = productionOrigin(domain);
    if (origin) return origin;
  }
  return undefined;
}

/**
 * `https://<domain>` for a bare domain, lowercased, without a path, a trailing slash or a
 * default port. A value that names its scheme keeps an http(s) one. Undefined for a blank value,
 * another scheme (whose origin is the string "null"), a user name or password, or no host.
 *
 * @param {string | undefined} value
 * @returns {string | undefined}
 */
function productionOrigin(value) {
  const text = value?.trim();
  if (!text) return undefined;
  // `://`, not just `:`, so `host:port` isn't read as a scheme.
  const hasScheme = /^[a-z][a-z0-9+.-]*:\/\//i.test(text);
  let url;
  try {
    url = new URL(hasScheme ? text : `https://${text}`);
  } catch {
    return undefined;
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") return undefined;
  if (!url.hostname || url.username || url.password) return undefined;
  return url.origin;
}
