import Link from "next/link";
import { unstable_rethrow } from "next/navigation";

import { api } from "~/trpc/server";

import { GoogleSignInButton } from "./login/google-sign-in-button";

const FEATURES = [
  {
    title: "Every account in one place",
    body: "One card per Claude account, however many you use: its 5-hour and weekly limits, when they reset, and what it used this week. Readings come from Claude Code and from the Claude Desktop app.",
  },
  {
    title: "Sessions by project",
    body: "Each Claude Code session with its title, project folder name, models, tokens and Claude Code's cost estimate, grouped by the project it ran in.",
  },
  {
    title: "Every Mac you use",
    body: "Each Mac running Agent Notch syncs on its own, so a laptop and a desktop add up to one history.",
  },
  {
    title: "Summaries, only if you want them",
    body: "Turn on session summaries in the app and Claude writes a sentence or two about each session on your Mac, using a little of your own Claude usage. They stay off until you turn them on.",
  },
  {
    title: "Pools for shared accounts",
    body: "Share one Claude account with the people who use it too. Everyone who joins with your code sees that account's sessions and usage, and your email, and nothing else of yours.",
  },
] as const;

const SENT = [
  "Project folder names (the last part of the path, never the path)",
  "Session titles, models, start and end times",
  "Token counts and Claude Code's cost estimate",
  "Usage-limit percentages and reset times",
  "Each account's email, plan and your own name for it",
  "Your Mac's name and the app version",
  "Session summaries, only when you turn them on",
] as const;

const NEVER_SENT = [
  "File paths or file contents",
  "Your prompts or Claude's replies",
  "Claude login tokens or any other credential",
] as const;

export default async function Home() {
  let signedIn = false;
  try {
    signedIn = (await api.viewer.current()) !== null;
  } catch (error) {
    unstable_rethrow(error);
  }

  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-20 px-4 py-12 sm:py-20">
      <section
        aria-labelledby="hero-title"
        className="grid items-center gap-12 lg:grid-cols-[1.15fr_1fr]"
      >
        <div className="flex flex-col gap-6">
          <p className="text-sm font-medium text-accent-ink">
            For Claude Code on the Mac
          </p>
          <h1
            id="hero-title"
            className="text-4xl font-semibold tracking-tight text-balance sm:text-5xl"
          >
            See what every Claude account was used for.
          </h1>
          <p className="max-w-xl text-lg text-ink-2">
            Agent Notch keeps your Claude usage limits in the Mac&apos;s notch.
            Turn on sync, and this site gathers it from every Mac you use: each
            account&apos;s limits, the projects it worked on, and the tokens
            every session took.
          </p>
          {signedIn ? (
            <div className="flex flex-wrap items-center gap-x-4 gap-y-3">
              <Link href="/dashboard" className="btn btn-primary px-5">
                Open dashboard
              </Link>
              <DownloadLinks />
            </div>
          ) : (
            <>
              <div className="flex max-w-sm flex-col gap-3">
                <GoogleSignInButton next="/dashboard" />
                <p className="text-sm text-ink-3">
                  Google is used only to sign you in: your name and email.
                </p>
              </div>
              <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
                <DownloadLinks />
              </div>
            </>
          )}
        </div>
        <ExampleCard />
      </section>

      <section aria-labelledby="features-title" className="flex flex-col gap-8">
        <h2
          id="features-title"
          className="text-2xl font-semibold tracking-tight"
        >
          What you get
        </h2>
        <ul className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {FEATURES.map((feature) => (
            <li key={feature.title} className="card flex flex-col gap-2 p-5">
              <h3 className="font-semibold">{feature.title}</h3>
              <p className="text-sm text-ink-2">{feature.body}</p>
            </li>
          ))}
        </ul>
      </section>

      <section aria-labelledby="connect-title" className="flex flex-col gap-6">
        <h2
          id="connect-title"
          className="text-2xl font-semibold tracking-tight"
        >
          Connect a Mac
        </h2>
        <ol className="grid gap-4 md:grid-cols-3">
          {[
            [
              "Sign in here",
              "Use the Google account you want your history under. Each Mac can sign in to the same one.",
            ],
            [
              "Open the app's settings",
              "In Agent Notch, open Settings > Claude Code > Cloud and save this website's address under Website.",
            ],
            [
              "Sign in and turn on sync",
              "Choose Sign in with Google, then turn on Sync sessions and usage. Your accounts appear on the dashboard after the first sync.",
            ],
          ].map(([title, body], i) => (
            <li key={title} className="card flex gap-4 p-5">
              <span
                aria-hidden="true"
                className="flex size-7 shrink-0 items-center justify-center rounded-full bg-accent-soft text-sm font-semibold text-accent-ink"
              >
                {i + 1}
              </span>
              <div className="flex flex-col gap-1">
                <h3 className="font-semibold">{title}</h3>
                <p className="text-sm text-ink-2">{body}</p>
              </div>
            </li>
          ))}
        </ol>
      </section>

      <section
        id="privacy"
        aria-labelledby="privacy-title"
        className="card grid scroll-mt-24 gap-8 p-6 sm:p-8 md:grid-cols-[1fr_1fr]"
      >
        <div className="flex flex-col gap-3 md:col-span-2">
          <h2
            id="privacy-title"
            className="text-2xl font-semibold tracking-tight"
          >
            What the app sends
          </h2>
          <p className="max-w-2xl text-ink-2">
            Names and numbers only. The app never reads a Claude login token,
            and summaries are sent only when you turn them on in the app.
          </p>
        </div>
        <div className="flex flex-col gap-3">
          <h3 className="font-semibold">Sent when sync is on</h3>
          <ul className="flex flex-col gap-2 text-sm text-ink-2">
            {SENT.map((item) => (
              <li key={item} className="flex gap-2">
                <svg
                  viewBox="0 0 16 16"
                  aria-hidden="true"
                  className="mt-0.5 size-4 shrink-0 text-good-ink"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.75"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                >
                  <path d="M3.5 8.5 6.5 11.5 12.5 5" />
                </svg>
                {item}
              </li>
            ))}
          </ul>
        </div>
        <div className="flex flex-col gap-3">
          <h3 className="font-semibold">Never sent</h3>
          <ul className="flex flex-col gap-2 text-sm text-ink-2">
            {NEVER_SENT.map((item) => (
              <li key={item} className="flex gap-2">
                <svg
                  viewBox="0 0 16 16"
                  aria-hidden="true"
                  className="mt-0.5 size-4 shrink-0 text-critical-ink"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.75"
                  strokeLinecap="round"
                >
                  <path d="M4.5 4.5l7 7M11.5 4.5l-7 7" />
                </svg>
                {item}
              </li>
            ))}
          </ul>
        </div>
      </section>
    </div>
  );
}

