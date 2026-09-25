"use client";

import { useMemo } from "react";

import {
  compareWindowIds,
  formatPercent,
  windowDescription,
  windowLabel,
} from "~/lib/format";
import { dailyPeaks, toSeries } from "~/lib/usage-chart";
import { api, type RouterOutputs } from "~/trpc/react";

import { QueryBoundary } from "../../_components/query-boundary";
import { formatDay, useHydrated } from "../../_components/time";
import {
  cx,
  LoadingBlock,
  SectionHeading,
  Skeleton,
} from "../../_components/ui";
import { UsageMeter, type MeterReading } from "../../_components/usage-meter";
import { UsageSparkline } from "../../_components/usage-sparkline";
import { USAGE_HISTORY_DAYS, usageInput } from "./queries";

type HistoryData = RouterOutputs["usage"]["history"];

export function UsageSection({
  accountKey,
  current,
  fromMs,
}: {
  accountKey: string;
  current: MeterReading[];
  fromMs: number;
}) {
  return (
    <section aria-labelledby="usage-title" className="flex flex-col gap-4">
      <SectionHeading
        id="usage-title"
        title="Usage limits"
        description="The latest reading of each limit, and how it moved over the last 30 days. Readings come from Claude Code and Claude Desktop."
      />

      {current.length > 0 ? (
        <div className="card grid gap-5 p-5 sm:grid-cols-2 lg:grid-cols-3">
          {current.map((reading) => (
            <UsageMeter key={reading.windowId} reading={reading} />
          ))}
        </div>
      ) : null}

      <QueryBoundary
        what="the usage history"
        fallback={
          <LoadingBlock label="Loading usage history…">
            <div className="grid gap-4 md:grid-cols-2">
              {[0, 1].map((i) => (
                <div
                  key={i}
                  className="card flex flex-col gap-3 p-5"
                  aria-hidden="true"
                >
                  <Skeleton className="h-4 w-40" />
                  <Skeleton className="h-24 w-full" />
                </div>
              ))}
            </div>
          </LoadingBlock>
        }
      >
        <History accountKey={accountKey} fromMs={fromMs} />
      </QueryBoundary>
    </section>
  );
}

function History({
  accountKey,
  fromMs,
}: {
  accountKey: string;
  fromMs: number;
}) {
  const [history] = api.usage.history.useSuspenseQuery(
    usageInput(accountKey, fromMs),
  );
  if (history.windows.length === 0) {
    return (
      <p className="card p-5 text-sm text-ink-2">
        No usage readings in the last {USAGE_HISTORY_DAYS} days.
      </p>
    );
  }
  return <HistoryCharts history={history} />;
}

function HistoryCharts({ history }: { history: HistoryData }) {
  const windows = useMemo(
    () =>
      // The fixed windows first, like the meters above.
      [...history.windows].sort((a, b) =>
        compareWindowIds(a.windowId, b.windowId),
      ),
    [history.windows],
  );

  return (
    <div className="flex flex-col gap-4">
      <ul
        className={cx(
          "grid gap-4 md:grid-cols-2",
          // Three charts fill a row; two or four pair up.
          windows.length % 3 === 0 && "lg:grid-cols-3",
        )}
      >
        {windows.map((w) => (
          <li key={w.windowId} className="card p-5">
            <UsageSparkline
              windowId={w.windowId}
              points={w.points}
              from={history.from}
              to={history.to}
            />
          </li>
        ))}
      </ul>
      {history.truncated ? (
        <p className="text-xs text-ink-3">
          Only the most recent readings are shown; older ones in this period
          were left out.
        </p>
      ) : null}
      <PeaksTable windows={windows} from={history.from} to={history.to} />
    </div>
  );
}

function localDayKey(t: number): string {
  const d = new Date(t);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

/** The charts' table twin: each window's highest reading per day, newest day first. */
function PeaksTable({
  windows,
  from,
  to,
}: {
  windows: HistoryData["windows"];
  from: Date;
  to: Date;
}) {
  const hydrated = useHydrated();

  const rows = useMemo(() => {
    if (!hydrated) return [];
    const days: Array<{ key: string; date: Date }> = [];
    const cursor = new Date(to);
    cursor.setHours(0, 0, 0, 0);
    while (cursor.getTime() >= from.getTime() - 24 * 60 * 60 * 1000) {
      days.push({ key: localDayKey(cursor.getTime()), date: new Date(cursor) });
      cursor.setDate(cursor.getDate() - 1);
    }
    const peaks = dailyPeaks(
      windows.map((w) => ({
        windowId: w.windowId,
        series: toSeries(w.points),
      })),
      localDayKey,
      days.map((d) => d.key),
    );
    return peaks
      .map((row, i) => ({ ...row, date: days[i]!.date }))
      .filter((row) => row.peaks.some((p) => p !== null));
  }, [hydrated, windows, from, to]);

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
        Show the charts as a table (daily peaks)
      </summary>
      <div
        className="mt-4 max-h-96 overflow-auto"
        tabIndex={0}
        role="region"
        aria-label="Daily peak utilization per limit"
      >
        {rows.length === 0 ? (
          <p className="text-sm text-ink-2">No readings in this period.</p>
        ) : (
          <table className="w-full text-sm tabular-nums">
            <caption className="sr-only">
              Highest utilization of each limit per day, newest first
            </caption>
            <thead className="sticky top-0 bg-surface text-left text-xs text-ink-2">
              <tr>
                <th scope="col" className="py-2 pr-4 font-medium">
                  Day
                </th>
                {windows.map((w) => (
                  <th
                    key={w.windowId}
                    scope="col"
                    className="py-2 pr-4 text-right font-medium"
                    title={windowDescription(w.windowId)}
                  >
                    {windowLabel(w.windowId)}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody className="divide-y divide-line">
              {rows.map((row) => (
                <tr key={row.day}>
                  <th
                    scope="row"
                    className="py-1.5 pr-4 text-left font-normal text-ink-2"
                  >
                    {formatDay(row.date)}
                  </th>
                  {row.peaks.map((peak, i) => (
                    <td
                      key={windows[i]!.windowId}
                      className="py-1.5 pr-4 text-right"
                    >
                      {peak === null ? (
                        <span className="text-ink-3">–</span>
                      ) : (
                        formatPercent(peak)
                      )}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </details>
  );
}
