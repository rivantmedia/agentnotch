import { type Metadata } from "next";
import Link from "next/link";

import { requireViewer, siteAddress } from "~/server/pages";
import { api } from "~/trpc/server";

import { ConnectMacSteps } from "../_components/connect-mac";
import { RelativeTime } from "../_components/time";
import { PageHeader, SectionHeading } from "../_components/ui";
import { DataControls } from "./data-controls";

export const metadata: Metadata = { title: "Settings" };

export default async function SettingsPage() {
  await requireViewer("/settings");
  const [me, address] = await Promise.all([api.viewer.me(), siteAddress()]);

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-10 px-4 py-8 sm:py-10">
      <PageHeader title="Settings" />

      <section aria-labelledby="account-title" className="flex flex-col gap-4">
        <SectionHeading id="account-title" title="Your account" />
        <div className="card flex flex-col gap-4 p-5 sm:flex-row sm:items-center sm:justify-between">
          <dl className="flex min-w-0 flex-col gap-3">
            <div className="flex flex-col gap-0.5">
              <dt className="text-xs text-ink-2">Signed in as</dt>
              <dd className="font-medium break-all">
                {me.email || "No email"}
              </dd>
            </div>
            <div className="flex flex-col gap-0.5">
              <dt className="text-xs text-ink-2">What pool members see</dt>
              <dd className="font-medium break-all">
                {me.memberLabel.displayName}
                {me.memberLabel.name ? (
                  <span className="font-normal text-ink-2">
                    {" "}
                    ({me.memberLabel.name})
                  </span>
                ) : null}
              </dd>
            </div>
          </dl>
          <form action="/auth/signout" method="post">
            <button type="submit" className="btn btn-secondary">
              Sign out
            </button>
          </form>
        </div>
        <p className="text-sm text-ink-2">
          Your name and email come from your Google account. People in your
          pools see your email, with your name next to it. Signing out here
          signs out this browser only: the Mac app stays signed in and keeps
          syncing until you sign out in the app.
        </p>
      </section>

      <section aria-labelledby="connect-title" className="flex flex-col gap-4">
        <SectionHeading
          id="connect-title"
          title="Connect the Mac app"
          description="Agent Notch syncs with this website once you sign in from the app on each Mac you use."
        />
        <div className="card p-5">
          <ConnectMacSteps address={address} />
        </div>
      </section>

      <section aria-labelledby="macs-title" className="flex flex-col gap-4">
        <SectionHeading
          id="macs-title"
          title="Macs"
          description="Every Mac that has synced to your account, most recent first."
        />
        {me.devices.length === 0 ? (
          <p className="card p-5 text-sm text-ink-2">No Mac has synced yet.</p>
        ) : (
          <ul className="card divide-y divide-line">
            {me.devices.map((device) => (
              <li
                key={device.id}
                className="flex flex-wrap items-center justify-between gap-2 px-5 py-3.5"
              >
                <div className="min-w-0">
                  <p className="font-medium break-words">{device.name}</p>
                  <p className="text-xs text-ink-3">
                    Agent Notch {device.appVersion}
                  </p>
                </div>
                <p className="text-sm text-ink-2">
                  Last synced <RelativeTime date={device.lastSeenAt} />
                </p>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section aria-labelledby="privacy-title" className="flex flex-col gap-4">
        <SectionHeading id="privacy-title" title="What your Macs send" />
        <div className="card flex flex-col gap-2 p-5 text-sm text-ink-2">
          <p>
            Names and numbers only: project folder names, session titles,
            models, start and end times, token counts, cost estimates and
            usage-limit readings. Never file paths, prompts, replies or Claude
            credentials.
          </p>
          <p>
            Session summaries are sent only while &ldquo;Summarise finished
            sessions with Claude&rdquo; is on in the app, under Settings &gt;
            Claude Code &gt; Cloud.{" "}
            <Link href="/#privacy" className="link">
              The full list
            </Link>
          </p>
        </div>
      </section>

      <section aria-labelledby="data-title" className="flex flex-col gap-4">
        <SectionHeading
          id="data-title"
          title="Your data"
          description="Remove what your Macs synced to this website. Neither can be undone."
        />
        <DataControls />
      </section>
    </div>
  );
}
