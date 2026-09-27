import { readFileSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

import {
  MIN_SWEEP,
  OTHER_SLICE,
  PIE_SLOTS,
  pieSlices,
  sliceColor,
  sliceKeyFor,
  slicePath,
} from "~/lib/pie-chart";

const part = (key: string, value: bigint) => ({ key, value });

describe("pie slices", () => {
  it("go round once from twelve o'clock, largest first, without gaps", () => {
    const slices = pieSlices(
      [part("a", 50n), part("b", 30n), part("c", 20n)],
      100n,
    );
    expect(slices.map((s) => [s.key, s.slot, s.start, s.end])).toEqual([
      ["a", 1, 0, 0.5],
      ["b", 2, 0.5, 0.8],
      ["c", 3, 0.8, 1],
    ]);
    expect(slices.map((s) => s.share)).toEqual([0.5, 0.3, 0.2]);
  });

  it("name the first slots and fold the rest, and any tail, into one other slice", () => {
    const parts = Array.from({ length: PIE_SLOTS + 3 }, (_, i) =>
      part(`p${i}`, 10n),
    );
    // A tail the list already added up (the dashboard's rest row) adds to it too.
    const slices = pieSlices(parts, 10n * BigInt(parts.length) + 30n);
    expect(slices).toHaveLength(PIE_SLOTS + 1);
    expect(slices.map((s) => s.slot)).toEqual([1, 2, 3, 4, 5, 6, null]);
    const other = slices.at(-1)!;
    expect(other.key).toBe(OTHER_SLICE);
    expect(other.share).toBeCloseTo(60 / 120, 10);
    expect(other.end).toBe(1);
    for (let i = 1; i < slices.length; i++) {
      expect(slices[i]!.start).toBe(slices[i - 1]!.end);
    }
  });

  it("leave out what has nothing, and draw nothing for nothing", () => {
    expect(
      pieSlices([part("a", 5n), part("b", 0n)], 5n).map((s) => s.key),
    ).toEqual(["a"]);
    expect(pieSlices([part("a", 0n)], 0n)).toEqual([]);
    expect(pieSlices([], 0n)).toEqual([]);
  });

  it("close the circle exactly, whatever the rounding", () => {
    const slices = pieSlices([part("a", 1n), part("b", 1n), part("c", 1n)], 3n);
    expect(slices.at(-1)!.end).toBe(1);
  });

  it("draw a small part wide enough to see, keeping its true share", () => {
    const slices = pieSlices([part("big", 9_995n), part("small", 5n)], 10_000n);
    const [big, small] = slices;
    expect(small!.share).toBe(0.0005);
    expect(small!.end - small!.start).toBeCloseTo(MIN_SWEEP, 10);
    expect(big!.share).toBe(0.9995);
    expect(big!.end - big!.start).toBeCloseTo(1 - MIN_SWEEP, 10);
    expect(small!.end).toBe(1);
    // Several small ones take their room from the large ones only, in proportion.
    const many = pieSlices(
      [part("a", 600n), part("b", 390n), part("c", 5n), part("d", 5n)],
      1_000n,
    );
    const sweeps = many.map((s) => s.end - s.start);
    expect(sweeps[2]).toBeCloseTo(MIN_SWEEP, 10);
    expect(sweeps[3]).toBeCloseTo(MIN_SWEEP, 10);
    expect(sweeps.reduce((a, b) => a + b, 0)).toBeCloseTo(1, 10);
    expect(sweeps[0]! / sweeps[1]!).toBeGreaterThan(1.5);
  });

  it("never draw a blank pie when one part holds nearly everything", () => {
    const slices = pieSlices(
      [part("a", 3_000_000_000n), part("b", 2_000n)],
      3_000_002_000n,
    );
    for (const slice of slices) {
      const path = slicePath(slice.start, slice.end, 100);
      expect(path).not.toBe("");
      // An arc between the same two points draws nothing.
      expect(path).not.toMatch(/L 0 -100 A 100 100 0 [01] 1 0 -100 Z/);
    }
  });

  it("belong to a legend row by position", () => {
    expect(sliceKeyFor(0, "a")).toBe("a");
    expect(sliceKeyFor(PIE_SLOTS - 1, "f")).toBe("f");
    expect(sliceKeyFor(PIE_SLOTS, "g")).toBe(OTHER_SLICE);
  });

  it("take the categorical slots of the site's tokens, and gray for the rest", () => {
    expect(sliceColor(1)).toBe("var(--color-cat-1)");
    expect(sliceColor(null)).toBe("var(--color-cat-other)");
    const css = readFileSync(
      path.resolve(import.meta.dirname, "../../src/styles/globals.css"),
      "utf8",
    );
    for (let slot = 1; slot <= PIE_SLOTS; slot++) {
      // Light and dark.
      expect(css.match(new RegExp(`--color-cat-${slot}: #`, "g"))).toHaveLength(
        2,
      );
    }
    expect(css).toContain("--color-cat-other: #");
    // Each is spelled out in the source, or Tailwind leaves it out of the CSS it writes.
    const source = readFileSync(
      path.resolve(import.meta.dirname, "../../src/lib/pie-chart.ts"),
      "utf8",
    );
    for (let slot = 1; slot <= PIE_SLOTS; slot++) {
      expect(sliceColor(slot)).toBe(`var(--color-cat-${slot})`);
      expect(source).toContain(`"var(--color-cat-${slot})"`);
    }
    expect(sliceColor(PIE_SLOTS + 1)).toBe("var(--color-cat-other)");
  });
});

describe("slice paths", () => {
  it("draw a wedge from the centre", () => {
    expect(slicePath(0, 0.25, 100)).toBe(
      "M 0 0 L 0 -100 A 100 100 0 0 1 100 0 Z",
    );
    expect(slicePath(0.25, 0.5, 100)).toBe(
      "M 0 0 L 100 0 A 100 100 0 0 1 0 100 Z",
    );
  });

  it("take the long way round past half a turn", () => {
    expect(slicePath(0, 0.75, 100)).toBe(
      "M 0 0 L 0 -100 A 100 100 0 1 1 -100 0 Z",
    );
  });

  it("draw a whole turn as two half arcs, and nothing as nothing", () => {
    expect(slicePath(0, 1, 100)).toBe(
      "M 0 -100 A 100 100 0 1 1 0 100 A 100 100 0 1 1 0 -100 Z",
    );
    expect(slicePath(0.3, 0.3, 100)).toBe("");
    // So nearly whole that its rounded ends meet: still the whole circle.
    expect(slicePath(0.0000001, 1, 100)).toBe(slicePath(0, 1, 100));
  });
});
