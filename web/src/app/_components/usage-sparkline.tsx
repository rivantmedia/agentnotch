"use client";

import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type PointerEvent,
} from "react";

import {
  formatPercent,
  usageSourceLabel,
  windowDescription,
} from "~/lib/format";
import {
  areaPath,
  linePath,
  maxGapMs,
  nearestIndex,
  splitSegments,
  tAt,
  toSeries,
  xOf,
  yMax,
  yOf,
  type Frame,
  type RawPoint,
} from "~/lib/usage-chart";

import { formatDateTime, formatDay, useHydrated } from "./time";

const PLOT_HEIGHT = 96;
const AXIS_BAND = 20;
const PAD_X = 6;
const PAD_TOP = 16;

/** Width of an element, tracked with a ResizeObserver (a fixed guess until it is measured). */
function useWidth<T extends HTMLElement>(fallback: number) {
  const ref = useRef<T>(null);
  const [width, setWidth] = useState(fallback);
  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    const update = () => {
      const next = Math.round(element.getBoundingClientRect().width);
      if (next > 0) setWidth(next);
    };
    update();
    const observer = new ResizeObserver(update);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return [ref, width] as const;
}

/**
 * Utilization of one usage window over a period, as a line on a fixed 0–100% scale. Readings
 * on either side of a reset are not joined (see lib/usage-chart.ts). Hover, or focus and use the
 * arrow keys, to read any reading; the page's table view has the same data without hovering.
 */
