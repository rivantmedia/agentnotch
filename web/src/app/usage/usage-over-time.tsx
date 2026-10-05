"use client";

import { Fragment, useMemo, useState } from "react";

import {
  formatCost,
  formatCostTick,
  formatExact,
  formatTokens,
  plural,
} from "~/lib/format";
import { type AccountSelection } from "~/lib/account-selection";
import { type UsagePeriod } from "~/lib/usage-period";
import { api, type RouterOutputs } from "~/trpc/react";

import { QueryBoundary } from "../_components/query-boundary";
import { useHydrated } from "../_components/time";
import { cx, LoadingBlock, SectionHeading, Skeleton } from "../_components/ui";
import { UsageBars } from "../_components/usage-bars";
import { usageTimelineInput } from "./queries";

type Timeline = RouterOutputs["usage"]["timeline"];
type Bucket = Timeline["buckets"][number];
type Metric = "cost" | "tokens";

/** How a bucket reads: short under its column, in full in the tooltip and the table. */
type BucketWords = { axis: string; full: string };

const UNIT_NOUN: Record<Timeline["unit"], string> = {
  day: "day",
  week: "week",
  month: "month",
};

/** The browser's time zone; UTC when it can't say (the server falls back to it too). */
function browserTimeZone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  } catch {
    return "UTC";
  }
}

/**
 * The selected accounts' cost and tokens per day (or week, or month), in the viewer's calendar,
 * as two charts side by side: each one measure on one axis. Only the browser knows the viewer's
 * time zone, so this section loads once the page runs there.
 */
export function UsageOverTime({
  period,
  selection,
  selectionKey,
  stale,
  accountName,
}: {
  period: UsagePeriod;
  selection: AccountSelection;
  /** The selection as one value, to retry a failed load when it changes. */
  selectionKey: string | null;
  stale: boolean;
  accountName: (accountKey: string) => string;
}) {
  const hydrated = useHydrated();
  const timeZone = useMemo(
    () => (hydrated ? browserTimeZone() : null),
    [hydrated],
  );
  return (
    <section aria-labelledby="over-time-title" className="flex flex-col gap-4">
      <SectionHeading
        id="over-time-title"
        title="Over time"
        description="The selected accounts added up, by when each session started, in your time zone. Point at a column for its split by account."
      />
      {timeZone === null ? (
        <LoadingBlock label="Loading usage over time…">
          <ChartsSkeleton />
        </LoadingBlock>
      ) : (
        <QueryBoundary
          what="usage over time"
          fallback={
            <LoadingBlock label="Loading usage over time…">
              <ChartsSkeleton />
            </LoadingBlock>
          }
          resetKeys={[period, selectionKey]}
        >
          <Charts
            input={usageTimelineInput(period, selection, timeZone)}
            stale={stale}
            accountName={accountName}
          />
        </QueryBoundary>
      )}
    </section>
  );
}

function ChartsSkeleton() {
  return (
    <div className="grid gap-4 md:grid-cols-2" aria-hidden="true">
      {[0, 1].map((i) => (
        <div key={i} className="card flex flex-col gap-3 p-5">
          <Skeleton className="h-4 w-28" />
          <Skeleton className="h-44 w-full" />
        </div>
      ))}
    </div>
  );
}

