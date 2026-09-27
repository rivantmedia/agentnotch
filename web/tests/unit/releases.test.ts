/**
 * /download reads the latest release from GitHub and offers one file per platform. These hold
 * the rules that decide what is offered: which files count as installers (by name, as GitHub
 * renames them), which release is the latest, which addresses may be linked, and that the fetch
 * turns every failure into "unavailable" instead of an error page. Nothing here reaches GitHub:
 * the fetch is a fake.
 */
import { readFileSync } from "node:fs";
import path from "node:path";

import { afterEach, describe, expect, it, vi } from "vitest";

import {
  classifyAsset,
  DEFAULT_RELEASES_REPO,
  formatSize,
  isReleaseDownloadUrl,
  latestFromGitHub,
  leadingDownload,
  pickDownloads,
  pickRelease,
  platformFromUserAgent,
  RELEASES_REPO_PATTERN,
  releasesRepoFrom,
  versionFromTag,
  type GitHubAsset,
  type GitHubRelease,
} from "~/lib/releases";
import { fetchLatestRelease, type FetchLike } from "~/server/releases";

const REPO = "rivantmedia/agentnotch";
const DOWNLOAD = `https://github.com/${REPO}/releases/download`;

function asset(
  name: string,
  overrides: Partial<GitHubAsset> = {},
): GitHubAsset {
  return {
    name,
    browser_download_url: `${DOWNLOAD}/agentnotch-v1.0.0/${name}`,
    size: 14_234_567,
    state: "uploaded",
    ...overrides,
  };
}

function release(
  version: string,
  overrides: Partial<GitHubRelease> = {},
): GitHubRelease {
  const tag = `agentnotch-v${version}`;
  return {
    tag_name: tag,
    html_url: `https://github.com/${REPO}/releases/tag/${tag}`,
    published_at: "2026-09-27T12:00:00Z",
    draft: false,
    prerelease: false,
    assets: [
      asset(`AgentNotch-${version}.dmg`),
      asset(`AgentNotch-${version}.zip`),
      asset("appcast.xml", { size: 1200 }),
    ],
    ...overrides,
  };
}

describe("classifyAsset", () => {
  it("names the platform of each installer the release workflow makes, or may make", () => {
    const cases: Array<[string, string]> = [
      ["AgentNotch-1.0.0.dmg", "mac"],
      ["AgentNotch-1.0.0.zip", "mac"],
      ["AgentNotch-1.0.0-macos-universal.zip", "mac"],
      ["AgentNotch-1.0.0.pkg", "mac"],
      ["AgentNotch-1.0.0-Setup.exe", "windows"],
      ["AgentNotch-1.0.0.msi", "windows"],
      ["AgentNotch-1.0.0.msix", "windows"],
      ["AgentNotch-1.0.0-windows-x64.zip", "windows"],
      ["AgentNotch-1.0.0-x86_64.AppImage", "linux"],
      ["AgentNotch-1.0.0-amd64.deb", "linux"],
      ["AgentNotch-1.0.0-x86_64.rpm", "linux"],
      ["AgentNotch-1.0.0-linux-x86_64.tar.gz", "linux"],
      ["AgentNotch-1.0.0-linux-arm64.zip", "linux"],
    ];
    for (const [name, platform] of cases) {
      expect(classifyAsset(name), name).toBe(platform);
    }
  });

  it("matches GitHub's renamed uploads: a space becomes a dot", () => {
    // Uploaded as "Agent Notch 1.0.0.dmg", served as "Agent.Notch.1.0.0.dmg".
    expect(classifyAsset("Agent.Notch.1.0.0.dmg")).toBe("mac");
    expect(classifyAsset("Agent.Notch.dmg")).toBe("mac");
    expect(classifyAsset("Agent.Notch.1.0.0.zip")).toBe("mac");
    expect(classifyAsset("Agent.Notch.Setup.1.0.0.exe")).toBe("windows");
    expect(classifyAsset("Agent.Notch-1.0.0.AppImage")).toBe("linux");
    // Extensions in any case.
    expect(classifyAsset("AGENTNOTCH-1.0.0.DMG")).toBe("mac");
    expect(classifyAsset("agentnotch-1.0.0.appimage")).toBe("linux");
  });

  it("ignores the feed, signatures, checksums, metadata and upstream's builds", () => {
    for (const name of [
      "appcast.xml",
      "AppCast.XML",
      "AgentNotch-1.0.0.dmg.sha256",
      "AgentNotch-1.0.0.zip.sig",
      "AgentNotch-1.0.0.dmg.asc",
      "AgentNotch-1.0.0-Setup.exe.blockmap",
      "AgentNotch-1.0.0.delta",
      "SHA256SUMS",
      "SHA256SUMS.txt",
      "checksums.txt",
      "AgentNotch-checksums.zip",
      "latest.yml",
      "latest-mac.yml",
      "release.json",
      "notes.md",
      "AgentNotch-1.0.0-dSYM.zip",
      "AgentNotch-1.0.0-source.zip",
      // Upstream's names, whatever the extension: they are not this app.
      "Codenotch.dmg",
      "Codenotch-Setup.exe",
      "codenotch-1.0.0.zip",
      "CodeNotch_1.0.0_x64-setup.msi",
      "Agent.Notch.Codenotch.dmg",
      // No installer, or no telling which platform.
      "AgentNotch-1.0.0.tar.gz",
      "AgentNotch",
      "",
      "   ",
    ]) {
      expect(classifyAsset(name), name).toBeNull();
    }
  });
});

