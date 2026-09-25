"use client";

import {
  formatDuration,
  formatPercent,
  meterLevel,
  meterLevelText,
  usageSourceLabel,
  windowDescription,
  windowLabel,
} from "~/lib/format";

import { formatSoon, RelativeTime, useNow } from "./time";
import { cx } from "./ui";

export type MeterReading = {
  windowId: string;
  utilization: number;
  resetsAt: Date | null;
  observedAt: Date;
  source: string;
};

const FILL: Record<ReturnType<typeof meterLevel>, string> = {
  normal: "var(--color-series)",
  high: "var(--color-warning)",
  critical: "var(--color-critical)",
};

/**
 * One usage window as a meter: the percentage in text first, then the bar, then when it resets
 * and how old the reading is. A window that has reset since the reading says so instead of
 * pretending the old number still holds.
 */
export function UsageMeter({
  reading,
  compact = false,
}: {
  reading: MeterReading;
  compact?: boolean;
}) {
  const now = useNow();
  const level = meterLevel(reading.utilization);
  const levelText = meterLevelText(reading.utilization);
  const hasReset =
    now !== null &&
    reading.resetsAt !== null &&
    reading.resetsAt.getTime() <= now.getTime();
  const width = Math.min(100, Math.max(0, reading.utilization));
  const fill = FILL[level];

  let resetText: string | null = null;
  if (now && reading.resetsAt && !hasReset) {
    const ahead = reading.resetsAt.getTime() - now.getTime();
    resetText =
      ahead < 24 * 60 * 60 * 1000
        ? `Resets in ${formatDuration(ahead)}`
        : `Resets ${formatSoon(reading.resetsAt, now)}`;
  }

  return (
    <div className={cx("flex flex-col", compact ? "gap-1" : "gap-1.5")}>
      <div className="flex items-baseline justify-between gap-3 text-sm">
        <span
          className="font-medium text-ink"
          title={windowDescription(reading.windowId)}
        >
          {windowLabel(reading.windowId)}
        </span>
        <span className="flex items-baseline gap-2">
          {levelText && !hasReset ? (
            <span
              className={cx(
                "inline-flex items-center gap-1 text-xs font-medium",
                level === "critical" ? "text-critical-ink" : "text-warning-ink",
              )}
            >
              <svg
                viewBox="0 0 12 12"
                aria-hidden="true"
                className="size-3"
                fill="currentColor"
              >
                <path d="M6 .75c.3 0 .57.16.72.42l4.6 8.1c.3.53-.08 1.18-.7 1.18H1.38c-.62 0-1-.65-.7-1.18l4.6-8.1A.83.83 0 0 1 6 .75Zm0 3a.6.6 0 0 0-.6.6v2.3a.6.6 0 0 0 1.2 0v-2.3a.6.6 0 0 0-.6-.6Zm0 5.4a.7.7 0 1 0 0-1.4.7.7 0 0 0 0 1.4Z" />
              </svg>
              {levelText}
            </span>
          ) : null}
          <span className="font-semibold tabular-nums">
            {formatPercent(reading.utilization)}
          </span>
        </span>
      </div>
      <div
        role="meter"
        aria-label={`${windowDescription(reading.windowId)} used`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(
          Math.min(100, Math.max(0, reading.utilization)),
        )}
        aria-valuetext={`${formatPercent(reading.utilization)} used${resetText ? `, ${resetText.toLowerCase()}` : ""}${hasReset ? ", the window has reset since this reading" : ""}`}
        className="h-2 w-full overflow-hidden rounded-full"
        style={{
          backgroundColor: `color-mix(in oklab, ${fill} 16%, var(--color-surface))`,
          opacity: hasReset ? 0.5 : 1,
        }}
      >
        <div
          className="h-full rounded-full"
          style={{ width: `${width}%`, backgroundColor: fill }}
        />
      </div>
      <p className="flex flex-wrap gap-x-2 text-xs text-ink-3">
        {hasReset ? (
          <span>Reset since this reading</span>
        ) : resetText ? (
          <span>{resetText}</span>
        ) : null}
        {!compact || hasReset ? (
          <span>
            {usageSourceLabel(reading.source)},{" "}
            <RelativeTime date={reading.observedAt} />
          </span>
        ) : null}
      </p>
    </div>
  );
}
