/**
 * /download/<platform> is the stable link every download button uses. It follows only a file on
 * the configured repository's releases, sends a platform with no file back to /download (which
 * says so), sends everyone to GitHub's releases page when GitHub's API can't answer, and is never
 * cached. GitHub is a mock here.
 */
import { NextRequest } from "next/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { type LatestRelease, type ReleaseAsset } from "~/lib/releases";

const getLatestRelease = vi.fn<() => Promise<LatestRelease>>();
const releasesRepo = vi.fn(() => "rivantmedia/agentnotch");
vi.mock("~/server/releases", () => ({ getLatestRelease, releasesRepo }));

const { GET } = await import("~/app/download/[platform]/route");

const SITE = "https://notch.example.com";
const FILES = "https://github.com/rivantmedia/agentnotch/releases/download";
const RELEASES_PAGE =
  "https://github.com/rivantmedia/agentnotch/releases/latest";

function file(name: string, url = `${FILES}/agentnotch-v1.0.0/${name}`) {
  return { name, url, size: 14_234_567 } satisfies ReleaseAsset;
}

function ok(downloads: Partial<Record<string, ReleaseAsset>>): LatestRelease {
  return {
    kind: "ok",
    release: {
      tag: "agentnotch-v1.0.0",
      version: "1.0.0",
      notesUrl:
        "https://github.com/rivantmedia/agentnotch/releases/tag/agentnotch-v1.0.0",
      publishedAt: new Date("2026-09-27T12:00:00Z"),
    },
    downloads: {
      mac: downloads.mac ?? null,
      windows: downloads.windows ?? null,
      linux: downloads.linux ?? null,
    },
  };
}

function get(platform: string, headers: Record<string, string> = {}) {
  return GET(
    new NextRequest(`${SITE}/download/${encodeURIComponent(platform)}`, {
      headers: { host: "notch.example.com", ...headers },
    }),
    { params: Promise.resolve({ platform }) },
  );
}

function expectRedirect(response: Response, location: string) {
  expect(response.status).toBe(302);
  expect(response.headers.get("location")).toBe(location);
  expect(response.headers.get("cache-control")).toBe("private, no-store");
}

beforeEach(() => {
  getLatestRelease.mockReset();
  releasesRepo.mockClear();
  releasesRepo.mockReturnValue("rivantmedia/agentnotch");
});

describe("GET /download/<platform>", () => {
  it("sends the browser to the release's file", async () => {
    const dmg = file("AgentNotch-1.0.0.dmg");
    getLatestRelease.mockResolvedValue(ok({ mac: dmg }));
    expectRedirect(await get("mac"), dmg.url);

    const exe = file("AgentNotch-1.0.0-Setup.exe");
    const appImage = file("AgentNotch-1.0.0-x86_64.AppImage");
    getLatestRelease.mockResolvedValue(
      ok({ mac: dmg, windows: exe, linux: appImage }),
    );
    expectRedirect(await get("windows"), exe.url);
    expectRedirect(await get("linux"), appImage.url);
  });

  it("follows only files on the repository's releases", async () => {
    for (const url of [
      "https://evil.example/AgentNotch-1.0.0.dmg",
      "http://github.com/rivantmedia/agentnotch/releases/download/v1/a.dmg",
      "https://github.com/someone/else/releases/download/v1/a.dmg",
      "https://github.com/rivantmedia/agentnotch/releases/tag/v1",
      "https://github.com.evil.example/rivantmedia/agentnotch/releases/download/v1/a.dmg",
      "https://github.com/rivantmedia/agentnotch/releases/download/../../../evil/x/a.dmg",
    ]) {
      getLatestRelease.mockResolvedValue(
        ok({ mac: file("AgentNotch-1.0.0.dmg", url) }),
      );
      const response = await get("mac");
      expectRedirect(response, RELEASES_PAGE);
    }
  });

  it("checks against the configured repository", async () => {
    releasesRepo.mockReturnValue("someone/fork");
    const theirs = file(
      "AgentNotch-1.0.0.dmg",
      "https://github.com/someone/fork/releases/download/v1/AgentNotch-1.0.0.dmg",
    );
    getLatestRelease.mockResolvedValue(ok({ mac: theirs }));
    expectRedirect(await get("mac"), theirs.url);

    // The default repository's file is not the configured one's.
    getLatestRelease.mockResolvedValue(
      ok({ mac: file("AgentNotch-1.0.0.dmg") }),
    );
    expectRedirect(
      await get("mac"),
      "https://github.com/someone/fork/releases/latest",
    );
  });

  it("goes back to /download when the release has no file for the platform", async () => {
    getLatestRelease.mockResolvedValue(
      ok({ mac: file("AgentNotch-1.0.0.dmg") }),
    );
    expectRedirect(
      await get("windows"),
      `${SITE}/download?unavailable=windows`,
    );
    expectRedirect(await get("linux"), `${SITE}/download?unavailable=linux`);

    getLatestRelease.mockResolvedValue(ok({}));
    expectRedirect(await get("mac"), `${SITE}/download?unavailable=mac`);
  });

  it("goes back to /download when there is no release yet", async () => {
    getLatestRelease.mockResolvedValue({ kind: "none" });
    expectRedirect(await get("mac"), `${SITE}/download?unavailable=mac`);
  });

  it("builds that address from the address the browser used", async () => {
    getLatestRelease.mockResolvedValue({ kind: "none" });
    expectRedirect(
      await get("mac", {
        host: "localhost:3000",
        "x-forwarded-host": "notch.example.org",
        "x-forwarded-proto": "https",
      }),
      "https://notch.example.org/download?unavailable=mac",
    );
  });

  it("sends everyone to GitHub's releases page when GitHub's API can't answer", async () => {
    getLatestRelease.mockResolvedValue({ kind: "unavailable" });
    for (const platform of ["mac", "windows", "linux"]) {
      expectRedirect(await get(platform), RELEASES_PAGE);
    }
  });

  it("answers 404 for anything but a known platform, without asking GitHub", async () => {
    for (const platform of [
      "ios",
      "Mac",
      "macos",
      "",
      "..",
      "mac.dmg",
      "mac/../windows",
      "constructor",
      "__proto__",
    ]) {
      const response = await get(platform);
      expect(response.status, platform).toBe(404);
      expect(response.headers.get("location"), platform).toBeNull();
      expect(response.headers.get("cache-control"), platform).toBe(
        "private, no-store",
      );
    }
    expect(getLatestRelease).not.toHaveBeenCalled();
  });
});
