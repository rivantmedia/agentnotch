/**
 * The latest release of the app, from GitHub's API, for /download and its redirects.
 *
 * It lists the repository's releases instead of asking for `/releases/latest`: with no release
 * published yet, that answers 404, and Next caches only a 200, so every view would reach GitHub,
 * which allows 60 unauthenticated requests an hour per IP address (shared by every visitor on
 * the host's servers). The list answers 200, an empty one too, and the Data Cache keeps it for
 * `RELEASES_REVALIDATE_SECONDS`, so a new release shows up within that without a redeploy.
 */
import "server-only";

import { unstable_rethrow } from "next/navigation";

import { env } from "~/env";
import {
  latestFromGitHub,
  releasesRepoFrom,
  type LatestRelease,
} from "~/lib/releases";

export const RELEASES_REVALIDATE_SECONDS = 300;

// Five releases hold the latest published one unless drafts and prereleases pile up above it.
// Each carries its notes, and Next skips caching an answer over about 2 MB.
const PER_PAGE = 5;
// A cache miss waits on GitHub; past this the page offers GitHub's releases page instead.
const TIMEOUT_MS = 5_000;

export type FetchLike = (url: string, init: RequestInit) => Promise<Response>;

/** The repository whose releases the site offers (`RELEASES_REPO`, else the default). */
export function releasesRepo(): string {
  // A build made with SKIP_ENV_VALIDATION passes the raw value through, unchecked.
  return releasesRepoFrom(env.RELEASES_REPO);
}

/** The latest release, as `getLatestRelease` reads it, with every dependency passed in. */
export async function fetchLatestRelease({
  fetch: fetchReleases,
  repo,
  token,
  timeoutMs = TIMEOUT_MS,
  logError = (message) => console.warn(`[releases] ${message}`),
}: {
  fetch: FetchLike;
  repo: string;
  /** Sent only when set: public releases need none, but it lifts GitHub's rate limit. */
  token?: string | null;
  timeoutMs?: number;
  logError?: (message: string) => void;
}): Promise<LatestRelease> {
  const headers: Record<string, string> = {
    Accept: "application/vnd.github+json",
    "X-GitHub-Api-Version": "2022-11-28",
    // GitHub refuses API requests without one.
    "User-Agent": "agentnotch-website",
  };
  // Trimmed: a blank or padded value (easily pasted into a host's settings) would otherwise send
  // a malformed Authorization header, and a refused token sends every visitor to GitHub instead.
  const bearer = token?.trim();
  if (bearer) headers.Authorization = `Bearer ${bearer}`;

  try {
    const response = await fetchReleases(
      `https://api.github.com/repos/${repo}/releases?per_page=${PER_PAGE}`,
      {
        headers,
        // Explicit, so it is cached even on these dynamic pages, and with a token.
        next: { revalidate: RELEASES_REVALIDATE_SECONDS },
        signal: AbortSignal.timeout(timeoutMs),
      },
    );
    if (response.status !== 200) {
      // 403 or 429 is usually the rate limit; 404, a wrong RELEASES_REPO.
      logError(`GitHub answered ${response.status} for ${repo}`);
      return { kind: "unavailable" };
    }
    const latest = latestFromGitHub(await response.json(), repo);
    if (latest.kind === "unavailable")
      logError(`GitHub's list of releases for ${repo} wasn't readable`);
    return latest;
  } catch (error) {
    // Next's own control flow (dynamic rendering, a prerender bailing out) must pass through.
    unstable_rethrow(error);
    logError(
      `Couldn't reach GitHub for ${repo}: ${error instanceof Error ? error.name : "error"}`,
    );
    return { kind: "unavailable" };
  }
}

/** The latest published release of `releasesRepo()`. Never throws. */
export function getLatestRelease(): Promise<LatestRelease> {
  return fetchLatestRelease({
    fetch: globalThis.fetch,
    repo: releasesRepo(),
    token: env.GITHUB_RELEASES_TOKEN,
  });
}
