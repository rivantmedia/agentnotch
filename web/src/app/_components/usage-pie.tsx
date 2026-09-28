"use client";

import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type PointerEvent,
} from "react";

import { formatCost, formatShare, formatTokens, plural } from "~/lib/format";
import { sliceColor, slicePath, type PieSlice } from "~/lib/pie-chart";

import { cx } from "./ui";

export type PieDatum = PieSlice & {
  /** What the slice is, for its tooltip: a project's name, "12 other projects". */
  label: string;
  tokens: bigint;
  costUsd: number | null;
  sessions: number;
};

const RADIUS = 100;
/** Space between the tooltip and the pointer, and between the tooltip and the window's edge. */
const OFFSET = 12;
const MARGIN = 8;

/** Where the tooltip was asked for: a slice, and the pointer in window coordinates. */
type Tip = { key: string; x: number; y: number; touch: boolean };

/**
 * Shares of a whole as a pie: slices clockwise from twelve o'clock, largest first, each its
 * categorical colour (lib/pie-chart.ts), parted by a 2px gap in the surface colour. Pointing at
 * a slice lights it (the others fade) and shows its numbers; a tap does the same until the next
 * tap, on it or anywhere else. `active` lights a slice from outside, e.g. its legend row. The pie
 * is a picture of the legend beside it, which holds every number as text, so it is hidden from
 * screen readers and never the only way to a value.
 */
export function UsagePie({
  slices,
  active,
  onActive,
  dimmed = false,
  className,
}: {
  slices: readonly PieDatum[];
  active: string | null;
  onActive: (key: string | null) => void;
  /** A refetch is under way: keep the old pie, faded. */
  dimmed?: boolean;
  className?: string;
}) {
  const rootRef = useRef<HTMLDivElement>(null);
  const tipRef = useRef<HTMLDivElement>(null);
  const [tip, setTip] = useState<Tip | null>(null);
  // Only the slice pointed at gets the tooltip; one lit from its legend row has the row's text.
  const shown =
    tip?.key === active ? slices.find((s) => s.key === tip.key) : undefined;

  const show = (event: PointerEvent<SVGPathElement>, key: string) => {
    onActive(key);
    setTip({
      key,
      x: event.clientX,
      y: event.clientY,
      touch: event.pointerType !== "mouse",
    });
  };
  const clear = () => {
    onActive(null);
    setTip(null);
  };

  // A tapped slice stays lit until a tap lands anywhere outside the pie.
  const touched = tip?.touch === true;
  useEffect(() => {
    if (!touched) return;
    const away = (event: globalThis.PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) {
        onActive(null);
        setTip(null);
      }
    };
    document.addEventListener("pointerdown", away);
    return () => document.removeEventListener("pointerdown", away);
  }, [touched, onActive]);

  // Beside the pointer (above a finger), flipped and clamped so it stays in the window. Set
  // before paint, so it never shows where it doesn't fit.
  useLayoutEffect(() => {
    const box = tipRef.current;
    if (!box || !tip) return;
    const { width, height } = box.getBoundingClientRect();
    const fits = (at: number, size: number, limit: number) =>
      Math.min(Math.max(MARGIN, at), limit - size - MARGIN);
    let left = tip.x + OFFSET;
    if (left + width > window.innerWidth - MARGIN) {
      left = tip.x - OFFSET - width;
    }
    let top = tip.touch ? tip.y - OFFSET * 2 - height : tip.y + OFFSET;
    if (!tip.touch && top + height > window.innerHeight - MARGIN) {
      top = tip.y - OFFSET - height;
    }
    box.style.left = `${fits(left, width, window.innerWidth)}px`;
    box.style.top = `${fits(top, height, window.innerHeight)}px`;
  }, [tip, shown]);

  return (
    <div ref={rootRef} className={cx("relative aspect-square", className)}>
      <svg
        viewBox={`${-RADIUS} ${-RADIUS} ${RADIUS * 2} ${RADIUS * 2}`}
        className={cx(
          "block size-full overflow-visible transition-opacity",
          dimmed && "opacity-60",
        )}
        aria-hidden="true"
      >
        {slices.length === 0 ? (
          <circle r={RADIUS} fill="var(--color-grid)" />
        ) : (
          // Leaving the slices, not the square around them, puts the pie out of focus.
          <g
            onPointerLeave={(e) => {
              if (e.pointerType === "mouse") clear();
            }}
          >
            {slices.map((slice) => (
              <path
                key={slice.key}
                d={slicePath(slice.start, slice.end, RADIUS)}
                fill={sliceColor(slice.slot)}
                // The gap between slices: the surface showing through, not an outline.
                stroke={slices.length > 1 ? "var(--color-surface)" : "none"}
                strokeWidth={2}
                strokeLinejoin="round"
                vectorEffect="non-scaling-stroke"
                className="transition-opacity"
                opacity={active === null || active === slice.key ? 1 : 0.35}
                onPointerMove={(e) => {
                  if (e.pointerType === "mouse") show(e, slice.key);
                }}
                onPointerDown={(e) => {
                  if (e.pointerType === "mouse") return;
                  // A tap on the lit slice puts it out; on another, lights that one.
                  if (tip?.key === slice.key && active === slice.key) clear();
                  else show(e, slice.key);
                }}
              />
            ))}
          </g>
        )}
      </svg>
      {shown && tip ? (
        <div
          ref={tipRef}
          aria-hidden="true"
          className="pointer-events-none fixed top-0 left-0 z-10 w-max max-w-56 rounded-md border border-line bg-surface px-2.5 py-1.5 text-xs shadow-md"
        >
          <p className="tabular-nums">
            <span className="font-semibold text-ink">
              {formatTokens(shown.tokens)} tokens
            </span>{" "}
            <span className="text-ink-2">· {formatShare(shown.share)}</span>
          </p>
          <p className="flex items-center gap-1.5 break-all text-ink-2">
            <span
              aria-hidden="true"
              className="h-0.5 w-3 shrink-0 rounded-full"
              style={{ backgroundColor: sliceColor(shown.slot) }}
            />
            {shown.label}
          </p>
          <p className="text-ink-3 tabular-nums">
            {formatCost(shown.costUsd) ?? "No cost estimate"} ·{" "}
            {plural(shown.sessions, "session", "sessions")}
          </p>
        </div>
      ) : null}
    </div>
  );
}
