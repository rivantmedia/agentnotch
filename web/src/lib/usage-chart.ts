/**
 * Geometry for the usage-history sparklines, kept pure so it can be tested without a browser.
 *
 * A usage window's utilization only grows until the window resets, so a line drawn from a
 * reading before a reset to one after it would invent a gradual decline that never happened.
 * The series is therefore split into segments wherever a reset (or a long silence) falls between
 * two readings, and each segment is drawn on its own.
 */

export type SeriesPoint = {
  /** observedAt, epoch ms. */
  t: number;
  /** Utilization, percent. */
  v: number;
  /** resetsAt, epoch ms, when known. */
  resetsAt: number | null;
  source: string;
};

export type RawPoint = {
  observedAt: Date;
  utilization: number;
  resetsAt: Date | null;
  source: string;
};

const HOUR = 60 * 60 * 1000;

/** A drop bigger than this between two readings can only be a reset. */
export const RESET_DROP = 5;

export function toSeries(points: readonly RawPoint[]): SeriesPoint[] {
  return points
    .map((p) => ({
      t: p.observedAt.getTime(),
      v: p.utilization,
      resetsAt: p.resetsAt ? p.resetsAt.getTime() : null,
      source: p.source,
    }))
    .filter((p) => Number.isFinite(p.t) && Number.isFinite(p.v))
    .sort((a, b) => a.t - b.t);
}

/** How long two readings may be apart and still be joined by a line. */
export function maxGapMs(windowId: string): number {
  return windowId === "session" ? 5 * HOUR : 24 * HOUR;
}

/** Splits a time-ordered series wherever a reset or a long silence falls between readings. */
export function splitSegments(
  series: readonly SeriesPoint[],
  maxGap: number,
): SeriesPoint[][] {
  const segments: SeriesPoint[][] = [];
  let current: SeriesPoint[] = [];
  for (const point of series) {
    const prev = current.at(-1);
    const breaks =
      prev !== undefined &&
      (point.t - prev.t > maxGap ||
        (prev.resetsAt !== null && point.t >= prev.resetsAt) ||
        point.v < prev.v - RESET_DROP);
    if (breaks) {
      segments.push(current);
      current = [];
    }
    current.push(point);
  }
  if (current.length > 0) segments.push(current);
  return segments;
}

/** The top of the y scale: 100% unless a reading went past it (then the next multiple of 25). */
export function yMax(series: readonly SeriesPoint[]): number {
  const max = series.reduce((m, p) => Math.max(m, p.v), 0);
  return max <= 100 ? 100 : Math.ceil(max / 25) * 25;
}

export type Frame = {
  from: number;
  to: number;
  yMax: number;
  width: number;
  /** Plot height, without the axis band below it. */
  height: number;
  /** Room kept at the left and right so end dots aren't clipped. */
  padX: number;
  /** Room at the top so a 100% line and its dot aren't clipped. */
  padTop: number;
};

export function xOf(frame: Frame, t: number): number {
  const span = Math.max(1, frame.to - frame.from);
  const usable = Math.max(1, frame.width - frame.padX * 2);
  const clamped = Math.min(frame.to, Math.max(frame.from, t));
  return frame.padX + ((clamped - frame.from) / span) * usable;
}

export function yOf(frame: Frame, v: number): number {
  const usable = Math.max(1, frame.height - frame.padTop);
  const clamped = Math.min(frame.yMax, Math.max(0, v));
  return frame.padTop + usable - (clamped / frame.yMax) * usable;
}

const round = (n: number) => Math.round(n * 10) / 10;

/** An SVG path through a segment's points; empty for a single point (drawn as a dot instead). */
export function linePath(
  frame: Frame,
  segment: readonly SeriesPoint[],
): string {
  if (segment.length < 2) return "";
  return segment
    .map(
      (p, i) =>
        `${i === 0 ? "M" : "L"}${round(xOf(frame, p.t))},${round(yOf(frame, p.v))}`,
    )
    .join(" ");
}

/** The same path closed down to the baseline, for the area wash. */
export function areaPath(
  frame: Frame,
  segment: readonly SeriesPoint[],
): string {
  const line = linePath(frame, segment);
  if (!line) return "";
  const first = segment[0]!;
  const last = segment.at(-1)!;
  const base = round(yOf(frame, 0));
  return `${line} L${round(xOf(frame, last.t))},${base} L${round(xOf(frame, first.t))},${base} Z`;
}

/** Index of the point closest in time to `t` (the series is time-ordered); -1 when empty. */
export function nearestIndex(
  series: readonly SeriesPoint[],
  t: number,
): number {
  if (series.length === 0) return -1;
  let lo = 0;
  let hi = series.length - 1;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (series[mid]!.t < t) lo = mid + 1;
    else hi = mid;
  }
  if (lo > 0 && t - series[lo - 1]!.t <= series[lo]!.t - t) return lo - 1;
  return lo;
}

/** The inverse of xOf, for pointer positions. */
export function tAt(frame: Frame, x: number): number {
  const usable = Math.max(1, frame.width - frame.padX * 2);
  const ratio = Math.min(1, Math.max(0, (x - frame.padX) / usable));
  return frame.from + ratio * (frame.to - frame.from);
}

/** Each window's highest reading per day, for the table view of the charts. */
export function dailyPeaks(
  windows: ReadonlyArray<{ windowId: string; series: readonly SeriesPoint[] }>,
  dayKey: (t: number) => string,
  days: readonly string[],
): Array<{ day: string; peaks: Array<number | null> }> {
  const perWindow = windows.map(({ series }) => {
    const peaks = new Map<string, number>();
    for (const point of series) {
      const key = dayKey(point.t);
      peaks.set(key, Math.max(peaks.get(key) ?? -Infinity, point.v));
    }
    return peaks;
  });
  return days.map((day) => ({
    day,
    peaks: perWindow.map((peaks) => peaks.get(day) ?? null),
  }));
}