describe("pickDownloads", () => {
  it("offers the disk image over the Sparkle zip, in whichever order they come", () => {
    const dmg = asset("AgentNotch-1.0.0.dmg", { size: 15_000_000 });
    const zip = asset("AgentNotch-1.0.0.zip", { size: 14_000_000 });
    const feed = asset("appcast.xml", { size: 1200 });
    for (const list of [
      [dmg, zip, feed],
      [zip, dmg, feed],
      [feed, zip, dmg],
    ]) {
      expect(pickDownloads(list, REPO)).toEqual({
        mac: {
          name: "AgentNotch-1.0.0.dmg",
          url: dmg.browser_download_url,
          size: 15_000_000,
        },
        windows: null,
        linux: null,
      });
    }
  });

  it("falls back to the zip when the disk image isn't usable", () => {
    const zip = asset("AgentNotch-1.0.0.zip");
    for (const dmg of [
      asset("AgentNotch-1.0.0.dmg", { state: "open" }), // still uploading
      asset("AgentNotch-1.0.0.dmg", { size: 0 }),
      asset("AgentNotch-1.0.0.dmg", {
        browser_download_url: "https://evil.example/AgentNotch-1.0.0.dmg",
      }),
      asset("AgentNotch-1.0.0.dmg", {
        browser_download_url: `https://github.com/someone/else/releases/download/v1/AgentNotch-1.0.0.dmg`,
      }),
    ]) {
      expect(pickDownloads([dmg, zip], REPO).mac?.name).toBe(
        "AgentNotch-1.0.0.zip",
      );
    }
  });

  it("picks one file per platform, the best kind first", () => {
    const downloads = pickDownloads(
      [
        asset("AgentNotch-1.0.0.msi"),
        asset("AgentNotch-1.0.0-Setup.exe"),
        asset("AgentNotch-1.0.0-x86_64.rpm"),
        asset("AgentNotch-1.0.0-amd64.deb"),
        asset("AgentNotch-1.0.0-x86_64.AppImage"),
        asset("Codenotch-Setup.exe"),
      ],
      REPO,
    );
    expect(downloads.mac).toBeNull();
    expect(downloads.windows?.name).toBe("AgentNotch-1.0.0-Setup.exe");
    expect(downloads.linux?.name).toBe("AgentNotch-1.0.0-x86_64.AppImage");
  });

  it("offers nothing for a release of upstream's files only", () => {
    expect(
      pickDownloads(
        [
          asset("Codenotch.dmg"),
          asset("Codenotch-Setup.exe"),
          asset("appcast.xml"),
        ],
        REPO,
      ),
    ).toEqual({ mac: null, windows: null, linux: null });
  });
});

