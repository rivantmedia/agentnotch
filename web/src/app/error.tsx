"use client";

import Link from "next/link";
import { useEffect } from "react";

/**
 * Anything a page throws while rendering. The message stays generic: production builds hide
 * server error details anyway, and the digest is what the logs can be searched for.
 */
export default function PageError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  useEffect(() => {
    console.error(error);
  }, [error]);

  return (
    <div className="mx-auto flex max-w-xl flex-col items-start gap-4 px-4 py-16 sm:py-24">
      <h1 className="text-2xl font-semibold tracking-tight">
        This page didn&apos;t load
      </h1>
      <p className="text-ink-2">
        Something went wrong on our side. Try again, or come back in a moment.
      </p>
      {error.digest ? (
        <p className="text-xs text-ink-3">
          Reference: <code className="font-mono">{error.digest}</code>
        </p>
      ) : null}
      <div className="flex flex-wrap gap-2">
        <button type="button" onClick={reset} className="btn btn-primary">
          Try again
        </button>
        <Link href="/dashboard" className="btn btn-secondary">
          Go to the dashboard
        </Link>
      </div>
    </div>
  );
}
