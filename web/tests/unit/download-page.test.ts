/**
 * /download shows what the latest release offers, marks the visitor's platform, and says so
 * plainly when there is no release yet or GitHub can't be reached. Its download buttons (and the
 * landing page's) are plain links to /download/<platform>, and the landing page never waits on
 * GitHub. Rendered on the server's terms (renderToString); GitHub and the request are mocks.
 */
import { readFileSync } from "node:fs";
import path from "node:path";

import { renderToString } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { type LatestRelease, type ReleaseAsset } from "~/lib/releases";

const getLatestRelease = vi.fn<() => Promise<LatestRelease>>();
vi.mock("~/server/releases", () => ({
  getLatestRelease,
  releasesRepo: () => "rivantmedia/agentnotch",
}));

let userAgent = "";
vi.mock("next/headers", () => ({
  headers: async () => new Headers({ "user-agent": userAgent }),
}));

const { default: DownloadPage } = await import("~/app/download/page");

const MAC_UA =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15";
const WINDOWS_UA =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";
const IPHONE_UA =
  "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";

const DMG: ReleaseAsset = {
  name: "AgentNotch-1.0.0.dmg",
  url: "https://github.com/rivantmedia/agentnotch/releases/download/agentnotch-v1.0.0/AgentNotch-1.0.0.dmg",
  size: 14_234_567,
};

const OK: LatestRelease = {
  kind: "ok",
  release: {
    tag: "agentnotch-v1.0.0",
    version: "1.0.0",
    notesUrl:
      "https://github.com/rivantmedia/agentnotch/releases/tag/agentnotch-v1.0.0",
    publishedAt: new Date("2026-09-27T12:00:00Z"),
  },
  downloads: { mac: DMG, windows: null, linux: null },
};

async function render(query: Record<string, string | string[]> = {}) {
  const page = await DownloadPage({ searchParams: Promise.resolve(query) });
  // React separates adjacent text with comments; the tests read the text.
  return renderToString(page).replace(/<!-- -->/g, "");
}

/** Each platform's card, by its heading. */
function cards(html: string): Record<string, string> {
  const found: Record<string, string> = {};
  for (const [card] of html.matchAll(/<li class="card[^"]*">[\s\S]*?<\/li>/g)) {
    const name = /<h3[^>]*>([^<]+)<\/h3>/.exec(card)?.[1];
    if (name) found[name] = card;
  }
  return found;
}

beforeEach(() => {
  getLatestRelease.mockReset();
  userAgent = MAC_UA;
});