describe("leadingDownload", () => {
  const mac = { name: "a.dmg", url: `${DOWNLOAD}/v/a.dmg`, size: 1 };
  const exe = { name: "a.exe", url: `${DOWNLOAD}/v/a.exe`, size: 1 };

  it("leads with the visitor's platform when it has a file, else the Mac's", () => {
    const both = { mac, windows: exe, linux: null };
    expect(leadingDownload(both, "windows")).toBe("windows");
    expect(leadingDownload(both, "mac")).toBe("mac");
    expect(leadingDownload(both, "linux")).toBe("mac");
    expect(leadingDownload(both, null)).toBe("mac");
    expect(
      leadingDownload({ mac: null, windows: exe, linux: null }, null),
    ).toBe("windows");
    expect(
      leadingDownload({ mac: null, windows: null, linux: null }, "mac"),
    ).toBeNull();
  });
});

describe("pickRelease and latestFromGitHub", () => {
  it("takes the newest release that is neither a draft nor a prerelease", () => {
    const list = [
      release("1.2.0", { draft: true, published_at: null }),
      release("1.1.0-beta.1", { prerelease: true }),
      release("1.1.0"),
      release("1.0.0"),
    ];
    expect(pickRelease(list)?.tag_name).toBe("agentnotch-v1.1.0");

    const latest = latestFromGitHub(list, REPO);
    expect(latest).toEqual({
      kind: "ok",
      release: {
        tag: "agentnotch-v1.1.0",
        version: "1.1.0",
        notesUrl: `https://github.com/${REPO}/releases/tag/agentnotch-v1.1.0`,
        publishedAt: new Date("2026-09-27T12:00:00Z"),
      },
      downloads: {
        mac: {
          name: "AgentNotch-1.1.0.dmg",
          url: `${DOWNLOAD}/agentnotch-v1.0.0/AgentNotch-1.1.0.dmg`,
          size: 14_234_567,
        },
        windows: null,
        linux: null,
      },
    });
  });

  it("says none for an empty list, or drafts and prereleases only", () => {
    expect(pickRelease([])).toBeNull();
    expect(latestFromGitHub([], REPO)).toEqual({ kind: "none" });
    expect(
      latestFromGitHub(
        [
          release("1.0.0", { draft: true, published_at: null }),
          release("1.0.0-rc.1", { prerelease: true }),
        ],
        REPO,
      ),
    ).toEqual({ kind: "none" });
  });

  it("says unavailable for an answer of another shape", () => {
    for (const body of [
      null,
      "[]",
      { message: "Not Found" },
      { message: "API rate limit exceeded" },
      [{ tag_name: "v1.0.0" }],
      [{ ...release("1.0.0"), draft: "no" }],
      [{ ...release("1.0.0"), assets: [{ name: "a.dmg" }] }],
    ]) {
      expect(latestFromGitHub(body, REPO), JSON.stringify(body)).toEqual({
        kind: "unavailable",
      });
    }
  });

  it("ignores fields it doesn't read, and tolerates missing or odd dates", () => {
    const extra = {
      ...release("1.0.0", { published_at: "not a date" }),
      body: "## Notes",
      author: { login: "someone" },
    };
    const latest = latestFromGitHub([extra], REPO);
    expect(latest.kind).toBe("ok");
    if (latest.kind === "ok") expect(latest.release.publishedAt).toBeNull();
  });

  it("links release notes on the repository only", () => {
    const latest = latestFromGitHub(
      [release("1.0.0", { html_url: "https://evil.example/notes" })],
      REPO,
    );
    expect(latest.kind === "ok" && latest.release.notesUrl).toBe(
      `https://github.com/${REPO}/releases/latest`,
    );
  });

  it("reads the version off the tag", () => {
    expect(versionFromTag("agentnotch-v1.0.0")).toBe("1.0.0");
    expect(versionFromTag("AgentNotch-v2.10.3")).toBe("2.10.3");
    expect(versionFromTag("v1.18.0")).toBe("1.18.0");
    expect(versionFromTag("nightly")).toBe("nightly");
    expect(versionFromTag("agentnotch-vnext")).toBe("agentnotch-vnext");
  });
});

