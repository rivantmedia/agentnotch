/**
 * The column charts' geometry (lib/bar-chart.ts): clean ticks from zero, thin columns in their
 * bands, a sliver for a small value, rounded data ends, and dates that don't crowd.
 */
import { describe, expect, it } from "vitest";

import {
  BAR_GAP,
  columnAt,
  columnHeight,
  columnIndexAt,
  columnPath,
  labelledColumns,
  MAX_BAR_WIDTH,
  MIN_BAR_HEIGHT,
  niceTicks,
  type BarFrame,
} from "~/lib/bar-chart";

const frame: BarFrame = {
  width: 448,
  height: 144,
  left: 48,
  right: 0,
  top: 10,
};

describe("value axis", () => {
  it("steps in clean values from zero to at least the largest", () => {
    expect(niceTicks(13_389_984)).toEqual([0, 5e6, 10e6, 15e6]);
    expect(niceTicks(31.39)).toEqual([0, 10, 20, 30, 40]);
    expect(niceTicks(0.25)).toEqual([0, 0.1, 0.2, 0.3]);
    expect(niceTicks(10)).toEqual([0, 2.5, 5, 7.5, 10]);
    expect(niceTicks(0)).toEqual([0]);
    expect(niceTicks(Number.NaN)).toEqual([0]);
  });

  it("keeps counts whole", () => {
    expect(niceTicks(3, { integer: true })).toEqual([0, 1, 2, 3]);
    expect(niceTicks(10, { integer: true })).toEqual([0, 5, 10]);
    expect(niceTicks(1, { integer: true })).toEqual([0, 1]);
  });
});

describe("columns", () => {
  it("are at most 24px wide, centred in their band, with a gap", () => {
    // 400px for 8 columns: 50px bands, 24px columns.
    const first = columnAt(frame, 8, 0);
    expect(first).toMatchObject({
      band: 50,
      width: MAX_BAR_WIDTH,
      bandStart: 48,
      center: 73,
    });
    expect(first.x).toBe(48 + 13);
    // 400px for 31 columns: narrower than 24px, and 2px apart.
    const narrow = columnAt(frame, 31, 3);
    expect(narrow.width).toBeCloseTo(400 / 31 - BAR_GAP, 6);
  });

  it("are found under the pointer, clamped to the plot", () => {
    expect(columnIndexAt(frame, 8, 48)).toBe(0);
    expect(columnIndexAt(frame, 8, 97)).toBe(0);
    expect(columnIndexAt(frame, 8, 98)).toBe(1);
    expect(columnIndexAt(frame, 8, 10)).toBe(0);
    expect(columnIndexAt(frame, 8, 9999)).toBe(7);
    expect(columnIndexAt(frame, 0, 100)).toBeNull();
  });

  it("show a small value as a sliver, and nothing as nothing", () => {
    expect(columnHeight(0, 40, 144)).toBe(0);
    expect(columnHeight(0.001, 40, 144)).toBe(MIN_BAR_HEIGHT);
    expect(columnHeight(20, 40, 144)).toBe(72);
    expect(columnHeight(40, 40, 144)).toBe(144);
    expect(columnHeight(5, 0, 144)).toBe(0);
  });

  it("round the data end and keep the foot square", () => {
    expect(columnPath(10, 100, 20, 50)).toBe(
      "M 10 100 V 54 Q 10 50 14 50 H 26 Q 30 50 30 54 V 100 Z",
    );
    // A column shorter than the radius rounds by its height only.
    expect(columnPath(10, 100, 20, 2)).toBe(
      "M 10 100 V 100 Q 10 98 12 98 H 28 Q 30 98 30 100 V 100 Z",
    );
    expect(columnPath(10, 100, 20, 0)).toBe("");
  });
});

describe("dates under the columns", () => {
  it("label every column that fits, else evenly spaced ones and always the last", () => {
    expect(labelledColumns(8, 10)).toEqual([0, 1, 2, 3, 4, 5, 6, 7]);
    expect(labelledColumns(31, 6)).toEqual([0, 6, 12, 18, 24, 30]);
    // One that would crowd the last gives way.
    // One less than a step before the last gives way to it.
    expect(labelledColumns(30, 6)).toEqual([0, 6, 12, 18, 29]);
    expect(labelledColumns(12, 6)).toEqual([0, 3, 6, 11]);
    expect(labelledColumns(5, 1)).toEqual([4]);
    expect(labelledColumns(0, 6)).toEqual([]);
  });
});
