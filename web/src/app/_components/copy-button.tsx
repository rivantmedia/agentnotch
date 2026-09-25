"use client";

import { useEffect, useRef, useState } from "react";

import { cx } from "./ui";

/**
 * Copies `value` to the clipboard and says so. Where the clipboard is unavailable (an insecure
 * origin, a denied permission) it says how to copy by hand instead.
 */
export function CopyButton({
  value,
  label = "Copy",
  copiedLabel = "Copied",
  describedBy,
  className,
}: {
  value: string;
  label?: string;
  copiedLabel?: string;
  /** Id of the element naming what is copied, for screen readers. */
  describedBy?: string;
  className?: string;
}) {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  useEffect(() => () => clearTimeout(timer.current), []);

  async function copy() {
    clearTimeout(timer.current);
    try {
      await navigator.clipboard.writeText(value);
      setState("copied");
    } catch {
      setState("failed");
    }
    timer.current = setTimeout(() => setState("idle"), 2500);
  }

  return (
    <span className="inline-flex items-center gap-2">
      <button
        type="button"
        onClick={() => void copy()}
        aria-describedby={describedBy}
        className={cx("btn btn-secondary btn-sm", className)}
      >
        <svg
          viewBox="0 0 16 16"
          aria-hidden="true"
          className="size-3.5"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.5"
        >
          {state === "copied" ? (
            <path
              d="M3 8.5 6.5 12 13 4.5"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          ) : (
            <>
              <rect x="5.25" y="5.25" width="8" height="8" rx="1.5" />
              <path d="M10.75 3.25v-.5a1 1 0 0 0-1-1h-6a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h.5" />
            </>
          )}
        </svg>
        {state === "copied" ? copiedLabel : label}
      </button>
      <span role="status" aria-live="polite" className="text-xs text-ink-2">
        {state === "failed" ? "Select it and press ⌘C to copy." : ""}
        {state === "copied" ? (
          <span className="sr-only">Copied to the clipboard.</span>
        ) : null}
      </span>
    </span>
  );
}