describe("isReleaseDownloadUrl", () => {
  it("accepts files on this repository's releases", () => {
    for (const url of [
      `${DOWNLOAD}/agentnotch-v1.0.0/AgentNotch-1.0.0.dmg`,
      `https://github.com/RivantMedia/AgentNotch/releases/download/agentnotch-v1.0.0/Agent.Notch.dmg`,
      `https://github.com:443/${REPO}/releases/download/agentnotch-v1.0.0/AgentNotch-1.0.0.zip`,
    ]) {
      expect(isReleaseDownloadUrl(url, REPO), url).toBe(true);
    }
  });

  it("refuses anything else", () => {
    for (const url of [
      `http://github.com/${REPO}/releases/download/v1/a.dmg`,
      `https://github.com.evil.example/${REPO}/releases/download/v1/a.dmg`,
      `https://objects.githubusercontent.com/${REPO}/releases/download/v1/a.dmg`,
      `https://evil.example/https://github.com/${REPO}/releases/download/v1/a.dmg`,
      `https://github.com/${REPO}-evil/releases/download/v1/a.dmg`,
      `https://github.com/someone/agentnotch/releases/download/v1/a.dmg`,
      `https://github.com/${REPO}/releases/tag/v1`,
      `https://github.com/${REPO}/releases/downloads/v1/a.dmg`,
      `https://github.com/${REPO}/archive/refs/heads/main.zip`,
      `https://user:pass@github.com/${REPO}/releases/download/v1/a.dmg`,
      `https://github.com:8443/${REPO}/releases/download/v1/a.dmg`,
      `https://github.com/${REPO}/releases/download/../../../evil/repo/a.dmg`,
      `https://github.com/${REPO}/releases/download/%2e%2e/%2e%2e/%2e%2e/evil/a.dmg`,
      `//github.com/${REPO}/releases/download/v1/a.dmg`,
      `/${REPO}/releases/download/v1/a.dmg`,
      "javascript:alert(1)",
      "not a url",
      "",
    ]) {
      expect(isReleaseDownloadUrl(url, REPO), url).toBe(false);
    }
  });
});

describe("releasesRepoFrom", () => {
  it("takes a plain owner/name, else the default", () => {
    expect(releasesRepoFrom("someone/some.repo_name-2")).toBe(
      "someone/some.repo_name-2",
    );
    expect(releasesRepoFrom("  someone/fork  ")).toBe("someone/fork");
    for (const value of [
      undefined,
      null,
      "",
      "  ",
      "agentnotch",
      "someone/fork/extra",
      "someone/..",
      "someone/.",
      "../fork",
      "-someone/fork",
      "some.one/fork",
      "https://github.com/someone/fork",
      "someone/fork?x=1",
      "someone/fork#x",
      "someone /fork",
    ]) {
      expect(releasesRepoFrom(value), String(value)).toBe(
        DEFAULT_RELEASES_REPO,
      );
    }
  });

  it("is the rule src/env.js checks RELEASES_REPO with", () => {
    const env = readFileSync(
      path.resolve(import.meta.dirname, "../../src/env.js"),
      "utf8",
    );
    expect(env).toContain(`.regex(/${RELEASES_REPO_PATTERN.source}/)`);
  });
});

describe("platformFromUserAgent", () => {
  it("tells desktop systems apart", () => {
    const cases: Array<[string, string]> = [
      [
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15",
        "mac",
      ],
      [
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36",
        "mac",
      ],
      [
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:143.0) Gecko/20100101 Firefox/143.0",
        "mac",
      ],
      [
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36 Edg/140.0.0.0",
        "windows",
      ],
      [
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:143.0) Gecko/20100101 Firefox/143.0",
        "windows",
      ],
      [
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36",
        "linux",
      ],
      [
        "Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:143.0) Gecko/20100101 Firefox/143.0",
        "linux",
      ],
    ];
    for (const [ua, platform] of cases) {
      expect(platformFromUserAgent(ua), ua).toBe(platform);
    }
  });

  it("offers nothing to phones, tablets, ChromeOS or the unknown", () => {
    for (const ua of [
      // iOS says "like Mac OS X"; Android and ChromeOS say "Linux".
      "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1",
      "Mozilla/5.0 (iPad; CPU OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1",
      "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Mobile Safari/537.36",
      "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36",
      "Mozilla/5.0 (Windows Phone 10.0; Android 6.0.1; Microsoft; Lumia 950) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/52.0 Mobile Safari/537.36 Edge/15.14977",
      "Mozilla/5.0 (X11; FreeBSD amd64; rv:140.0) Gecko/20100101 Firefox/140.0",
      "curl/8.7.1",
      "",
      null,
      undefined,
    ]) {
      expect(platformFromUserAgent(ua), String(ua)).toBeNull();
    }
  });
});

