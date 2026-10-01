import { type Metadata } from "next";
import { headers } from "next/headers";
import { type ReactNode } from "react";

import {
  formatSize,
  isPlatform,
  latestReleasePageUrl,
  leadingDownload,
  PLATFORM_NAMES,
  platformFromUserAgent,
  PLATFORMS,
  repoUrl,
  type Downloads,
  type Platform,
  type Release,
} from "~/lib/releases";
import { getLatestRelease, releasesRepo } from "~/server/releases";

import { CopyButton } from "../_components/copy-button";
import { DateTime } from "../_components/time";
import { Badge, Callout, cx, PageHeader } from "../_components/ui";

export const metadata: Metadata = {
  title: "Download",
  description:
    "Download Agent Notch for the Mac and Windows: your Claude usage limits in the notch.",
};

// Per request (it reads the visitor's User-Agent), and never prerendered at build time, which
// would ask GitHub from the build. GitHub's answer is still cached, by the fetch.
export const dynamic = "force-dynamic";

// What the release workflow builds: universal, with LSMinimumSystemVersion 15.0.
const MAC_REQUIREMENTS = "macOS 15 or later · Apple silicon and Intel";
const QUARANTINE_COMMAND =
  'xattr -dr com.apple.quarantine "/Applications/Agent Notch.app"';

// What the Windows installer is built for (the installer needs no administrator).
const WINDOWS_REQUIREMENTS = "Preview: Windows 10 or 11 (x64)";

const NO_DOWNLOADS: Downloads = { mac: null, windows: null, linux: null };

/**
 * /download: the latest release's file for each platform, the visitor's own marked. Public.
 * GitHub's answer is cached for a few minutes (src/server/releases.ts); when GitHub can't be
 * reached, the page points to its releases page instead of guessing.
 */
