"use client";

import "~/styles/globals.css";

/** When even the root layout fails: a bare page of its own (it replaces the layout). */
export default function GlobalError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  return (
    <html lang="en">
      <body>
        <main className="mx-auto flex max-w-xl flex-col items-start gap-4 px-4 py-24">
          <h1 className="text-2xl font-semibold tracking-tight">
            Agent Notch didn&apos;t load
          </h1>
          <p className="text-ink-2">
            Something went wrong on our side. Try again in a moment.
          </p>
          {error.digest ? (
            <p className="text-xs text-ink-3">
              Reference: <code className="font-mono">{error.digest}</code>
            </p>
          ) : null}
          <button type="button" onClick={reset} className="btn btn-primary">
            Try again
          </button>
        </main>
      </body>
    </html>
  );
}
