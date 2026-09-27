/**
 * What /download offers: the latest published release of the app's GitHub repository, and which
 * of its files is the download for each platform. Pure (parsing GitHub's answer, naming files,
 * reading a User-Agent), so the page, its redirects and the tests agree; the fetch itself is
 * src/server/releases.ts.
 */
import { z } from "zod";

/** Where releases come from unless `RELEASES_REPO` says otherwise. */
export const DEFAULT_RELEASES_REPO = "rivantmedia/agentnotch";

/**
 * `owner/name` as GitHub allows them. src/env.js holds the same rule (a test says so); this copy
 * covers a build made with SKIP_ENV_VALIDATION, which passes the raw value through.
 */
export const RELEASES_REPO_PATTERN =
  /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})\/(?!\.\.?$)[\w.-]{1,100}$/;

export const PLATFORMS = ["mac", "windows", "linux"] as const;
export type Platform = (typeof PLATFORMS)[number];

export const PLATFORM_NAMES: Record<Platform, string> = {
  mac: "Mac",
  windows: "Windows",
  linux: "Linux",
};

export function isPlatform(value: string): value is Platform {
  return (PLATFORMS as readonly string[]).includes(value);
}

/** The configured repository when it is a plain `owner/name`, else the default. */
export function releasesRepoFrom(
  configured: string | null | undefined,
): string {
  const repo = configured?.trim();
  return repo && RELEASES_REPO_PATTERN.test(repo)
    ? repo
    : DEFAULT_RELEASES_REPO;
}

export function repoUrl(repo: string): string {
  return `https://github.com/${repo}`;
}

/** GitHub's page for the newest published release: the way out when GitHub's API can't help. */
export function latestReleasePageUrl(repo: string): string {
  return `${repoUrl(repo)}/releases/latest`;
}

/**
 * True for `https://github.com/<repo>/<section>…` and nothing else: the only addresses the site
 * links or redirects to from GitHub's answer, so a changed or tampered answer can't send
 * visitors elsewhere. Owner and name compare without case, as GitHub's do.
 */
export function isRepoUrl(url: string, repo: string, section: string): boolean {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return false;
  }
  return (
    parsed.protocol === "https:" &&
    parsed.host === "github.com" &&
    parsed.username === "" &&
    parsed.password === "" &&
    parsed.pathname
      .toLowerCase()
      .startsWith(`/${repo}/${section}`.toLowerCase())
  );
}

/** A release's file, as GitHub serves it to browsers. */
export function isReleaseDownloadUrl(url: string, repo: string): boolean {
  return isRepoUrl(url, repo, "releases/download/");
}

// What the site reads of GitHub's "list releases" answer; everything else is dropped.
const assetSchema = z.object({
  name: z.string(),
  browser_download_url: z.string(),
  size: z.number(),
  // "uploaded", or "open" while an upload is unfinished (its address answers 404 until then).
  state: z.string().optional(),
});

const releaseSchema = z.object({
  tag_name: z.string(),
  html_url: z.string(),
  // Null on drafts.
  published_at: z.string().nullish(),
  draft: z.boolean(),
  prerelease: z.boolean(),
  assets: z.array(assetSchema),
});

export const releaseListSchema = z.array(releaseSchema);

export type GitHubRelease = z.infer<typeof releaseSchema>;
export type GitHubAsset = z.infer<typeof assetSchema>;

export type ReleaseAsset = { name: string; url: string; size: number };
export type Downloads = Record<Platform, ReleaseAsset | null>;

export type Release = {
  tag: string;
  /** The tag without its `agentnotch-v` (or `v`) prefix: "1.0.0". */
  version: string;
  /** The release's page on GitHub, with its notes. */
  notesUrl: string;
  publishedAt: Date | null;
};

/**
 * `ok` with the release and its downloads; `none` when the repository has no published release
 * (yet); `unavailable` when GitHub didn't answer, or answered something else.
 */