describe("formatSize", () => {
  it("reads like Finder: decimal units, one decimal at most", () => {
    expect(formatSize(0)).toBe("0 bytes");
    expect(formatSize(999)).toBe("999 bytes");
    expect(formatSize(1000)).toBe("1 KB");
    expect(formatSize(1_234)).toBe("1.2 KB");
    expect(formatSize(14_234_567)).toBe("14.2 MB");
    expect(formatSize(999_999)).toBe("1 MB");
    expect(formatSize(999_949_999)).toBe("999.9 MB");
    expect(formatSize(1_500_000_000)).toBe("1.5 GB");
    expect(formatSize(-1)).toBe("–");
    expect(formatSize(Number.NaN)).toBe("–");
  });
});

describe("fetchLatestRelease", () => {
  type Call = { url: string; init: RequestInit };

  function fakeFetch(answer: (call: Call) => Promise<Response> | Response) {
    const calls: Call[] = [];
    const fetch: FetchLike = async (url, init) => {
      const call = { url, init };
      calls.push(call);
      return answer(call);
    };
    return { fetch, calls };
  }

  const json = (body: unknown, status = 200) =>
    new Response(JSON.stringify(body), {
      status,
      headers: { "content-type": "application/json" },
    });

  it("lists a few releases, cached, and names itself", async () => {
    const { fetch, calls } = fakeFetch(() => json([release("1.0.0")]));
    const latest = await fetchLatestRelease({ fetch, repo: REPO });
    expect(latest.kind).toBe("ok");
    expect(calls).toHaveLength(1);
    const [call] = calls;
    expect(call!.url).toBe(
      `https://api.github.com/repos/${REPO}/releases?per_page=5`,
    );
    expect(call!.init.next).toEqual({
      revalidate: 300,
      tags: ["github-releases"],
    });
    expect(call!.init.signal).toBeInstanceOf(AbortSignal);
    expect(call!.init.headers).toEqual({
      Accept: "application/vnd.github+json",
      "X-GitHub-Api-Version": "2022-11-28",
      "User-Agent": "agentnotch-website",
    });
  });

  it("sends the token only when one is set", async () => {
    for (const token of [undefined, null, "", "  ", "\n"]) {
      const { fetch, calls } = fakeFetch(() => json([]));
      await fetchLatestRelease({ fetch, repo: REPO, token });
      const headers = calls[0]!.init.headers as Record<string, string>;
      expect(Object.keys(headers), JSON.stringify(token)).not.toContain(
        "Authorization",
      );
    }
    // Pasted with stray whitespace: the header carries the token alone.
    for (const token of ["github_pat_x", " github_pat_x\n"]) {
      const { fetch, calls } = fakeFetch(() => json([]));
      await fetchLatestRelease({ fetch, repo: REPO, token });
      expect(calls[0]!.init.headers, JSON.stringify(token)).toMatchObject({
        Authorization: "Bearer github_pat_x",
      });
    }
  });

  it("says none when the repository has no published release", async () => {
    const { fetch } = fakeFetch(() => json([]));
    expect(await fetchLatestRelease({ fetch, repo: REPO })).toEqual({
      kind: "none",
    });
  });

  it("says unavailable, and logs why, for anything but a readable 200", async () => {
    const answers: Array<[string, () => Promise<Response> | Response]> = [
      ["404", () => json({ message: "Not Found" }, 404)],
      ["403", () => json({ message: "API rate limit exceeded" }, 403)],
      ["429", () => json({ message: "Too many" }, 429)],
      ["500", () => new Response("oops", { status: 500 })],
      ["201", () => json([release("1.0.0")], 201)],
      ["not json", () => new Response("<html>", { status: 200 })],
      ["wrong shape", () => json({ releases: [] })],
      ["network", () => Promise.reject(new TypeError("fetch failed"))],
    ];
    for (const [what, answer] of answers) {
      const { fetch } = fakeFetch(answer);
      const logError = vi.fn();
      expect(
        await fetchLatestRelease({ fetch, repo: REPO, logError }),
        what,
      ).toEqual({ kind: "unavailable" });
      expect(logError, what).toHaveBeenCalledOnce();
    }
  });

  it("gives up after the timeout", async () => {
    // Answers only when the request is aborted, the way fetch does.
    const { fetch } = fakeFetch(
      ({ init }) =>
        new Promise<Response>((_, reject) => {
          init.signal?.addEventListener("abort", () =>
            reject(init.signal?.reason as Error),
          );
        }),
    );
    const logError = vi.fn();
    const started = Date.now();
    expect(
      await fetchLatestRelease({ fetch, repo: REPO, timeoutMs: 20, logError }),
    ).toEqual({ kind: "unavailable" });
    expect(Date.now() - started).toBeLessThan(2_000);
    expect(logError).toHaveBeenCalledWith(
      expect.stringContaining("TimeoutError"),
    );
  });

  it("never puts the token in a log line", async () => {
    const { fetch } = fakeFetch(() =>
      json({ message: "Bad credentials" }, 401),
    );
    const logError = vi.fn();
    await fetchLatestRelease({
      fetch,
      repo: REPO,
      token: "github_pat_secret",
      logError,
    });
    expect(JSON.stringify(logError.mock.calls)).not.toContain("secret");
  });
});

