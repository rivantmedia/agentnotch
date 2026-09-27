"use client";

import Link from "next/link";

import {
  formatCost,
  formatDuration,
  formatExact,
  formatTokens,
  isSessionActive,
  modelLabel,
  sessionDurationMs,
  sessionSourceLabel,
} from "~/lib/format";
import { type RouterOutputs } from "~/trpc/react";

import { DateTime, RelativeTime, useNow } from "./time";
import { Badge, cx } from "./ui";

export type SessionItem = RouterOutputs["sessions"]["list"]["items"][number];

/**
 * Sessions as a list. Each shows its title, project, where it ran, models, tokens (split on
 * hover and in the details line), cost, start and duration, whose it is in a pool, and its
 * summary when the owner turned summaries on.
 */
export function SessionList({
  items,
  accountName,
  showOwner,
  showProject = true,
  dimmed = false,
}: {
  items: readonly SessionItem[];
  /** When set, each session links to its account under this name (lists across accounts). */
  accountName?: (accountKey: string) => string;
  /** Show whose session it is (pooled accounts). */
  showOwner: boolean;
  /** Show its project's name; a project's own page leaves it out. */
  showProject?: boolean;
  /** A refetch is under way: keep the old rows, faded. */
  dimmed?: boolean;
}) {
  return (
    <ul
      className={cx(
        "divide-y divide-line transition-opacity",
        dimmed && "opacity-60",
      )}
      aria-busy={dimmed || undefined}
    >
      {items.map((session) => (
        <SessionRow
          key={session.id}
          session={session}
          accountName={accountName}
          showOwner={showOwner}
          showProject={showProject}
        />
      ))}
    </ul>
  );
}

function SessionRow({
  session,
  accountName,
  showOwner,
  showProject,
}: {
  session: SessionItem;
  accountName?: (accountKey: string) => string;
  showOwner: boolean;
  showProject: boolean;
}) {
  const now = useNow();
  const active = now !== null && isSessionActive(session, now);
  const cost = formatCost(session.costUsd);
  const tokens = session.tokens;
  const split = `Input ${formatExact(tokens.input)} · Output ${formatExact(tokens.output)} · Cache write ${formatExact(tokens.cacheCreation)} · Cache read ${formatExact(tokens.cacheRead)}`;

  return (
    <li className="flex flex-col gap-2 py-4 first:pt-0 last:pb-0">
      <div className="flex flex-wrap items-start justify-between gap-x-4 gap-y-1">
        <div className="flex min-w-0 flex-col gap-1">
          <h3 className="font-medium break-words">
            {session.title ?? (
              <span className="text-ink-2 italic">Untitled session</span>
            )}
          </h3>
          <p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-ink-2">
            {accountName ? (
              <Link href={`/accounts/${session.accountKey}`} className="link">
                {accountName(session.accountKey)}
              </Link>
            ) : null}
            {showProject ? (
              // A folder name can be one long word; it breaks rather than widen the page.
              <span className="min-w-0 font-medium wrap-anywhere text-ink">
                {session.project.name}
              </span>
            ) : null}
            <Badge>{sessionSourceLabel(session.source)}</Badge>
            {active ? (
              <Badge tone="good">
                <span
                  aria-hidden="true"
                  className="size-1.5 rounded-full bg-good"
                />
                Active
              </Badge>
            ) : null}
            {showOwner ? (
              <span>
                by{" "}
                <span
                  className="break-all text-ink"
                  title={session.owner.name ?? undefined}
                >
                  {session.owner.isViewer ? "you" : session.owner.displayName}
                </span>
              </span>
            ) : null}
          </p>
        </div>
        <div className="flex items-baseline gap-3 sm:flex-col sm:items-end sm:gap-0">
          <p className="font-semibold tabular-nums" title={split}>
            {formatTokens(tokens.total)}{" "}
            <span className="text-sm font-normal text-ink-2">tokens</span>
          </p>
          <p className="text-sm text-ink-2 tabular-nums">
            {cost ?? (
              <span title="Claude Code didn't report a cost for this session">
                No cost reported
              </span>
            )}
          </p>
        </div>
      </div>

      <dl className="flex flex-wrap gap-x-4 gap-y-1 text-xs text-ink-2 sm:gap-x-5">
        <div className="flex gap-1 whitespace-nowrap">
          <dt>Started</dt>
          <dd className="text-ink">
            <DateTime date={session.startedAt} />
          </dd>
        </div>
        <div className="flex gap-1 whitespace-nowrap">
          <dt>{session.endedAt ? "Ran" : "So far"}</dt>
          <dd className="text-ink">
            {formatDuration(sessionDurationMs(session))}
          </dd>
        </div>
        {!session.endedAt ? (
          <div className="flex gap-1 whitespace-nowrap">
            <dt>Last activity</dt>
            <dd className="text-ink">
              <RelativeTime date={session.lastActivityAt} />
            </dd>
          </div>
        ) : null}
        {session.models.length > 0 ? (
          <div className="flex gap-1">
            <dt>{session.models.length === 1 ? "Model" : "Models"}</dt>
            <dd className="text-ink">
              {session.models.map((m, i) => (
                <span key={`${m}-${i}`} title={m}>
                  {i > 0 ? ", " : ""}
                  {modelLabel(m)}
                </span>
              ))}
            </dd>
          </div>
        ) : null}
        <div className="flex basis-full gap-1 md:basis-auto">
          <dt className="sr-only">Tokens</dt>
          <dd className="tabular-nums">
            In {formatTokens(tokens.input)} · Out {formatTokens(tokens.output)}{" "}
            · Cache write {formatTokens(tokens.cacheCreation)} · Cache read{" "}
            {formatTokens(tokens.cacheRead)}
          </dd>
        </div>
      </dl>

      {session.summary ? (
        <details className="group rounded-md bg-surface-2 px-3 py-2 text-sm">
          <summary className="flex items-center gap-1.5 font-medium text-ink select-none">
            <svg
              viewBox="0 0 12 12"
              aria-hidden="true"
              className="size-3 transition-transform group-open:rotate-90"
              fill="currentColor"
            >
              <path d="M4 2.5 8 6l-4 3.5z" />
            </svg>
            Summary
          </summary>
          <p className="mt-2 whitespace-pre-line text-ink-2">
            {session.summary.text}
          </p>
          {session.summary.model || session.summary.generatedAt ? (
            <p className="mt-1.5 text-xs text-ink-3">
              Written
              {session.summary.model ? (
                <> by {modelLabel(session.summary.model)}</>
              ) : null}
              {session.summary.generatedAt ? (
                <>
                  {" "}
                  <RelativeTime date={session.summary.generatedAt} />
                </>
              ) : null}
            </p>
          ) : null}
        </details>
      ) : null}
    </li>
  );
}

/** Rows shaped like sessions, while they load. */
export function SessionListSkeleton({ rows = 4 }: { rows?: number }) {
  return (
    <ul className="divide-y divide-line" aria-hidden="true">
      {Array.from({ length: rows }, (_, i) => (
        <li key={i} className="flex flex-col gap-2 py-4 first:pt-0 last:pb-0">
          <div className="flex justify-between gap-4">
            <div className="skeleton h-4 w-2/3 max-w-80" />
            <div className="skeleton h-4 w-16" />
          </div>
          <div className="skeleton h-3 w-1/2 max-w-60" />
          <div className="skeleton h-3 w-3/4 max-w-96" />
        </li>
      ))}
    </ul>
  );
}