function Charts({
  input,
  stale,
  accountName,
}: {
  input: ReturnType<typeof usageTimelineInput>;
  stale: boolean;
  accountName: (accountKey: string) => string;
}) {
  const [timeline] = api.usage.timeline.useSuspenseQuery(input);
  const [active, setActive] = useState<number | null>(null);
  const words = useMemo(() => bucketWords(timeline), [timeline]);
  const { buckets } = timeline;
  const unit = UNIT_NOUN[timeline.unit];

  if (buckets.length === 0) {
    return (
      <p
        className={cx(
          "card p-5 text-sm text-ink-2 transition-opacity",
          stale && "opacity-60",
        )}
      >
        No sessions on these accounts yet.
      </p>
    );
  }

  const split = timeline.accountKeys.length > 1;
  const hasCost = buckets.some((b) => b.costUsd !== null);

  return (
    <div className="flex flex-col gap-4">
      <div className="grid gap-4 md:grid-cols-2">
        <div className="card min-w-0 p-5">
          {hasCost ? (
            <UsageBars
              title={`Cost per ${unit}`}
              values={buckets.map((b) => b.costUsd ?? 0)}
              labels={words.map((w) => w.axis)}
              describe={(i) =>
                `${formatCost(buckets[i]!.costUsd) ?? "No cost estimate"} on ${words[i]!.full}`
              }
              tooltip={(i) => (
                <BucketTip
                  bucket={buckets[i]!}
                  words={words[i]!}
                  metric="cost"
                  accountName={split ? accountName : undefined}
                />
              )}
              formatTick={formatCostTick}
              active={active}
              onActive={setActive}
              dimmed={stale}
            />
          ) : (
            <div className="flex h-full flex-col gap-2">
              <p className="text-sm font-medium">Cost per {unit}</p>
              <p className="text-sm text-ink-2">
                No cost was reported or estimated for these sessions.
              </p>
            </div>
          )}
        </div>
        <div className="card min-w-0 p-5">
          <UsageBars
            title={`Tokens per ${unit}`}
            values={buckets.map((b) => Number(b.tokens.total))}
            labels={words.map((w) => w.axis)}
            describe={(i) =>
              `${formatExact(buckets[i]!.tokens.total)} tokens on ${words[i]!.full}`
            }
            tooltip={(i) => (
              <BucketTip
                bucket={buckets[i]!}
                words={words[i]!}
                metric="tokens"
                accountName={split ? accountName : undefined}
              />
            )}
            formatTick={formatTokens}
            integer
            active={active}
            onActive={setActive}
            dimmed={stale}
          />
        </div>
      </div>
      <BucketTable
        timeline={timeline}
        words={words}
        accountName={split ? accountName : undefined}
      />
    </div>
  );
}

function metricText(
  totals: { costUsd: number | null; tokens: { total: bigint } },
  metric: Metric,
): string {
  return metric === "cost"
    ? (formatCost(totals.costUsd) ?? "No cost estimate")
    : `${formatTokens(totals.tokens.total)} tokens`;
}

/** A column's tooltip: its value first, then when, then each account's part. */
function BucketTip({
  bucket,
  words,
  metric,
  accountName,
}: {
  bucket: Bucket;
  words: BucketWords;
  metric: Metric;
  /** Lists the accounts' parts; left out when only one account is added up. */
  accountName?: (accountKey: string) => string;
}) {
  const parts = [...bucket.accounts].sort((a, b) =>
    metric === "cost"
      ? (b.costUsd ?? -1) - (a.costUsd ?? -1)
      : a.tokens.total === b.tokens.total
        ? 0
        : a.tokens.total > b.tokens.total
          ? -1
          : 1,
  );
  const listed = parts.slice(0, 4);
  const more = parts.length - listed.length;
  return (
    <div className="flex flex-col gap-0.5">
      <p className="text-sm font-semibold text-ink tabular-nums">
        {metricText(bucket, metric)}
      </p>
      <p className="text-ink-2">{words.full}</p>
      {accountName && listed.length > 0 ? (
        <ul className="mt-1 flex flex-col gap-0.5 border-t border-line pt-1">
          {listed.map((part) => (
            <li
              key={part.accountKey}
              className="flex items-baseline justify-between gap-3"
            >
              <span className="min-w-0 truncate text-ink-2">
                {accountName(part.accountKey)}
              </span>
              <span className="shrink-0 text-ink tabular-nums">
                {metricText(part, metric)}
              </span>
            </li>
          ))}
          {more > 0 ? (
            <li className="text-ink-3">
              {plural(more, "more account", "more accounts")}
            </li>
          ) : null}
        </ul>
      ) : null}
      <p className="text-ink-3 tabular-nums">
        {bucket.sessions === 0
          ? "No sessions"
          : plural(bucket.sessions, "session", "sessions")}
      </p>
    </div>
  );
}

/**
 * The words for each bucket, in the zone the server counted in (the viewer's, unless their
 * browser named one it didn't know). A rolling period's first bucket starts partway through its
 * day, and the last one ends now; both say so.
 */