describe("getLatestRelease", () => {
  afterEach(() => {
    vi.unstubAllEnvs();
    vi.unstubAllGlobals();
    vi.resetModules();
  });

  // src/env.js reads the environment when it loads, so each case loads a fresh copy. The fetch
  // is a fake in place of the global one Next.js patches.
  async function latestWith(environment: Record<string, string | undefined>) {
    for (const [name, value] of Object.entries(environment)) {
      vi.stubEnv(name, value);
    }
    vi.resetModules();
    const calls: Array<{ url: string; init: RequestInit }> = [];
    vi.stubGlobal("fetch", async (url: string, init: RequestInit) => {
      calls.push({ url, init });
      return new Response("[]", { status: 200 });
    });
    const { getLatestRelease } = await import("~/server/releases");
    return { latest: await getLatestRelease(), calls };
  }

  it("asks for RELEASES_REPO's releases, with GITHUB_RELEASES_TOKEN", async () => {
    const { latest, calls } = await latestWith({
      RELEASES_REPO: "someone/fork",
      GITHUB_RELEASES_TOKEN: "github_pat_x",
    });
    expect(latest).toEqual({ kind: "none" });
    expect(calls.map((call) => call.url)).toEqual([
      "https://api.github.com/repos/someone/fork/releases?per_page=5",
    ]);
    expect(calls[0]!.init.headers).toMatchObject({
      Authorization: "Bearer github_pat_x",
    });
  });

  it("falls back to the default repository, without a token, when they're unset or malformed", async () => {
    // Tests, like a build made with SKIP_ENV_VALIDATION, get the raw values, unchecked.
    for (const repo of [undefined, "", "https://github.com/someone/fork"]) {
      const { calls } = await latestWith({
        RELEASES_REPO: repo,
        GITHUB_RELEASES_TOKEN: undefined,
      });
      expect(calls[0]!.url, String(repo)).toBe(
        `https://api.github.com/repos/${DEFAULT_RELEASES_REPO}/releases?per_page=5`,
      );
      expect(Object.keys(calls[0]!.init.headers ?? {})).not.toContain(
        "Authorization",
      );
    }
  });
});
