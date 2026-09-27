/**
 * Pie chart geometry for the usage-by-project charts: pure, so the slices can be tested and the
 * server and the browser draw the same paths.
 *
 * A pie has at most PIE_SLOTS named slices, coloured in the categorical order of globals.css
 * (`--color-cat-1` onwards), and everything past them folds into one gray "other" slice. The
 * colours follow the rank, not the project: around the pie only consecutive slots touch (and the
 * last touches the first, or the gray), and it is those neighbours that were checked to stay
 * apart for colour-blind readers. The legend beside the pie names every slice, so identity never
 * rests on remembering a colour.
 */
import { shareOf } from "./format";

/** Named slices at most; the rest is one "other" slice. */
export const PIE_SLOTS = 6;

export const OTHER_SLICE = "other";

/**
 * The least a slice is drawn, as a fraction of a turn: about 6px of rim on the dashboard's pie,
 * so a small project still shows past the 2px gaps beside it. Its `share` stays true.
 */
export const MIN_SWEEP = 0.01;

export type PieSlice = {
  key: string;
  /** 1…PIE_SLOTS for a named slice, null for the other slice. */
  slot: number | null;
  /** Share of the whole, 0 to 1. */
  share: number;
  /**
   * Where it is drawn, as fractions of a turn clockwise from twelve o'clock: its share, or
   * MIN_SWEEP for a smaller one (taken from the larger slices).
   */
  start: number;
  end: number;
};

/**
 * The slots' colours, spelled out: Tailwind writes a theme variable into the CSS only when it
 * finds its name in the source, so a name pieced together at run time would be missing there.
 */
const SLOT_COLORS = [
  "var(--color-cat-1)",
  "var(--color-cat-2)",
  "var(--color-cat-3)",
  "var(--color-cat-4)",
  "var(--color-cat-5)",
  "var(--color-cat-6)",
] as const;

const OTHER_COLOR = "var(--color-cat-other)";

/** The CSS colour of a slot, or of the other slice. */
export function sliceColor(slot: number | null): string {
  return slot === null ? OTHER_COLOR : (SLOT_COLORS[slot - 1] ?? OTHER_COLOR);
}

/**
 * Slices for `parts` (largest first, as the legend lists them) out of `whole`: the first
 * PIE_SLOTS become named slices; the parts after them and whatever of `whole` no part accounts
 * for (a folded tail) make the other slice. Parts with nothing are left out. Every slice is drawn
 * at least MIN_SWEEP wide; the room comes out of the slices larger than that, in proportion to
 * how much larger they are.
 */
export function pieSlices(
  parts: ReadonlyArray<{ key: string; value: bigint }>,
  whole: bigint,
): PieSlice[] {
  const named = parts.slice(0, PIE_SLOTS).filter((p) => p.value > 0n);
  const namedTotal = named.reduce((sum, p) => sum + p.value, 0n);
  const other = whole - namedTotal;
  const values = [
    ...named.map((p, i) => ({ key: p.key, slot: i + 1, value: p.value })),
    ...(other > 0n ? [{ key: OTHER_SLICE, slot: null, value: other }] : []),
  ];
  const shares = values.map((v) => shareOf(v.value, whole));
  const sweeps = drawnSweeps(shares);
  let at = 0;
  return values.map((v, i) => {
    const start = at;
    // The last slice closes the circle exactly, whatever the rounding of the others.
    const end = i === values.length - 1 ? 1 : Math.min(1, at + sweeps[i]!);
    at = end;
    return { key: v.key, slot: v.slot, share: shares[i]!, start, end };
  });
}

/** Each share raised to MIN_SWEEP, the excess taken back from the shares above it. */
function drawnSweeps(shares: readonly number[]): number[] {
  const raised = shares.map((share) => Math.max(share, MIN_SWEEP));
  const excess = raised.reduce((sum, s) => sum + s, 0) - 1;
  // At most PIE_SLOTS + 1 slices, so the minimums never add up to a whole turn.
  const room = shares.reduce((sum, s) => sum + Math.max(0, s - MIN_SWEEP), 0);
  if (excess <= 0 || room <= 0) return raised;
  return raised.map((sweep, i) =>
    shares[i]! > MIN_SWEEP
      ? sweep - (excess * (shares[i]! - MIN_SWEEP)) / room
      : sweep,
  );
}

/** Which slice a legend row belongs to: its own among the first PIE_SLOTS, else the other. */
export function sliceKeyFor(index: number, key: string): string {
  return index < PIE_SLOTS ? key : OTHER_SLICE;
}

/** A point on the circle of radius `r` around (0, 0), `turn` of the way round from the top. */
function point(turn: number, r: number): [number, number] {
  const angle = turn * 2 * Math.PI;
  return [round(r * Math.sin(angle)), round(-r * Math.cos(angle))];
}

function round(n: number): number {
  const r = Math.round(n * 1000) / 1000;
  return Object.is(r, -0) ? 0 : r;
}

/**
 * The SVG path of a slice, on a circle of radius `r` centred at (0, 0). A whole turn is drawn as
 * two half arcs, since an arc whose ends are the same point draws nothing (SVG's arc notes); so
 * is a turn so nearly whole that its rounded ends meet.
 */
export function slicePath(start: number, end: number, r: number): string {
  const sweep = end - start;
  if (sweep <= 0) return "";
  const [x0, y0] = point(start, r);
  const [x1, y1] = point(end, r);
  if (sweep >= 1 || (x0 === x1 && y0 === y1)) {
    return sweep > 0.5
      ? `M 0 ${-r} A ${r} ${r} 0 1 1 0 ${r} A ${r} ${r} 0 1 1 0 ${-r} Z`
      : "";
  }
  const large = sweep > 0.5 ? 1 : 0;
  return `M 0 0 L ${x0} ${y0} A ${r} ${r} 0 ${large} 1 ${x1} ${y1} Z`;
}
