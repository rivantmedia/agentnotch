"use client";

import { useSearchParams } from "next/navigation";
import {
  useCallback,
  useDeferredValue,
  useId,
  useState,
  type ReactNode,
} from "react";

import {
  formatCost,
  formatExact,
  formatShare,
  formatTokens,
  plural,
  shareOf,
} from "~/lib/format";
import {
  DEFAULT_USAGE_PERIOD,
  periodFromParam,
  periodLabel,
  USAGE_PERIODS,
  type UsagePeriod,
} from "~/lib/usage-period";

import { RelativeTime } from "./time";
import { cx, Skeleton } from "./ui";

/** What a breakdown row adds up (project-usage.ts, `UsageTotals`). */
export type BreakdownTotals = {
  sessions: number;
  tokens: {
    input: bigint;
    output: bigint;
    cacheCreation: bigint;
    cacheRead: bigint;
    total: bigint;
  };
  costUsd: number | null;
};

export type BreakdownRow = {
  key: string;
  /** What the row is: a project or an account, usually a link. */
  label: ReactNode;
  /** A line under the bar: where it ran, whose it is. */
  detail?: ReactNode;
  /** A control at the end of the name line, e.g. showing its sessions. */
  action?: ReactNode;
  totals: BreakdownTotals;
  lastUsedAt?: Date | null;
  /** The folded tail ("12 more projects"): drawn in gray, so it never reads as one project. */
  rest?: boolean;
};

/**
 * The period a breakdown shows, kept in `?period=` so a view can be reloaded or shared. The
 * breakdown follows it a render behind (`shown`): while the new period loads, the old rows stay,
 * faded, instead of flashing back to a skeleton.
 *
 * The address bar is the source of truth, and `initial` (the server's reading of it) only a
 * fallback: Back restores a page with the props of its first render while the address keeps the
 * period picked since, and a link to the same page changes the address without remounting.
 */
export function usePeriod(initial: UsagePeriod) {
  // An address without `?period=` means the default period. Outside the App Router (a component
  // rendered on its own) there are no search params, and `initial` stands in.
  const params = useSearchParams();
  const fromUrl = params
    ? periodFromParam(params.get("period") ?? undefined)
    : initial;
  const [period, setPeriod] = useState(fromUrl);
  const [seen, setSeen] = useState(fromUrl);
  if (fromUrl !== seen) {
    // Adjusting state while rendering, as React documents, so the view never shows a period the
    // address doesn't. After `choose`, Next syncs the address it set, and this changes nothing.
    setSeen(fromUrl);
    setPeriod(fromUrl);
  }
  const shown = useDeferredValue(period);
  const choose = useCallback((next: UsagePeriod) => {
    setPeriod(next);
    // Keep the URL in step without a navigation (Next.js syncs history.replaceState).
    const url = new URL(window.location.href);
    if (next === DEFAULT_USAGE_PERIOD) url.searchParams.delete("period");
    else url.searchParams.set("period", next);
    window.history.replaceState(null, "", url);
  }, []);
  return { period, shown, stale: shown !== period, choose };
}

/** 7 days · 30 days · All time, as one row of radio buttons. */
export function PeriodPicker({
  value,
  onChange,
  label = "Period",
}: {
  value: UsagePeriod;
  onChange: (period: UsagePeriod) => void;
  label?: string;
}) {
  const name = useId();
  return (
    <fieldset>
      <legend className="sr-only">{label}</legend>
      <div className="inline-flex rounded-lg border border-edge bg-surface p-0.5">
        {USAGE_PERIODS.map((period) => {
          const checked = value === period;
          return (
            <label
              key={period}
              className={cx(
                "cursor-pointer rounded-md px-2.5 py-1 text-sm font-medium whitespace-nowrap transition-colors",
                "has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-focus",
                // Filled like a primary button, so the chosen period stands out by more than hue.
                checked
                  ? "bg-accent text-on-accent"
                  : "text-ink-2 hover:bg-surface-2 hover:text-ink",
              )}
            >
              <input
                type="radio"
                name={name}
                value={period}
                checked={checked}
                onChange={() => onChange(period)}
                className="sr-only"
              />
              {periodLabel(period)}
            </label>
          );
        })}
      </div>
    </fieldset>
  );
}

/**
 * Usage split into parts (projects, or a project's accounts) as ranked bars: each part's tokens
 * as a share of `whole`, one hue on a same-hue track like the limit meters. Every number is also
 * written out beside its bar, so nothing depends on reading the bar or hovering it.
 */