export type LatestRelease =
  | { kind: "ok"; release: Release; downloads: Downloads }
  | { kind: "none" }
  | { kind: "unavailable" };

/**
 * The newest release that is neither a draft nor a prerelease, as GitHub's own "latest" picks it
 * (the list comes newest first). A token with access to the repository sees drafts too, and the
 * release workflow publishes a draft only once its files are all there.
 */
export function pickRelease(
  releases: readonly GitHubRelease[],
): GitHubRelease | null {
  return releases.find((r) => !r.draft && !r.prerelease) ?? null;
}

/** Reads GitHub's "list releases" answer for `repo`. */
export function latestFromGitHub(body: unknown, repo: string): LatestRelease {
  const parsed = releaseListSchema.safeParse(body);
  if (!parsed.success) return { kind: "unavailable" };
  const release = pickRelease(parsed.data);
  if (!release) return { kind: "none" };
  const published = release.published_at
    ? new Date(release.published_at)
    : null;
  return {
    kind: "ok",
    release: {
      tag: release.tag_name,
      version: versionFromTag(release.tag_name),
      notesUrl: isRepoUrl(release.html_url, repo, "releases/")
        ? release.html_url
        : latestReleasePageUrl(repo),
      publishedAt:
        published && Number.isFinite(published.getTime()) ? published : null,
    },
    downloads: pickDownloads(release.assets, repo),
  };
}

/** "agentnotch-v1.0.0" and "v1.0.0" read "1.0.0"; any other tag is shown as it is. */
export function versionFromTag(tag: string): string {
  return tag.replace(/^agentnotch-v(?=\d)/i, "").replace(/^v(?=\d)/i, "");
}

/**
 * The file to offer for each platform: the best kind of installer (lowest rank) among the
 * finished uploads on this repository's release, first one listed on a tie.
 */
export function pickDownloads(
  assets: readonly GitHubAsset[],
  repo: string,
): Downloads {
  const best: Record<Platform, { asset: ReleaseAsset; rank: number } | null> = {
    mac: null,
    windows: null,
    linux: null,
  };
  for (const asset of assets) {
    if (asset.state !== undefined && asset.state !== "uploaded") continue;
    if (!(asset.size > 0)) continue;
    if (!isReleaseDownloadUrl(asset.browser_download_url, repo)) continue;
    const kind = assetKind(asset.name);
    if (!kind) continue;
    const current = best[kind.platform];
    if (!current || kind.rank < current.rank) {
      best[kind.platform] = {
        asset: {
          name: asset.name,
          url: asset.browser_download_url,
          size: asset.size,
        },
        rank: kind.rank,
      };
    }
  }
  return {
    mac: best.mac?.asset ?? null,
    windows: best.windows?.asset ?? null,
    linux: best.linux?.asset ?? null,
  };
}

/** The one download to lead with: the visitor's own when there is one, else the Mac's, else any. */
export function leadingDownload(
  downloads: Downloads,
  visitor: Platform | null,
): Platform | null {
  if (visitor && downloads[visitor]) return visitor;
  return PLATFORMS.find((platform) => downloads[platform] !== null) ?? null;
}

/** The platform a release file installs on, or null when it is no installer. */
export function classifyAsset(name: string): Platform | null {
  return assetKind(name)?.platform ?? null;
}

// Files a release carries beside the installers: Sparkle's appcast.xml, signatures, checksums,
// update metadata (electron-builder's latest.yml and .blockmap), notes.
const NOT_INSTALLER =
  /\.(?:xml|json|ya?ml|txt|md|sig|asc|minisig|pem|p7s|sha1|sha256|sha512|md5|blockmap|delta)$/;
const NOT_INSTALLER_WORDS = new Set([
  "checksum",
  "checksums",
  "sha256sums",
  "sums",
  "dsym",
  "dsyms",
  "symbols",
  "debug",
  "src",
  "source",
]);
const WINDOWS_WORDS = new Set(["win", "windows", "win32", "win64"]);
const LINUX_WORDS = new Set(["linux"]);
const MAC_WORDS = new Set(["mac", "macos", "darwin", "osx"]);

