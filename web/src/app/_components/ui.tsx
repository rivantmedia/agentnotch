/**
 * Small presentational pieces shared by the pages. No hooks here, so server and client
 * components can both use them.
 */
import Link from "next/link";
import { type ReactNode } from "react";

import { describeError } from "~/trpc/errors";

export function cx(...parts: Array<string | false | null | undefined>): string {
  return parts.filter(Boolean).join(" ");
}

export function PageHeader({
  title,
  description,
  actions,
  eyebrow,
}: {
  title: ReactNode;
  description?: ReactNode;
  actions?: ReactNode;
  eyebrow?: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-4 sm:flex-row sm:items-end sm:justify-between">
      <div className="flex min-w-0 flex-col gap-1.5">
        {eyebrow ? <div className="text-sm text-ink-2">{eyebrow}</div> : null}
        <h1 className="text-2xl font-semibold tracking-tight break-words sm:text-3xl">
          {title}
        </h1>
        {description ? (
          <div className="max-w-2xl text-ink-2">{description}</div>
        ) : null}
      </div>
      {actions ? <div className="flex shrink-0 gap-2">{actions}</div> : null}
    </div>
  );
}

export function SectionHeading({
  id,
  title,
  description,
  actions,
}: {
  id?: string;
  title: ReactNode;
  description?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <div className="flex flex-wrap items-end justify-between gap-3">
      <div className="flex min-w-0 flex-col gap-1">
        <h2 id={id} className="text-lg font-semibold tracking-tight">
          {title}
        </h2>
        {description ? (
          <p className="max-w-2xl text-sm text-ink-2">{description}</p>
        ) : null}
      </div>
      {actions ? <div className="flex flex-wrap gap-2">{actions}</div> : null}
    </div>
  );
}

type BadgeTone = "neutral" | "accent" | "warning" | "critical" | "good";

const BADGE_TONES: Record<BadgeTone, string> = {
  neutral: "bg-surface-2 text-ink-2",
  accent: "bg-accent-soft text-accent-ink",
  warning: "bg-warning-soft text-warning-ink",
  critical: "bg-critical-soft text-critical-ink",
  good: "bg-surface-2 text-good-ink",
};

export function Badge({
  children,
  tone = "neutral",
  title,
  className,
}: {
  children: ReactNode;
  tone?: BadgeTone;
  title?: string;
  className?: string;
}) {
  return (
    <span
      title={title}
      className={cx(
        "inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs font-medium whitespace-nowrap",
        BADGE_TONES[tone],
        className,
      )}
    >
      {children}
    </span>
  );
}

/** A small people icon for the "shared" badge (decorative; the badge text says it). */
export function PeopleIcon({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 16 16"
      aria-hidden="true"
      className={cx("size-3.5", className)}
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
    >
      <circle cx="6" cy="5.5" r="2.25" />
      <path d="M1.75 13c.4-2.2 2.1-3.5 4.25-3.5s3.85 1.3 4.25 3.5" />
      <path d="M10.5 3.4a2.25 2.25 0 0 1 0 4.2M12 9.8c1.2.5 2 1.6 2.25 3.2" />
    </svg>
  );
}

export function Skeleton({ className }: { className?: string }) {
  return <div aria-hidden="true" className={cx("skeleton", className)} />;
}

/** A labelled loading region: screen readers hear the label, sighted people see the shapes. */
export function LoadingBlock({
  label,
  children,
  className,
}: {
  label: string;
  children: ReactNode;
  className?: string;
}) {
  return (
    <div role="status" aria-live="polite" className={className}>
      <span className="sr-only">{label}</span>
      {children}
    </div>
  );
}

/** A failed load, phrased for people, with a way forward (retry or sign in). */
export function ErrorNotice({
  error,
  what,
  onRetry,
  retrying,
}: {
  error: unknown;
  /** What failed to load, e.g. "your accounts". */
  what: string;
  onRetry?: () => void;
  retrying?: boolean;
}) {
  const { message, signIn, retry } = describeError(error);
  return (
    <div
      role="alert"
      className="flex flex-col items-start gap-3 rounded-lg border border-critical/40 bg-critical-soft p-4 text-sm"
    >
      <div className="flex flex-col gap-1">
        <p className="font-semibold text-critical-ink">
          Couldn&apos;t load {what}.
        </p>
        <p className="text-ink-2">{message}</p>
      </div>
      {signIn ? (
        <Link href="/login" className="btn btn-secondary btn-sm">
          Sign in again
        </Link>
      ) : retry && onRetry ? (
        <button
          type="button"
          onClick={onRetry}
          disabled={retrying}
          className="btn btn-secondary btn-sm"
        >
          {retrying ? "Retrying…" : "Try again"}
        </button>
      ) : null}
    </div>
  );
}

/** A highlighted note: `warning` for anything that shares data. */
export function Callout({
  tone = "neutral",
  title,
  children,
  className,
}: {
  tone?: "neutral" | "warning";
  title?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cx(
        "flex gap-3 rounded-lg border p-3.5 text-sm",
        tone === "warning"
          ? "border-warning/60 bg-warning-soft"
          : "border-line bg-surface-2",
        className,
      )}
    >
      {tone === "warning" ? (
        <svg
          viewBox="0 0 16 16"
          aria-hidden="true"
          className="mt-0.5 size-4 shrink-0 text-warning-ink"
          fill="currentColor"
        >
          <path d="M8 1.5c.4 0 .75.2.95.55l6.2 10.9c.4.7-.1 1.55-.9 1.55H1.75c-.8 0-1.3-.85-.9-1.55l6.2-10.9A1.1 1.1 0 0 1 8 1.5Zm0 4a.75.75 0 0 0-.75.75v3.5a.75.75 0 0 0 1.5 0v-3.5A.75.75 0 0 0 8 5.5Zm0 7.25a.9.9 0 1 0 0-1.8.9.9 0 0 0 0 1.8Z" />
        </svg>
      ) : null}
      <div className="flex min-w-0 flex-col gap-1">
        {title ? (
          <p
            className={cx(
              "font-semibold",
              tone === "warning" ? "text-warning-ink" : "text-ink",
            )}
          >
            {title}
          </p>
        ) : null}
        <div className="text-ink-2">{children}</div>
      </div>
    </div>
  );
}

/** A labelled figure: the value leads, the label follows. */
export function Stat({
  label,
  value,
  detail,
  title,
}: {
  label: ReactNode;
  value: ReactNode;
  detail?: ReactNode;
  title?: string;
}) {
  return (
    <div className="flex min-w-0 flex-col gap-0.5">
      <dt className="text-xs text-ink-2">{label}</dt>
      <dd className="text-lg font-semibold tracking-tight" title={title}>
        {value}
      </dd>
      {detail ? <dd className="text-xs text-ink-3">{detail}</dd> : null}
    </div>
  );
}
