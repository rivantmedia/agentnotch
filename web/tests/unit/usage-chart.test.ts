import { describe, expect, it } from "vitest";

import {
  areaPath,
  dailyPeaks,
  linePath,
  maxGapMs,
  nearestIndex,
  RESET_DROP,
  splitSegments,
  tAt,
  toSeries,
  xOf,
  yMax,
  yOf,
  type Frame,
  type SeriesPoint,
} from "~/lib/usage-chart";

const HOUR = 60 * 60 * 1000;
const T0 = Date.parse("2026-09-01T00:00:00Z");

function point(
  hours: number,
  v: number,
  resetsInHours: number | null = null,
): SeriesPoint {
  return {
    t: T0 + hours * HOUR,
    v,
    resetsAt: resetsInHours === null ? null : T0 + resetsInHours * HOUR,
    source: "probe",
  };
}

describe("toSeries", () => {
  it("sorts by time and drops unusable readings", () => {
    const series = toSeries([
      {
        observedAt: new Date(T0 + 2 * HOUR),
        utilization: 20,
        resetsAt: null,
        source: "desktop",
      },
      {
        observedAt: new Date(T0),
        utilization: 10,
        resetsAt: new Date(T0 + 5 * HOUR),
        source: "probe",
      },
      {
        observedAt: new Date(Number.NaN),
        utilization: 50,
        resetsAt: null,
        source: "probe",
      },
      {
        observedAt: new Date(T0 + HOUR),
        utilization: Number.NaN,
        resetsAt: null,
        source: "probe",
      },
    ]);
    expect(series).toEqual([
      { t: T0, v: 10, resetsAt: T0 + 5 * HOUR, source: "probe" },
      { t: T0 + 2 * HOUR, v: 20, resetsAt: null, source: "desktop" },
    ]);
  });
});

describe("splitSegments", () => {
  it("keeps a rising window in one segment", () => {
    const series = [point(0, 5, 5), point(1, 20, 5), point(2, 40, 5)];
    expect(splitSegments(series, maxGapMs("session"))).toEqual([series]);
  });

  it("breaks where the window reset between two readings", () => {
    const series = [point(0, 30, 3), point(2, 60, 3), point(4, 62, 8)];
    // 62 after 60 looks like growth, but the window reset at hour 3 in between.
    expect(splitSegments(series, maxGapMs("session"))).toEqual([
      [series[0], series[1]],
      [series[2]],
    ]);
  });

  it("breaks on a drop that can only be a reset, but not on reporting jitter", () => {
    const series = [
      point(0, 50),
      point(1, 50 - RESET_DROP + 1),
      point(2, 10),
      point(3, 12),
    ];
    expect(splitSegments(series, maxGapMs("weekly_all"))).toEqual([
      [series[0], series[1]],
      [series[2], series[3]],
    ]);
  });

  it("breaks on a long silence, shorter for the 5-hour window", () => {
    const series = [point(0, 10), point(6, 12), point(40, 15)];
    expect(splitSegments(series, maxGapMs("session"))).toHaveLength(3);
    expect(splitSegments(series, maxGapMs("weekly_all"))).toEqual([
      [series[0], series[1]],
      [series[2]],
    ]);
  });

  it("returns nothing for nothing", () => {
    expect(splitSegments([], HOUR)).toEqual([]);
  });
});

describe("scales", () => {
  const frame: Frame = {
    from: T0,
    to: T0 + 10 * HOUR,
    yMax: 100,
    width: 220,
    height: 110,
    padX: 10,
    padTop: 10,
  };

  it("fix the top at 100% unless a reading went past it", () => {
    expect(yMax([point(0, 12), point(1, 99)])).toBe(100);
    expect(yMax([point(0, 101)])).toBe(125);
    expect(yMax([point(0, 150)])).toBe(150);
    expect(yMax([])).toBe(100);
  });

  it("map time and value into the plot, clamped to its edges", () => {
    expect(xOf(frame, T0)).toBe(10);
    expect(xOf(frame, T0 + 5 * HOUR)).toBe(110);
    expect(xOf(frame, T0 + 10 * HOUR)).toBe(210);
    expect(xOf(frame, T0 - HOUR)).toBe(10);
    expect(xOf(frame, T0 + 20 * HOUR)).toBe(210);
    expect(yOf(frame, 0)).toBe(110);
    expect(yOf(frame, 100)).toBe(10);
    expect(yOf(frame, 50)).toBe(60);
    expect(yOf(frame, 140)).toBe(10);
    expect(yOf(frame, -5)).toBe(110);
  });

  it("turn a pointer position back into a time", () => {
    expect(tAt(frame, 110)).toBe(T0 + 5 * HOUR);
    expect(tAt(frame, 0)).toBe(T0);
    expect(tAt(frame, 500)).toBe(T0 + 10 * HOUR);
  });

  it("draw a segment as a line and a closed area; a lone reading as neither", () => {
    const segment = [point(0, 0), point(5, 50), point(10, 100)];
    expect(linePath(frame, segment)).toBe("M10,110 L110,60 L210,10");
    expect(areaPath(frame, segment)).toBe(
      "M10,110 L110,60 L210,10 L210,110 L10,110 Z",
    );
    expect(linePath(frame, [point(1, 5)])).toBe("");
    expect(areaPath(frame, [point(1, 5)])).toBe("");
  });
});

describe("nearestIndex", () => {
  const series = [point(0, 1), point(2, 2), point(6, 3)];

  it("finds the reading closest in time", () => {
    expect(nearestIndex(series, T0 - HOUR)).toBe(0);
    expect(nearestIndex(series, T0 + 0.9 * HOUR)).toBe(0);
    expect(nearestIndex(series, T0 + 1.5 * HOUR)).toBe(1);
    expect(nearestIndex(series, T0 + 3.9 * HOUR)).toBe(1);
    expect(nearestIndex(series, T0 + 4.1 * HOUR)).toBe(2);
    expect(nearestIndex(series, T0 + 99 * HOUR)).toBe(2);
    expect(nearestIndex([], T0)).toBe(-1);
  });
});

describe("dailyPeaks", () => {
  it("gives each window's highest reading per day, and null for silent days", () => {
    const dayKey = (t: number) => new Date(t).toISOString().slice(0, 10);
    const rows = dailyPeaks(
      [
        {
          windowId: "session",
          series: [point(1, 20), point(3, 70), point(26, 15)],
        },
        { windowId: "weekly_all", series: [point(2, 40)] },
      ],
      dayKey,
      ["2026-09-03", "2026-09-02", "2026-09-01"],
    );
    expect(rows).toEqual([
      { day: "2026-09-03", peaks: [null, null] },
      { day: "2026-09-02", peaks: [15, null] },
      { day: "2026-09-01", peaks: [70, 40] },
    ]);
  });
});
