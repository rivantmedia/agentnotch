"use client";

import {
  useEffect,
  useId,
  useRef,
  useState,
  type KeyboardEvent,
  type PointerEvent,
  type ReactNode,
} from "react";

import {
  columnAt,
  columnHeight,
  columnIndexAt,
  columnPath,
  labelledColumns,
  niceTicks,
  type BarFrame,
} from "~/lib/bar-chart";

import { cx } from "./ui";
import { useWidth } from "./use-width";

const PLOT_HEIGHT = 144;
const PAD_TOP = 10;
const AXIS_BAND = 22;
/** Room for the value axis' labels ("$1.25K", "12.5M"), right-aligned against the plot. */
const PAD_LEFT = 48;
const PAD_RIGHT = 4;
/** Room each date under the columns needs. */
const LABEL_WIDTH = 64;
const TOOLTIP_WIDTH = 224;

/**
 * One measure over time as columns from a zero baseline, on one value axis: a single series in
 * the series colour, so the title says what it is and there is no legend. Point at a column (or
 * focus the chart and use the arrow keys) to read it; `active` lights the same column in a
 * sibling chart, and only the chart being pointed at shows its tooltip. The page's table view
 * holds every value, so nothing here depends on hovering.
 */
export function UsageBars({
  title,
  values,
  labels,
  describe,
  tooltip,
  formatTick,
  integer = false,
  active,
  onActive,
  dimmed = false,
}: {
  title: string;
  values: readonly number[];
  /** The column's date, short, for the axis. */
  labels: readonly string[];
  /** The column in words, for screen readers: "$12.30 on Thu, Sep 25". */
  describe: (index: number) => string;
  /** What the tooltip shows for a column, its value first. */
  tooltip: (index: number) => ReactNode;
  formatTick: (value: number) => string;
  /** Whole numbers only on the value axis (counts). */
  integer?: boolean;
  active: number | null;
  onActive: (index: number | null) => void;
  /** A refetch is under way: keep the old columns, faded. */
  dimmed?: boolean;
}) {
  const [boxRef, width] = useWidth<HTMLDivElement>(480);
  // Whether this chart (not a sibling sharing `active`) is the one being pointed at or focused,
  // and whether by a finger, whose tap keeps the column lit until the next tap elsewhere.
  const [pointing, setPointing] = useState(false);
  const [touched, setTouched] = useState(false);
  const rootRef = useRef<HTMLElement>(null);
  const [announce, setAnnounce] = useState(false);
  const titleId = useId();

  const frame: BarFrame = {
    width,
    height: PLOT_HEIGHT,
    left: PAD_LEFT,
    right: PAD_RIGHT,
    top: PAD_TOP,
  };
  const count = values.length;
  const ticks = niceTicks(Math.max(0, ...values), { integer });
  const top = ticks[ticks.length - 1]!;
  const baseY = PAD_TOP + PLOT_HEIGHT;
  const yOf = (value: number) =>
    baseY - (top > 0 ? (value / top) * PLOT_HEIGHT : 0);
  const labelled = new Set(
    labelledColumns(
      count,
      Math.max(2, Math.floor((width - PAD_LEFT - PAD_RIGHT) / LABEL_WIDTH)),
    ),
  );
  const shown = pointing && active !== null && active < count ? active : null;

  function onPointer(event: PointerEvent<SVGSVGElement>) {
    const rect = event.currentTarget.getBoundingClientRect();
    setAnnounce(false);
    setPointing(true);
    setTouched(event.pointerType !== "mouse");
    onActive(columnIndexAt(frame, count, event.clientX - rect.left));
  }

  useEffect(() => {
    if (!touched) return;
    const away = (event: globalThis.PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) {
        setTouched(false);
        setPointing(false);
        onActive(null);
      }
    };
    document.addEventListener("pointerdown", away);
    return () => document.removeEventListener("pointerdown", away);
  }, [touched, onActive]);

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (count === 0) return;
    const last = count - 1;
    const current = active ?? last;
    let next: number | null = null;
    if (event.key === "ArrowLeft") next = Math.max(0, current - 1);
    else if (event.key === "ArrowRight") next = Math.min(last, current + 1);
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = last;
    else if (event.key === "Escape") {
      onActive(null);
      return;
    }
    if (next === null) return;
    event.preventDefault();
    setPointing(true);
    setAnnounce(true);
    onActive(next);
  }

  const tip =
    shown !== null
      ? Math.min(
          Math.max(0, columnAt(frame, count, shown).center - TOOLTIP_WIDTH / 2),
          Math.max(0, width - TOOLTIP_WIDTH),
        )
      : 0;

  return (
    <figure ref={rootRef} className="flex flex-col gap-2">
      <figcaption id={titleId} className="text-sm font-medium">
        {title}
      </figcaption>
      <div
        ref={boxRef}
        role="group"
        aria-labelledby={titleId}
        aria-describedby={`${titleId}-summary`}
        tabIndex={count > 0 ? 0 : -1}
        onKeyDown={onKeyDown}
        onFocus={() => {
          setPointing(true);
          if (active === null && count > 0) {
            setAnnounce(true);
            onActive(count - 1);
          }
        }}
        onBlur={() => {
          setPointing(false);
          onActive(null);
        }}
        className="relative rounded-md"
      >
        <span id={`${titleId}-summary`} className="sr-only">
          {`${count} columns. Use the left and right arrow keys to read them.`}
        </span>
        <span className="sr-only" aria-live="polite">
          {announce && shown !== null ? describe(shown) : ""}
        </span>

        <svg
          width={width}
          height={PAD_TOP + PLOT_HEIGHT + AXIS_BAND}
          viewBox={`0 0 ${width} ${PAD_TOP + PLOT_HEIGHT + AXIS_BAND}`}
          aria-hidden="true"
          className={cx(
            "block max-w-full touch-pan-y transition-opacity",
            dimmed && "opacity-60",
          )}
          onPointerMove={onPointer}
          onPointerDown={onPointer}
          onPointerLeave={(event) => {
            if (event.pointerType !== "mouse") return;
            setPointing(false);
            onActive(null);
          }}
        >
          {active !== null && active < count ? (
            // The lit column's band, so an empty day shows where the pointer is too.
            <rect
              x={columnAt(frame, count, active).bandStart}
              y={PAD_TOP}
              width={columnAt(frame, count, active).band}
              height={PLOT_HEIGHT}
              fill="var(--color-ink)"
              opacity={0.04}
            />
          ) : null}

          {/* Recessive frame: hairline gridlines at the ticks, the baseline a step darker. */}
          {ticks.map((tick) => (
            <g key={tick}>
              <line
                x1={PAD_LEFT}
                x2={width - PAD_RIGHT}
                y1={yOf(tick)}
                y2={yOf(tick)}
                stroke={tick === 0 ? "var(--color-axis)" : "var(--color-grid)"}
                strokeWidth={1}
              />
              <text
                x={PAD_LEFT - 8}
                y={yOf(tick)}
                dy="0.32em"
                textAnchor="end"
                fontSize={11}
                fill="var(--color-ink-3)"
                className="tabular-nums"
              >
                {formatTick(tick)}
              </text>
            </g>
          ))}

          {values.map((value, i) => {
            const column = columnAt(frame, count, i);
            return (
              <path
                key={i}
                d={columnPath(
                  column.x,
                  baseY,
                  column.width,
                  columnHeight(value, top, PLOT_HEIGHT),
                )}
                fill="var(--color-series)"
                className="transition-opacity"
                opacity={active === null || active === i ? 1 : 0.45}
              />
            );
          })}

          {values.map((_, i) =>
            labelled.has(i) ? (
              <text
                key={i}
                x={
                  i === count - 1 && count > 1
                    ? width - PAD_RIGHT
                    : i === 0 && count > 1
                      ? PAD_LEFT
                      : columnAt(frame, count, i).center
                }
                y={PAD_TOP + PLOT_HEIGHT + AXIS_BAND - 5}
                textAnchor={
                  count > 1 && i === count - 1
                    ? "end"
                    : count > 1 && i === 0
                      ? "start"
                      : "middle"
                }
                fontSize={11}
                fill="var(--color-ink-3)"
              >
                {labels[i]}
              </text>
            ) : null,
          )}
        </svg>

        {shown !== null ? (
          <div
            aria-hidden="true"
            className="pointer-events-none absolute top-0 z-10 rounded-md border border-line bg-surface px-2.5 py-1.5 text-xs shadow-md"
            style={{
              left: tip,
              width: TOOLTIP_WIDTH,
              transform: "translateY(-100%)",
              marginTop: -4,
            }}
          >
            {tooltip(shown)}
          </div>
        ) : null}
      </div>
    </figure>
  );
}
