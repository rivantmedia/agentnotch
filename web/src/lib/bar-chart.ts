/**
 * Geometry for the usage-over-time column charts, kept pure so it can be tested without a
 * browser: a value axis from zero with clean ticks, columns in equal bands, and which of them
 * get a date under them.
 *
 * Columns are at most MAX_BAR_WIDTH wide and leave the rest of their band as air, with at least
 * BAR_GAP between neighbours; their data end is rounded (BAR_RADIUS) and their foot square on
 * the baseline. A value that is there but would round to nothing still shows MIN_BAR_HEIGHT,
 * so a quiet day never reads as an empty one; the axis, the tooltip and the table carry the
 * exact numbers.
 */

export const MAX_BAR_WIDTH = 24;
export const BAR_GAP = 2;
export const BAR_RADIUS = 4;
export const MIN_BAR_HEIGHT = 2;

/** The plot inside the chart: margins for the axis labels around it. */
export type BarFrame = {
  width: number;
  /** The plot's height, from its top to the baseline. */
  height: number;
  left: number;
  right: number;
  top: number;
};

/** Numbers like 0.1 + 0.2 kept clean for labels. */
function tidy(value: number): number {
  return Number(value.toPrecision(12));
}

/**
 * Ticks from 0 to the first clean value at or above `max`: about `count` steps of 1, 2, 2.5 or
 * 5 times a power of ten. For counts (`integer`), never a step below 1 or a 2.5 one.
 */
export function niceTicks(
  max: number,
  { count = 4, integer = false }: { count?: number; integer?: boolean } = {},
): number[] {
  if (!Number.isFinite(max) || max <= 0) return [0];
  const rough = max / count;
  const power = 10 ** Math.floor(Math.log10(rough));
  const multiples = integer && power < 10 ? [1, 2, 5, 10] : [1, 2, 2.5, 5, 10];
  let step = multiples.map((m) => m * power).find((s) => s >= rough)!;
  if (integer) step = Math.max(1, Math.round(step));
  const steps = Math.ceil(tidy(max / step));
  return Array.from({ length: steps + 1 }, (_, i) => tidy(i * step));
}

/** Where column `index` of `count` sits: its band, and the column centred in it. */
export function columnAt(frame: BarFrame, count: number, index: number) {
  const plot = Math.max(0, frame.width - frame.left - frame.right);
  const band = count > 0 ? plot / count : 0;
  const width = Math.max(1, Math.min(MAX_BAR_WIDTH, band - BAR_GAP));
  const bandStart = frame.left + band * index;
  return {
    bandStart,
    band,
    x: bandStart + (band - width) / 2,
    width,
    center: bandStart + band / 2,
  };
}

/** The column under a point `x` pixels from the chart's left edge; null outside the plot. */
export function columnIndexAt(
  frame: BarFrame,
  count: number,
  x: number,
): number | null {
  const plot = frame.width - frame.left - frame.right;
  if (count === 0 || plot <= 0) return null;
  const index = Math.floor(((x - frame.left) / plot) * count);
  return Math.min(count - 1, Math.max(0, index));
}

/** A column's height for `value` on an axis up to `top`. */
export function columnHeight(
  value: number,
  top: number,
  plotHeight: number,
): number {
  if (!(value > 0) || !(top > 0)) return 0;
  return Math.min(
    plotHeight,
    Math.max(MIN_BAR_HEIGHT, (value / top) * plotHeight),
  );
}

/** The column as a path: rounded at its data end, square at the baseline `baseY`. */
export function columnPath(
  x: number,
  baseY: number,
  width: number,
  height: number,
): string {
  if (height <= 0 || width <= 0) return "";
  const r = Math.min(BAR_RADIUS, width / 2, height);
  const top = baseY - height;
  const n = (v: number) => Math.round(v * 100) / 100;
  return [
    `M ${n(x)} ${n(baseY)}`,
    `V ${n(top + r)}`,
    `Q ${n(x)} ${n(top)} ${n(x + r)} ${n(top)}`,
    `H ${n(x + width - r)}`,
    `Q ${n(x + width)} ${n(top)} ${n(x + width)} ${n(top + r)}`,
    `V ${n(baseY)}`,
    "Z",
  ].join(" ");
}

/**
 * Which of `count` columns get a label under them when at most `max` fit: evenly spaced from
 * the first, and always the last (now). The last label sits flush with the plot's right edge,
 * left of its column's centre, so a label less than a full step before it gives way.
 */
export function labelledColumns(count: number, max: number): number[] {
  if (count <= 0) return [];
  if (count <= max) return Array.from({ length: count }, (_, i) => i);
  if (max <= 1) return [count - 1];
  const step = Math.ceil((count - 1) / (max - 1));
  const picked: number[] = [];
  for (let i = 0; i < count - 1; i += step) picked.push(i);
  if (picked.length > 1 && count - 1 - picked[picked.length - 1]! < step) {
    picked.pop();
  }
  picked.push(count - 1);
  return picked;
}
