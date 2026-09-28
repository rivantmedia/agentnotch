import Link from "next/link";

import { CopyButton } from "./copy-button";

/**
 * How to connect the Mac app to this website: where to get it and the two steps. The app is
 * built with the website it syncs with (the repository's app-config.json), so there is nothing
 * to type in; the address is shown only for a copy that still asks for one (Agent Notch 1.0.0,
 * until it updates). Used by the dashboard's empty state and by Settings.
 */
export function ConnectMacSteps({ address }: { address: string | null }) {
  return (
    <div className="flex flex-col gap-4">
      <p className="text-sm text-ink-2">
        Don&apos;t have the app yet?{" "}
        <Link href="/download" className="link">
          Download Agent Notch
        </Link>
        .
      </p>
      <ol className="flex flex-col gap-3 text-sm">
        {[
          <>
            In Agent Notch on your Mac, open{" "}
            <strong className="font-semibold text-ink">
              Settings &gt; Claude Code &gt; Cloud
            </strong>
            , choose{" "}
            <strong className="font-semibold text-ink">
              Sign in with Google
            </strong>{" "}
            and pick the Google account you use here.
          </>,
          <>
            Turn on{" "}
            <strong className="font-semibold text-ink">
              Sync sessions and usage
            </strong>
            . Your Claude accounts show up here after the first sync. Session
            summaries are a separate switch below it (&ldquo;Summarise finished
            sessions with Claude&rdquo;), off until you turn it on.
          </>,
        ].map((step, i) => (
          <li key={i} className="flex gap-3">
            <span
              aria-hidden="true"
              className="flex size-6 shrink-0 items-center justify-center rounded-full bg-accent-soft text-xs font-semibold text-accent-ink"
            >
              {i + 1}
            </span>
            <span className="text-ink-2">
              <span className="sr-only">Step {i + 1}: </span>
              {step}
            </span>
          </li>
        ))}
      </ol>
      <SiteAddressField address={address} />
    </div>
  );
}

/**
 * The website's address in a read-only field, with a copy button, for a copy of the app that
 * asks for a Website before it signs in.
 */
export function SiteAddressField({ address }: { address: string | null }) {
  if (!address) {
    return (
      <p className="text-sm text-ink-2">
        If Agent Notch asks for a{" "}
        <strong className="font-semibold text-ink">Website</strong> before you
        can sign in, enter the address you opened this website at (the part
        before &ldquo;/dashboard&rdquo; in the address bar) and choose{" "}
        <strong className="font-semibold text-ink">Save</strong>.
      </p>
    );
  }
  return (
    <div className="flex flex-col gap-1.5">
      <label htmlFor="site-address" className="text-sm font-medium">
        Website address
      </label>
      <p id="site-address-hint" className="text-sm text-ink-2">
        Only if Agent Notch asks for a{" "}
        <strong className="font-semibold text-ink">Website</strong> before you
        can sign in: paste this there and choose{" "}
        <strong className="font-semibold text-ink">Save</strong>.
      </p>
      <div className="flex flex-wrap items-center gap-2">
        <input
          id="site-address"
          readOnly
          value={address}
          aria-describedby="site-address-hint"
          className="field max-w-md min-w-0 flex-1 basis-56 font-mono"
        />
        <CopyButton value={address} describedBy="site-address" />
      </div>
    </div>
  );
}