export default async function DownloadPage({
  searchParams,
}: {
  searchParams: Promise<{ unavailable?: string | string[] }>;
}) {
  const [latest, requestHeaders, query] = await Promise.all([
    getLatestRelease(),
    headers(),
    searchParams,
  ]);
  const repo = releasesRepo();
  const visitor = platformFromUserAgent(requestHeaders.get("user-agent"));
  const downloads = latest.kind === "ok" ? latest.downloads : NO_DOWNLOADS;
  const leading = leadingDownload(downloads, visitor);
  // Set by /download/<platform> when the release has no file for it.
  const bounced = first(query.unavailable);
  const missing =
    latest.kind === "ok" &&
    bounced !== undefined &&
    isPlatform(bounced) &&
    downloads[bounced] === null
      ? bounced
      : null;

  return (
    <div className="mx-auto flex max-w-4xl flex-col gap-10 px-4 py-8 sm:py-12">
      <PageHeader
        title="Download Agent Notch"
        description={
          latest.kind === "ok" ? (
            <ReleaseSummary release={latest.release} />
          ) : undefined
        }
      />

      {missing ? (
        <Callout title={`No ${PLATFORM_NAMES[missing]} download`}>
          The latest release has no file for {PLATFORM_NAMES[missing]}.
        </Callout>
      ) : null}

      {latest.kind === "unavailable" ? (
        <Callout title="Couldn't load the latest release">
          <span className="flex flex-col items-start gap-3">
            <span>
              GitHub didn&apos;t answer just now. Every release is on its
              releases page.
            </span>
            <a
              href={latestReleasePageUrl(repo)}
              className="btn btn-primary btn-sm"
            >
              Open the releases page
            </a>
          </span>
        </Callout>
      ) : (
        <>
          {latest.kind === "none" ? (
            <Callout title="No release yet">
              The first release isn&apos;t out yet. Until it is, the app can be
              built from{" "}
              <a href={repoUrl(repo)} className="link">
                its source on GitHub
              </a>
              .
            </Callout>
          ) : null}
          <section aria-labelledby="platforms-title">
            <h2 id="platforms-title" className="sr-only">
              Platforms
            </h2>
            <ul className="grid gap-4 md:grid-cols-3">
              {PLATFORMS.map((platform) => (
                <PlatformCard
                  key={platform}
                  platform={platform}
                  downloads={downloads}
                  yours={visitor === platform}
                  leading={leading === platform}
                />
              ))}
            </ul>
          </section>
        </>
      )}

      <section aria-labelledby="install-title" className="flex flex-col gap-4">
        <h2 id="install-title" className="text-lg font-semibold tracking-tight">
          Installing on a Mac
        </h2>
        <p className="max-w-2xl text-sm text-ink-2">
          Updates install themselves in the background and apply the next time
          Agent Notch starts. To update right away, open{" "}
          <strong className="font-semibold text-ink">
            Settings &gt; General
          </strong>{" "}
          and choose{" "}
          <strong className="font-semibold text-ink">Check now</strong>.
        </p>
        <details className="card group p-5">
          <summary className="flex items-center gap-1.5 text-sm font-medium select-none">
            <svg
              viewBox="0 0 12 12"
              aria-hidden="true"
              className="size-3 transition-transform group-open:rotate-90"
              fill="currentColor"
            >
              <path d="M4 2.5 8 6l-4 3.5z" />
            </svg>
            Opening it for the first time
          </summary>
          <ol className="mt-4 flex flex-col gap-3 text-sm text-ink-2">
            <Step n={1}>
              Open the disk image and drag Agent Notch to{" "}
              <strong className="font-semibold text-ink">Applications</strong>,
              then open it from there. It can&apos;t update itself while it runs
              from the disk image or from Downloads.
            </Step>
            <Step n={2}>
              If macOS won&apos;t open it, open{" "}
              <strong className="font-semibold text-ink">
                System Settings &gt; Privacy &amp; Security
              </strong>
              , scroll down and choose{" "}
              <strong className="font-semibold text-ink">Open Anyway</strong>{" "}
              next to Agent Notch.
            </Step>
            <Step n={3}>
              <span className="flex flex-col gap-2">
                <span>
                  If macOS says the app is damaged, that is the quarantine flag
                  on the download, not a broken file. Clear it once in Terminal:
                </span>
                <span className="flex flex-wrap items-center gap-2">
                  <code
                    id="quarantine-command"
                    className="rounded-md bg-surface-2 px-2 py-1 font-mono text-xs break-all text-ink"
                  >
                    {QUARANTINE_COMMAND}
                  </code>
                  <CopyButton
                    value={QUARANTINE_COMMAND}
                    describedBy="quarantine-command"
                  />
                </span>
              </span>
            </Step>
          </ol>
        </details>
      </section>

      <section
        aria-labelledby="install-windows-title"
        className="flex flex-col gap-4"
      >
        <h2
          id="install-windows-title"
          className="flex items-center gap-2 text-lg font-semibold tracking-tight"
        >
          Installing on Windows
          <Badge>Preview</Badge>
        </h2>
        <p className="max-w-2xl text-sm text-ink-2">
          The Windows app is new and still a preview: it is built and tested
          automatically, but it has not been used by people yet. Please report
          anything odd. It needs Windows 10 or 11 (x64). It updates itself too:
          the app checks shortly after it starts, and{" "}
          <strong className="font-semibold text-ink">
            Settings &gt; General
          </strong>{" "}
          installs an update. Every update is verified with Agent Notch&apos;s
          own signing key before it runs.
        </p>
        <details className="card group p-5">
          <summary className="flex items-center gap-1.5 text-sm font-medium select-none">
            <svg
              viewBox="0 0 12 12"
              aria-hidden="true"
              className="size-3 transition-transform group-open:rotate-90"
              fill="currentColor"
            >
              <path d="M4 2.5 8 6l-4 3.5z" />
            </svg>
            Opening it for the first time
          </summary>
          <ol className="mt-4 flex flex-col gap-3 text-sm text-ink-2">
            <Step n={1}>
              Run the installer. It installs for your user only, with no
              administrator, into{" "}
              <code className="rounded-md bg-surface-2 px-1.5 py-0.5 font-mono text-xs text-ink">
                %LOCALAPPDATA%\Agent Notch
              </code>
              , and fetches Microsoft&apos;s WebView2 if it is missing.
            </Step>
            <Step n={2}>
              The installer isn&apos;t code-signed yet, so Windows SmartScreen
              may say &ldquo;Windows protected your PC&rdquo;. Choose{" "}
              <strong className="font-semibold text-ink">More info</strong>,
              then{" "}
              <strong className="font-semibold text-ink">Run anyway</strong>.
            </Step>
            <Step n={3}>
              With{" "}
              <strong className="font-semibold text-ink">
                Smart App Control
              </strong>{" "}
              turned on (Windows 11), unsigned apps are blocked with no way
              around it: Agent Notch can&apos;t be used there until it is
              code-signed.
            </Step>
          </ol>
        </details>
        <p className="max-w-2xl text-sm text-ink-2">
          To remove Agent Notch&apos;s Claude Code hooks when uninstalling, turn
          Claude Code control off in Settings first, or tick &ldquo;Delete the
          application data&rdquo; in the uninstaller.
        </p>
      </section>

      <p className="text-sm text-ink-2">
        Every release, with its notes, is on{" "}
        <a href={`${repoUrl(repo)}/releases`} className="link">
          GitHub
        </a>
        .
      </p>
    </div>
  );
}