export function UsageBreakdown({
  rows,
  whole,
  label,
  dimmed = false,
}: {
  rows: readonly BreakdownRow[];
  /** The tokens the shares are of: every part added up, the folded tail included. */
  whole: bigint;
  /** What the list is, for screen readers: "Usage by project". */
  label: string;
  /** A refetch is under way: keep the old rows, faded. */
  dimmed?: boolean;
}) {
  return (
    <ol
      aria-label={label}
      aria-busy={dimmed || undefined}
      className={cx(
        "divide-y divide-line transition-opacity",
        dimmed && "opacity-60",
      )}
    >
      {rows.map((row) => (
        <BreakdownItem key={row.key} row={row} whole={whole} />
      ))}
    </ol>
  );
}

function BreakdownItem({ row, whole }: { row: BreakdownRow; whole: bigint }) {
  const { tokens, sessions, costUsd } = row.totals;
  const share = shareOf(tokens.total, whole);
  const cost = formatCost(costUsd);
  const split = `Input ${formatExact(tokens.input)} · Output ${formatExact(tokens.output)} · Cache write ${formatExact(tokens.cacheCreation)} · Cache read ${formatExact(tokens.cacheRead)}`;

  return (
    <li className="flex flex-col gap-1.5 py-3.5 first:pt-0 last:pb-0">
      <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1">
        <div className="flex min-w-0 flex-wrap items-baseline gap-x-3 gap-y-1">
          <div className="min-w-0 font-medium break-words">{row.label}</div>
          {row.action}
        </div>
        <p className="text-sm whitespace-nowrap tabular-nums">
          <span className="font-semibold" title={split}>
            {formatTokens(tokens.total)}
          </span>{" "}
          <span className="text-ink-2">
            tokens · <span className="text-ink">{formatShare(share)}</span>
          </span>
        </p>
      </div>
      <ShareBar share={share} rest={row.rest === true} />
      <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1 text-xs text-ink-2">
        <div className="min-w-0">{row.detail}</div>
        <p className="tabular-nums">
          {sessions === 0 ? (
            "No sessions in this period"
          ) : (
            <>
              {cost ?? (
                <span title="Claude Code didn't report a cost for these sessions">
                  No cost reported
                </span>
              )}{" "}
              · {plural(sessions, "session", "sessions")}
            </>
          )}
          {row.lastUsedAt ? (
            <>
              {" "}
              · last used <RelativeTime date={row.lastUsedAt} />
            </>
          ) : null}
        </p>
      </div>
      {sessions > 0 ? (
        <p className="text-xs text-ink-3 tabular-nums">
          <span className="sr-only">Tokens: </span>
          In {formatTokens(tokens.input)} · Out {formatTokens(tokens.output)} ·
          Cache write {formatTokens(tokens.cacheCreation)} · Cache read{" "}
          {formatTokens(tokens.cacheRead)}
        </p>
      ) : null}
    </li>
  );
}

/** The bar: decorative, since the share is written beside it. */
function ShareBar({ share, rest }: { share: number; rest: boolean }) {
  const fill = rest ? "var(--color-edge)" : "var(--color-series)";
  // A part that is there but rounds to nothing still shows a sliver.
  const width = share > 0 ? Math.max(share * 100, 0.75) : 0;
  return (
    <div
      aria-hidden="true"
      className="h-2 w-full overflow-hidden rounded-full"
      style={{
        backgroundColor: `color-mix(in oklab, ${fill} 14%, var(--color-surface))`,
      }}
    >
      <div
        className="h-full rounded-full"
        style={{ width: `${width}%`, backgroundColor: fill }}
      />
    </div>
  );
}

/** Rows shaped like a breakdown, while it loads. */
export function UsageBreakdownSkeleton({ rows = 4 }: { rows?: number }) {
  return (
    <ul className="divide-y divide-line" aria-hidden="true">
      {Array.from({ length: rows }, (_, i) => (
        <li key={i} className="flex flex-col gap-2 py-3.5 first:pt-0 last:pb-0">
          <div className="flex justify-between gap-4">
            <Skeleton className="h-4 w-40 max-w-1/2" />
            <Skeleton className="h-4 w-28" />
          </div>
          <Skeleton className="h-2 w-full rounded-full" />
          <Skeleton className="h-3 w-56 max-w-2/3" />
        </li>
      ))}
    </ul>
  );
}

/** "12 more projects", added up: the folded tail of a breakdown. */
export function restRow(
  rest: (BreakdownTotals & { projects: number }) | null,
  noun: { one: string; many: string },
): BreakdownRow[] {
  if (!rest) return [];
  return [
    {
      key: "rest",
      label: (
        <span className="text-ink-2">
          {plural(rest.projects, `more ${noun.one}`, `more ${noun.many}`)}
        </span>
      ),
      totals: rest,
      rest: true,
    },
  ];
}