describe("the download page", () => {
  it("offers the Mac file through /download/mac, with its version, size and requirements", async () => {
    getLatestRelease.mockResolvedValue(OK);
    const html = await render();
    const { Mac, Windows, Linux } = cards(html);

    expect(html).toContain("Version 1.0.0");
    expect(html).toContain(`href="${OK.kind === "ok" && OK.release.notesUrl}"`);
    expect(Mac).toContain('href="/download/mac"');
    expect(Mac).toContain("btn btn-primary");
    expect(Mac).toContain("Download for Mac");
    expect(Mac).toContain("AgentNotch-1.0.0.dmg · 14.2 MB");
    expect(Mac).toContain("macOS 15 or later · Apple silicon and Intel");
    // Never the file's own address: every button goes through the redirect.
    expect(html).not.toContain("/releases/download/");

    for (const card of [Windows, Linux]) {
      expect(card).toContain("Not available yet");
      expect(card).not.toContain("/download/");
    }
  });

  it("marks the visitor's platform", async () => {
    getLatestRelease.mockResolvedValue(OK);
    let found = cards(await render());
    expect(found.Mac).toContain("Your computer");
    expect(found.Mac).toContain("border-accent");
    expect(found.Windows).not.toContain("Your computer");

    userAgent = WINDOWS_UA;
    found = cards(await render());
    expect(found.Windows).toContain("Your computer");
    expect(found.Windows).toContain("Not available yet");
    expect(found.Mac).not.toContain("Your computer");
    // The Mac's is still the one download, so it still leads.
    expect(found.Mac).toContain("btn btn-primary");

    userAgent = IPHONE_UA;
    expect(await render()).not.toContain("Your computer");
  });

  it("explains a bounce from a platform with no file", async () => {
    getLatestRelease.mockResolvedValue(OK);
    expect(await render({ unavailable: "windows" })).toContain(
      "The latest release has no file for Windows.",
    );
    // Not for a platform that has one (the release changed since), nor for nonsense.
    for (const unavailable of ["mac", "ios", "", "__proto__"]) {
      expect(await render({ unavailable }), unavailable).not.toContain(
        "The latest release has no file",
      );
    }
  });

  it("says when there is no release yet", async () => {
    getLatestRelease.mockResolvedValue({ kind: "none" });
    const html = await render({ unavailable: "mac" });
    expect(html).toContain("No release yet");
    expect(html).toContain('href="https://github.com/rivantmedia/agentnotch"');
    expect(html).not.toContain('href="/download/');
    expect(html.match(/Not available yet/g)).toHaveLength(3);
    // The notice above covers the bounce.
    expect(html).not.toContain("The latest release has no file");
  });

  it("points to GitHub's releases page when GitHub can't be reached", async () => {
    getLatestRelease.mockResolvedValue({ kind: "unavailable" });
    const html = await render();
    expect(html).toContain("Couldn&#x27;t load the latest release");
    expect(html).toContain(
      'href="https://github.com/rivantmedia/agentnotch/releases/latest"',
    );
    // No platform is claimed available or not.
    expect(html).not.toContain("Not available yet");
    expect(html).not.toContain('href="/download/');
  });

  it("tells people how to open and update the app", async () => {
    getLatestRelease.mockResolvedValue(OK);
    const html = await render();
    expect(html).toContain("<details");
    expect(html).toContain("Applications");
    expect(html).toContain("disk image or from Downloads");
    expect(html).toContain("System Settings &gt; Privacy &amp; Security");
    expect(html).toContain("Open Anyway");
    expect(html).toContain(
      "xattr -dr com.apple.quarantine &quot;/Applications/Agent Notch.app&quot;",
    );
    expect(html).toContain("Updates install themselves");
    expect(html).toMatch(/Settings &gt; General<\/strong> and choose/);
    expect(html).toContain("Check now");
  });

  it("offers Windows as a preview, with the SmartScreen and Smart App Control caveats", async () => {
    const exe: ReleaseAsset = {
      name: "AgentNotch-1.1.0-Setup.exe",
      url: "https://github.com/rivantmedia/agentnotch/releases/download/agentnotch-v1.1.0/AgentNotch-1.1.0-Setup.exe",
      size: 9_000_000,
    };
    if (OK.kind !== "ok") throw new Error("OK is a release");
    getLatestRelease.mockResolvedValue({
      ...OK,
      downloads: { mac: DMG, windows: exe, linux: null },
    });
    userAgent = WINDOWS_UA;
    const html = await render();
    const { Windows } = cards(html);
    expect(Windows).toContain('href="/download/windows"');
    expect(Windows).toContain("Preview: Windows 10 or 11 (x64)");
    expect(Windows).toContain("btn btn-primary");
    expect(html).toMatch(/Installing on Windows[\s\S]*?>Preview</);
    expect(html).toContain("isn&#x27;t code-signed yet");
    expect(html).toContain("Windows protected your PC");
    expect(html).toMatch(/More info[\s\S]*Run anyway/);
    expect(html).toContain("Smart App Control");
    expect(html).toContain("can&#x27;t be used there until it is");
    expect(html).toContain("Delete the application data");
  });

  it("leaves the Windows steps out while the latest release has no installer", async () => {
    getLatestRelease.mockResolvedValue(OK);
    let html = await render();
    expect(html).not.toContain("Installing on Windows");
    // The card still says what Windows will need.
    expect(cards(html).Windows).toContain("Preview: Windows 10 or 11 (x64)");

    getLatestRelease.mockResolvedValue({ kind: "none" });
    expect(await render()).not.toContain("Installing on Windows");

    // GitHub can't say what the latest release holds: the steps stay.
    getLatestRelease.mockResolvedValue({ kind: "unavailable" });
    html = await render();
    expect(html).toContain("Installing on Windows");
  });
});

describe("download links", () => {
  const app = path.resolve(import.meta.dirname, "../../src/app");
  const read = (file: string) => readFileSync(path.join(app, file), "utf8");

  it("are plain links, so nothing prefetches the redirect", () => {
    // next/link prefetches its target; for /download/<platform> that is a GitHub call per view.
    for (const file of ["page.tsx", "download/page.tsx"]) {
      expect(read(file), file).not.toMatch(
        /<Link[^>]*href=\{?[`"]\/download\//,
      );
    }
    expect(read("page.tsx")).toMatch(/<a\s+href="\/download\/mac"/);
    expect(read("download/page.tsx")).toMatch(
      /<a\s+href=\{`\/download\/\$\{platform\}`\}/,
    );
  });

  it("keep the landing page off GitHub", () => {
    const landing = read("page.tsx");
    expect(landing).not.toMatch(/from\s+"[^"]*releases"/);
    expect(landing).not.toMatch(/\bfetch\(/);
  });
});