export function UsageSparkline({
  windowId,
  points,
  from,
  to,
}: {
  windowId: string;
  points: readonly RawPoint[];
  from: Date;
  to: Date;
}) {
  const hydrated = useHydrated();
  const [boxRef, width] = useWidth<HTMLDivElement>(320);
  const [active, setActive] = useState<number | null>(null);
  const [announce, setAnnounce] = useState(false);
  const titleId = useId();

  const series = useMemo(() => toSeries(points), [points]);
  const segments = useMemo(
    () => splitSegments(series, maxGapMs(windowId)),
    [series, windowId],
  );
  const frame: Frame = {
    from: from.getTime(),
    to: to.getTime(),
    yMax: yMax(series),
    width,
    height: PLOT_HEIGHT,
    padX: PAD_X,
    padTop: PAD_TOP,
  };

  const latest = series.at(-1);
  const peak = series.reduce<number | null>(
    (max, p) => (max === null || p.v > max ? p.v : max),
    null,
  );
  const selected = active !== null ? series[active] : undefined;

  function onPointerMove(event: PointerEvent<SVGSVGElement>) {
    const rect = event.currentTarget.getBoundingClientRect();
    const index = nearestIndex(series, tAt(frame, event.clientX - rect.left));
    setAnnounce(false);
    setActive(index >= 0 ? index : null);
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (series.length === 0) return;
    const last = series.length - 1;
    const current = active ?? last;
    let next: number | null = null;
    if (event.key === "ArrowLeft") next = Math.max(0, current - 1);
    else if (event.key === "ArrowRight") next = Math.min(last, current + 1);
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = last;
    else if (event.key === "Escape") {
      setActive(null);
      return;
    }
    if (next === null) return;
    event.preventDefault();
    setAnnounce(true);
    setActive(next);
  }

  const ticks = hydrated ? tickTimes(from, to) : [];
  const limitY = yOf(frame, 100);
  const baseY = yOf(frame, 0);
  const tipLeft = selected
    ? Math.min(
        Math.max(0, xOf(frame, selected.t) - 88),
        Math.max(0, width - 176),
      )
    : 0;

  const describe = (p: (typeof series)[number]) =>
    `${formatPercent(p.v)} on ${formatDateTime(new Date(p.t))}, from ${usageSourceLabel(p.source)}`;

  return (
    <figure className="flex flex-col gap-2">
      <figcaption className="flex items-baseline justify-between gap-3">
        <span id={titleId} className="text-sm font-medium">
          {windowDescription(windowId)}
        </span>
        <span className="text-xs text-ink-2">
          {latest ? (
            <>
              Latest{" "}
              <span className="font-semibold text-ink">
                {formatPercent(latest.v)}
              </span>
              {peak !== null ? (
                <>
                  {" "}
                  · Peak{" "}
                  <span className="font-semibold text-ink">
                    {formatPercent(peak)}
                  </span>
                </>
              ) : null}
            </>
          ) : (
            "No readings"
          )}
        </span>
      </figcaption>

      <div
        ref={boxRef}
        role="group"
        aria-labelledby={titleId}
        aria-describedby={`${titleId}-summary`}
        tabIndex={series.length > 0 ? 0 : -1}
        onKeyDown={onKeyDown}
        onFocus={() => {
          if (active === null && series.length > 0) {
            setAnnounce(true);
            setActive(series.length - 1);
          }
        }}
        onBlur={() => setActive(null)}
        className="relative rounded-md"
      >
        <span id={`${titleId}-summary`} className="sr-only">
          {series.length === 0
            ? "No readings in this period."
            : `${series.length} readings. Use the left and right arrow keys to read them.`}
        </span>
        <span className="sr-only" aria-live="polite">
          {announce && selected ? describe(selected) : ""}
        </span>

        <svg
          width={width}
          height={PLOT_HEIGHT + AXIS_BAND}
          viewBox={`0 0 ${width} ${PLOT_HEIGHT + AXIS_BAND}`}
          aria-hidden="true"
          className="block max-w-full touch-pan-y"
          onPointerMove={onPointerMove}
          onPointerDown={onPointerMove}
          onPointerLeave={() => setActive(null)}
        >
          {/* Recessive frame: the limit and the baseline, solid hairlines. */}
          <line
            x1={PAD_X}
            x2={width - PAD_X}
            y1={limitY}
            y2={limitY}
            stroke="var(--color-grid)"
            strokeWidth={1}
          />
          <line
            x1={PAD_X}
            x2={width - PAD_X}
            y1={baseY}
            y2={baseY}
            stroke="var(--color-axis)"
            strokeWidth={1}
          />
          <text
            x={width - PAD_X}
            y={limitY - 3}
            textAnchor="end"
            fontSize={10}
            fill="var(--color-ink-3)"
          >
            100%
          </text>

          {segments.map((segment, i) => (
            <path
              key={`a${i}`}
              d={areaPath(frame, segment)}
              fill="var(--color-series-wash)"
            />
          ))}
          {segments.map((segment, i) =>
            segment.length > 1 ? (
              <path
                key={`l${i}`}
                d={linePath(frame, segment)}
                fill="none"
                stroke="var(--color-series)"
                strokeWidth={2}
                strokeLinejoin="round"
                strokeLinecap="round"
              />
            ) : (
              <circle
                key={`d${i}`}
                cx={xOf(frame, segment[0]!.t)}
                cy={yOf(frame, segment[0]!.v)}
                r={2.5}
                fill="var(--color-series)"
              />
            ),
          )}

          {latest && !selected ? (
            <circle
              cx={xOf(frame, latest.t)}
              cy={yOf(frame, latest.v)}
              r={4}
              fill="var(--color-series)"
              stroke="var(--color-surface)"
              strokeWidth={2}
            />
          ) : null}

          {selected ? (
            <g>
              <line
                x1={xOf(frame, selected.t)}
                x2={xOf(frame, selected.t)}
                y1={PAD_TOP / 2}
                y2={baseY}
                stroke="var(--color-ink-3)"
                strokeWidth={1}
              />
              <circle
                cx={xOf(frame, selected.t)}
                cy={yOf(frame, selected.v)}
                r={4.5}
                fill="var(--color-series)"
                stroke="var(--color-surface)"
                strokeWidth={2}
              />
            </g>
          ) : null}

          {ticks.map((tick, i) => (
            <text
              key={tick.t}
              x={xOf(frame, tick.t)}
              y={PLOT_HEIGHT + AXIS_BAND - 5}
              textAnchor={
                i === 0 ? "start" : i === ticks.length - 1 ? "end" : "middle"
              }
              fontSize={11}
              fill="var(--color-ink-3)"
            >
              {tick.label}
            </text>
          ))}
        </svg>

        {selected ? (
          <div
            aria-hidden="true"
            className="pointer-events-none absolute top-0 z-10 w-44 rounded-md border border-line bg-surface px-2.5 py-1.5 text-xs shadow-sm"
            style={{
              left: tipLeft,
              transform: "translateY(-100%)",
              marginTop: -4,
            }}
          >
            <div className="flex items-center gap-1.5">
              <span
                className="inline-block h-0.5 w-3 rounded-full"
                style={{ backgroundColor: "var(--color-series)" }}
              />
              <span className="text-sm font-semibold text-ink">
                {formatPercent(selected.v)}
              </span>
            </div>
            <div className="text-ink-2">
              {formatDateTime(new Date(selected.t))}
            </div>
            <div className="text-ink-3">
              {usageSourceLabel(selected.source)}
            </div>
          </div>
        ) : null}
      </div>
    </figure>
  );
}

/** Four labelled days across the period: its start, two between, and "Today" at the end. */
function tickTimes(from: Date, to: Date): Array<{ t: number; label: string }> {
  const span = to.getTime() - from.getTime();
  if (span <= 0) return [];
  const ticks: Array<{ t: number; label: string }> = [];
  for (let i = 0; i <= 3; i++) {
    const t = from.getTime() + (span * i) / 3;
    const date = new Date(t);
    ticks.push({ t, label: i === 3 ? "Today" : formatDay(date) });
  }
  return ticks;
}