/** "Version 1.0.0 · Released Sep 27 · Release notes" (the date in the viewer's time zone). */
function ReleaseSummary({ release }: { release: Release }) {
  return (
    <>
      Version {release.version}
      {release.publishedAt ? (
        <>
          {" · "}Released <DateTime date={release.publishedAt} kind="day" />
        </>
      ) : null}
      {" · "}
      <a href={release.notesUrl} className="link">
        Release notes
      </a>
    </>
  );
}

function PlatformCard({
  platform,
  downloads,
  yours,
  leading,
}: {
  platform: Platform;
  downloads: Downloads;
  yours: boolean;
  leading: boolean;
}) {
  const asset = downloads[platform];
  const name = PLATFORM_NAMES[platform];
  const about =
    platform === "mac"
      ? MAC_REQUIREMENTS
      : platform === "windows"
        ? WINDOWS_REQUIREMENTS
        : asset
          ? null
          : "Agent Notch has no Linux version yet.";

  return (
    <li
      className={cx("card flex flex-col gap-3 p-5", yours && "border-accent")}
    >
      <div className="flex items-start justify-between gap-3">
        <h3 className="font-semibold">{name}</h3>
        {yours ? <Badge tone="accent">Your computer</Badge> : null}
      </div>
      {about ? <p className="text-sm text-ink-2">{about}</p> : null}
      <div className="mt-auto flex flex-col gap-2 pt-1">
        {asset ? (
          <>
            {/* A plain link: next/link would prefetch the redirect, a GitHub call per view. */}
            <a
              href={`/download/${platform}`}
              className={cx("btn", leading ? "btn-primary" : "btn-secondary")}
            >
              <DownloadIcon />
              Download for {name}
            </a>
            <p className="text-xs break-all text-ink-3">
              {asset.name} · {formatSize(asset.size)}
            </p>
          </>
        ) : (
          <Badge className="self-start">Not available yet</Badge>
        )}
      </div>
    </li>
  );
}

function Step({ n, children }: { n: number; children: ReactNode }) {
  return (
    <li className="flex gap-3">
      <span
        aria-hidden="true"
        className="flex size-6 shrink-0 items-center justify-center rounded-full bg-accent-soft text-xs font-semibold text-accent-ink"
      >
        {n}
      </span>
      <span className="min-w-0">
        <span className="sr-only">Step {n}: </span>
        {children}
      </span>
    </li>
  );
}

function DownloadIcon() {
  return (
    <svg
      viewBox="0 0 16 16"
      aria-hidden="true"
      className="size-4"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M8 2.5v7.5M4.75 7 8 10.25 11.25 7M3 13.25h10" />
    </svg>
  );
}

function first(value: string | string[] | undefined): string | undefined {
  return Array.isArray(value) ? value[0] : value;
}