/**
 * Names are matched by extension and by the words in them, never exactly: GitHub renames
 * uploads (a space becomes a dot, so "Agent Notch.dmg" is served as "Agent.Notch.dmg").
 */
function assetKind(name: string): { platform: Platform; rank: number } | null {
  const lower = name.trim().toLowerCase();
  // Upstream's own builds (Codenotch-Setup.exe and the like) are not this app.
  if (lower === "" || lower.includes("codenotch")) return null;
  if (NOT_INSTALLER.test(lower)) return null;
  const words = lower.split(/[^a-z0-9]+/).filter(Boolean);
  if (words.some((word) => NOT_INSTALLER_WORDS.has(word))) return null;
  const says = (set: Set<string>) => words.some((word) => set.has(word));

  if (lower.endsWith(".dmg")) return { platform: "mac", rank: 0 };
  if (lower.endsWith(".pkg")) return { platform: "mac", rank: 1 };
  if (lower.endsWith(".exe")) return { platform: "windows", rank: 0 };
  if (lower.endsWith(".msi")) return { platform: "windows", rank: 1 };
  if (lower.endsWith(".msix") || lower.endsWith(".appx"))
    return { platform: "windows", rank: 2 };
  if (lower.endsWith(".appimage")) return { platform: "linux", rank: 0 };
  if (lower.endsWith(".deb")) return { platform: "linux", rank: 1 };
  if (lower.endsWith(".rpm")) return { platform: "linux", rank: 2 };
  if (/\.(?:tar\.gz|tgz|tar\.xz|tar\.zst)$/.test(lower)) {
    if (says(LINUX_WORDS)) return { platform: "linux", rank: 3 };
    if (says(MAC_WORDS)) return { platform: "mac", rank: 3 };
    return null;
  }
  if (lower.endsWith(".zip")) {
    if (says(WINDOWS_WORDS)) return { platform: "windows", rank: 3 };
    if (says(LINUX_WORDS)) return { platform: "linux", rank: 4 };
    // The Mac app's Sparkle archive (AgentNotch-<version>.zip) names no platform. The disk
    // image beside it ranks first; the archive is a fallback.
    return { platform: "mac", rank: 2 };
  }
  return null;
}

/**
 * The visitor's platform from their User-Agent, to put its download first; null for phones,
 * tablets, ChromeOS and anything unclear. iOS says "like Mac OS X", and Android and ChromeOS say
 * "Linux", so those are ruled out before the desktop names are read. (iPadOS Safari presents
 * itself as a Mac, and can't be told apart.)
 */
export function platformFromUserAgent(
  userAgent: string | null | undefined,
): Platform | null {
  const ua = userAgent ?? "";
  if (/iPhone|iPad|iPod|Android|CrOS|Windows Phone/i.test(ua)) return null;
  if (/Windows/i.test(ua)) return "windows";
  if (/Macintosh|Mac OS X/i.test(ua)) return "mac";
  if (/Linux/i.test(ua)) return "linux";
  return null;
}

const SIZE = new Intl.NumberFormat("en-US", { maximumFractionDigits: 1 });
const UNITS = ["KB", "MB", "GB"] as const;

/** "14.2 MB", in the decimal units Finder uses. */
export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "–";
  if (bytes < 1000) return `${Math.round(bytes)} bytes`;
  let value = bytes / 1000;
  let unit = 0;
  // Compared as shown (one decimal), so 999,999 bytes reads "1 MB" rather than "1,000 KB".
  while (unit < UNITS.length - 1 && Math.round(value * 10) / 10 >= 1000) {
    value /= 1000;
    unit += 1;
  }
  return `${SIZE.format(value)} ${UNITS[unit]}`;
}