function bucketWords(timeline: Timeline): BucketWords[] {
  const timeZone = timeline.timeZone;
  const year = new Intl.DateTimeFormat("en-US", { timeZone, year: "numeric" });
  const thisYear = year.format(timeline.to);
  const day = new Intl.DateTimeFormat("en-US", {
    timeZone,
    month: "short",
    day: "numeric",
  });
  const dayYear = new Intl.DateTimeFormat("en-US", {
    timeZone,
    month: "short",
    day: "numeric",
    year: "numeric",
  });
  const weekday = new Intl.DateTimeFormat("en-US", {
    timeZone,
    weekday: "short",
  });
  const month = new Intl.DateTimeFormat("en-US", {
    timeZone,
    month: "short",
    year: "numeric",
  });
  const monthLong = new Intl.DateTimeFormat("en-US", {
    timeZone,
    month: "long",
    year: "numeric",
  });
  const time = new Intl.DateTimeFormat("en-US", {
    timeZone,
    hour: "numeric",
    minute: "2-digit",
  });

  const last = timeline.buckets.length - 1;
  return timeline.buckets.map((bucket, i) => {
    const date =
      year.format(bucket.start) === thisYear
        ? day.format(bucket.start)
        : dayYear.format(bucket.start);
    let axis: string;
    let full: string;
    if (timeline.unit === "month") {
      axis = month.format(bucket.start);
      full = monthLong.format(bucket.start);
    } else if (timeline.unit === "week") {
      axis = date;
      full = `Week of ${date}`;
    } else {
      axis = date;
      full = `${weekday.format(bucket.start)}, ${date}`;
    }
    const startsAt = time.format(bucket.start);
    if (i === 0 && timeline.from !== null && startsAt !== "12:00 AM") {
      full += `, from ${startsAt}`;
    }
    if (i === last) full += " (so far)";
    return { axis, full };
  });
}

/**
 * The charts' table twin: each day (or week, or month) with sessions, newest first, with every
 * account's part when more than one is added up.
 */
function BucketTable({
  timeline,
  words,
  accountName,
}: {
  timeline: Timeline;
  words: BucketWords[];
  accountName?: (accountKey: string) => string;
}) {
  const unit = UNIT_NOUN[timeline.unit];
  const rows = timeline.buckets
    .map((bucket, i) => ({ bucket, words: words[i]! }))
    .filter((row) => row.bucket.sessions > 0)
    .reverse();
  const cell = "py-1.5 pl-4 text-right";

  return (
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
        Show the charts as a table
      </summary>
      <div
        className="mt-4 max-h-96 overflow-auto"
        tabIndex={0}
        role="region"
        aria-label={`Usage per ${unit}`}
      >
        {rows.length === 0 ? (
          <p className="text-sm text-ink-2">No sessions in this period.</p>
        ) : (
          <table className="w-full text-sm tabular-nums">
            <caption className="sr-only">
              Sessions, tokens and cost per {unit}, newest first
            </caption>
            <thead className="sticky top-0 bg-surface text-left text-xs text-ink-2">
              <tr>
                <th scope="col" className="py-2 pr-4 font-medium capitalize">
                  {unit}
                </th>
                {accountName ? (
                  <th scope="col" className="py-2 pr-4 font-medium">
                    Account
                  </th>
                ) : null}
                <th scope="col" className="py-2 pl-4 text-right font-medium">
                  Sessions
                </th>
                <th scope="col" className="py-2 pl-4 text-right font-medium">
                  Tokens
                </th>
                <th scope="col" className="py-2 pl-4 text-right font-medium">
                  Cost
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.map(({ bucket, words: w }) => {
                const parts = accountName ? bucket.accounts : [];
                return (
                  <Fragment key={bucket.start.toISOString()}>
                    <tr className="border-t border-line">
                      <th
                        scope="row"
                        rowSpan={1 + parts.length}
                        className="py-1.5 pr-4 text-left align-top font-normal text-ink-2"
                      >
                        {w.full}
                      </th>
                      {accountName ? (
                        <td className="py-1.5 pr-4 font-medium">All</td>
                      ) : null}
                      <Totals bucket={bucket} cell={cell} strong />
                    </tr>
                    {parts.map((part) => (
                      <tr key={part.accountKey} className="text-ink-2">
                        <td className="max-w-48 truncate py-1 pr-4">
                          {accountName!(part.accountKey)}
                        </td>
                        <Totals bucket={part} cell={cell} />
                      </tr>
                    ))}
                  </Fragment>
                );
              })}
            </tbody>
          </table>
        )}
      </div>
    </details>
  );
}

function Totals({
  bucket,
  cell,
  strong = false,
}: {
  bucket: {
    sessions: number;
    tokens: { total: bigint };
    costUsd: number | null;
  };
  cell: string;
  strong?: boolean;
}) {
  return (
    <>
      <td className={cx(cell, strong && "font-medium")}>
        {formatExact(bucket.sessions)}
      </td>
      <td
        className={cx(cell, strong && "font-medium")}
        title={`${formatExact(bucket.tokens.total)} tokens`}
      >
        {formatTokens(bucket.tokens.total)}
      </td>
      <td className={cx(cell, strong && "font-medium")}>
        {formatCost(bucket.costUsd) ?? <span className="text-ink-3">–</span>}
      </td>
    </>
  );
}