/** Links only: the landing page never waits on GitHub (the download page does). */
function DownloadLinks() {
  return (
    <>
      {/* /download/mac redirects to the file, it is no page: next/link would prefetch it (a
          GitHub lookup per view) and navigate to it in the client. */}
      {/* eslint-disable-next-line @next/next/no-html-link-for-pages */}
      <a href="/download/mac" className="btn btn-secondary px-4">
        Download for Mac
      </a>
      <Link href="/download" className="link text-sm">
        Other platforms
      </Link>
    </>
  );
}

/** A static picture of a dashboard card, labelled as an example. */
function ExampleCard() {
  const meters = [
    { label: "5-hour", value: 42, note: "Resets in 2 h 10 min" },
    { label: "Weekly", value: 61, note: "Resets Mon 9:00 AM" },
  ];
  return (
    <figure className="card flex flex-col gap-5 p-6 shadow-sm">
      <figcaption className="sr-only">
        An example account card: 5-hour limit 42% used, weekly limit 61% used,
        38 sessions and 212 million tokens in the last 7 days.
      </figcaption>
      <div aria-hidden="true" className="flex flex-col gap-5">
        <div className="flex items-start justify-between gap-3">
          <div>
            <p className="font-semibold">Personal</p>
            <p className="text-sm text-ink-2">you@example.com</p>
          </div>
          <div className="flex gap-1.5">
            <span className="rounded-full bg-surface-2 px-2 py-0.5 text-xs font-medium text-ink-2">
              Max 20x
            </span>
            <span className="rounded-full bg-accent-soft px-2 py-0.5 text-xs font-medium text-accent-ink">
              Shared · 2
            </span>
          </div>
        </div>
        {meters.map((m) => (
          <div key={m.label} className="flex flex-col gap-1.5">
            <div className="flex justify-between text-sm">
              <span className="font-medium">{m.label}</span>
              <span className="font-semibold">{m.value}%</span>
            </div>
            <div
              className="h-2 overflow-hidden rounded-full"
              style={{
                backgroundColor:
                  "color-mix(in oklab, var(--color-series) 16%, var(--color-surface))",
              }}
            >
              <div
                className="h-full rounded-full bg-series"
                style={{ width: `${m.value}%` }}
              />
            </div>
            <p className="text-xs text-ink-3">{m.note}</p>
          </div>
        ))}
        <dl className="grid grid-cols-3 gap-3 border-t border-line pt-4">
          {[
            ["Sessions", "38"],
            ["Tokens", "212M"],
            ["Cost", "$184.20"],
          ].map(([label, value]) => (
            <div key={label}>
              <dt className="text-xs text-ink-2">{label}, 7 days</dt>
              <dd className="text-lg font-semibold">{value}</dd>
            </div>
          ))}
        </dl>
      </div>
      <p className="text-xs text-ink-3">Example</p>
    </figure>
  );
}
