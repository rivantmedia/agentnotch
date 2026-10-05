/**
 * Calendar buckets in the viewer's time zone, for the usage-over-time charts: the instants where
 * each local day, week (Monday first) or month begins. Pure, and worked out here rather than in
 * SQL, so the database only ever compares instants (the buckets' starts), and the zone rules are
 * ICU's, the same ones the browser writes the labels with.
 */

export type BucketUnit = "day" | "week" | "month";

/** A date on the calendar, in no zone. `month` is 1–12. */
export type CalendarDate = { year: number; month: number; day: number };

const DAY_MS = 24 * 60 * 60 * 1000;

/** What the buckets fall back to when a browser names a zone this server doesn't know. */
export const FALLBACK_TIME_ZONE = "UTC";

/** The most days an all-time chart counts one by one; longer spans count weeks, then months. */
export const MAX_DAILY_BUCKETS = 62;
export const MAX_WEEKLY_BUCKETS = 62;

/**
 * More buckets than any period can need (months since 2023, the earliest date sync accepts, for
 * a long while yet): a guard against a loop that would otherwise never end.
 */
const MAX_BUCKETS = 1000;

/**
 * `timeZone` when ICU knows it ("Asia/Kolkata", "utc"), else null. The name is kept as given
 * rather than ICU's canonical one, which can be an older alias ("Asia/Calcutta").
 */
export function knownTimeZone(timeZone: string): string | null {
  if (timeZone.length === 0) return null;
  try {
    new Intl.DateTimeFormat("en-US", { timeZone });
    return timeZone;
  } catch {
    return null;
  }
}

/**
 * Formatters by zone name. Names come from browsers, and ICU takes any casing of one, so the
 * cache is emptied before it grows past a handful instead of keeping every spelling.
 */
const formatters = new Map<string, Intl.DateTimeFormat>();
const MAX_FORMATTERS = 32;

function formatter(timeZone: string): Intl.DateTimeFormat {
  let f = formatters.get(timeZone);
  if (!f) {
    if (formatters.size >= MAX_FORMATTERS) formatters.clear();
    f = new Intl.DateTimeFormat("en-US", {
      timeZone,
      hourCycle: "h23",
      year: "numeric",
      month: "numeric",
      day: "numeric",
      hour: "numeric",
      minute: "numeric",
      second: "numeric",
    });
    formatters.set(timeZone, f);
  }
  return f;
}

type WallClock = CalendarDate & {
  hour: number;
  minute: number;
  second: number;
};

function wallClock(t: number, timeZone: string): WallClock {
  const parts = formatter(timeZone).formatToParts(new Date(t));
  const part = (type: Intl.DateTimeFormatPartTypes) =>
    Number(parts.find((p) => p.type === type)?.value);
  return {
    year: part("year"),
    month: part("month"),
    day: part("day"),
    hour: part("hour"),
    minute: part("minute"),
    second: part("second"),
  };
}

/** How far the zone's clock is ahead of UTC at `t`, in ms. */
function offsetAt(t: number, timeZone: string): number {
  const w = wallClock(t, timeZone);
  const wall = Date.UTC(w.year, w.month - 1, w.day, w.hour, w.minute, w.second);
  return wall - Math.floor(t / 1000) * 1000;
}

/** The calendar date it is at `t` in the zone. */
export function localDate(t: number, timeZone: string): CalendarDate {
  const { year, month, day } = wallClock(t, timeZone);
  return { year, month, day };
}

function sameDate(a: CalendarDate, b: CalendarDate): boolean {
  return a.year === b.year && a.month === b.month && a.day === b.day;
}

function addDays(date: CalendarDate, days: number): CalendarDate {
  const d = new Date(Date.UTC(date.year, date.month - 1, date.day + days));
  return {
    year: d.getUTCFullYear(),
    month: d.getUTCMonth() + 1,
    day: d.getUTCDate(),
  };
}

/**
 * The first instant of a local day: its midnight, or, where a clock change skips midnight (as
 * Chile's does), the moment the day begins after the gap. The zone's offset is read at a first
 * guess and again where that guess lands; around a change the two can differ, and the day starts
 * at the earlier of the two answers that fall on it.
 */
export function startOfDay(date: CalendarDate, timeZone: string): number {
  const utcMidnight = Date.UTC(date.year, date.month - 1, date.day);
  const first = utcMidnight - offsetAt(utcMidnight, timeZone);
  const second = utcMidnight - offsetAt(first, timeZone);
  const onDay = [first, second]
    .filter((t) => sameDate(localDate(t, timeZone), date))
    .sort((a, b) => a - b);
  return onDay[0] ?? first;
}

/** The first date of the unit `date` falls in: itself, its week's Monday, its month's 1st. */
function unitStartDate(date: CalendarDate, unit: BucketUnit): CalendarDate {
  if (unit === "day") return date;
  if (unit === "month") return { ...date, day: 1 };
  const weekday = new Date(
    Date.UTC(date.year, date.month - 1, date.day),
  ).getUTCDay();
  // getUTCDay counts from Sunday; weeks here start on Monday.
  return addDays(date, -((weekday + 6) % 7));
}

function nextUnitDate(date: CalendarDate, unit: BucketUnit): CalendarDate {
  if (unit === "day") return addDays(date, 1);
  if (unit === "week") return addDays(date, 7);
  // `month` is 1-based, so as a 0-based month it already names the next one.
  const next = new Date(Date.UTC(date.year, date.month, 1));
  return { year: next.getUTCFullYear(), month: next.getUTCMonth() + 1, day: 1 };
}

/** The start of the local day, week or month that `t` falls in. */
export function startOfUnit(
  t: number,
  unit: BucketUnit,
  timeZone: string,
): number {
  return startOfDay(unitStartDate(localDate(t, timeZone), unit), timeZone);
}

/** What an all-time chart counts in: days for a short span, then weeks, then months. */
export function unitForSpan(spanMs: number): BucketUnit {
  if (spanMs <= MAX_DAILY_BUCKETS * DAY_MS) return "day";
  if (spanMs <= MAX_WEEKLY_BUCKETS * 7 * DAY_MS) return "week";
  return "month";
}

/**
 * Where each bucket of [from, to) begins: `from` itself, then every start of a local `unit`
 * after it and before `to`. `from` needn't be a unit's start: the first bucket of a rolling
 * period ("the last 7 days") is the rest of its first day. Never empty.
 */
export function bucketStarts(
  from: number,
  to: number,
  unit: BucketUnit,
  timeZone: string,
): number[] {
  const starts = [from];
  let date = unitStartDate(localDate(from, timeZone), unit);
  while (starts.length < MAX_BUCKETS) {
    date = nextUnitDate(date, unit);
    const start = startOfDay(date, timeZone);
    if (start >= to) break;
    // Always forward, whatever a zone's history does.
    if (start > starts[starts.length - 1]!) starts.push(start);
  }
  return starts;
}
