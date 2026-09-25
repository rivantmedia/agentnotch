"use client";

/**
 * Times in the viewer's own time zone. The server doesn't know it, so dates are only written
 * once the page runs in the browser; before that (the server's HTML) a placeholder keeps the
 * layout, and hydration never disagrees with the server.
 */
import { useSyncExternalStore } from "react";

import { formatRelative } from "~/lib/format";

const noop = () => () => undefined;

/** False on the server and while hydrating; true after. */
export function useHydrated(): boolean {
  return useSyncExternalStore(
    noop,
    () => true,
    () => false,
  );
}

// One shared clock for every relative time on the page, ticking every 30 seconds.
let now = 0;
let timer: ReturnType<typeof setInterval> | undefined;
const listeners = new Set<() => void>();

function subscribe(listener: () => void) {
  listeners.add(listener);
  if (timer === undefined) {
    now = Date.now();
    timer = setInterval(() => {
      now = Date.now();
      for (const l of listeners) l();
    }, 30_000);
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0 && timer !== undefined) {
      clearInterval(timer);
      timer = undefined;
    }
  };
}

function snapshot(): number {
  if (now === 0) now = Date.now();
  return now;
}

/** The current time (updated every 30 s) in the browser; null on the server and while hydrating. */
export function useNow(): Date | null {
  const value = useSyncExternalStore<number | null>(
    subscribe,
    snapshot,
    () => null,
  );
  return value === null ? null : new Date(value);
}

const DATE_TIME = new Intl.DateTimeFormat("en-US", {
  month: "short",
  day: "numeric",
  hour: "numeric",
  minute: "2-digit",
});
const DATE_TIME_YEAR = new Intl.DateTimeFormat("en-US", {
  year: "numeric",
  month: "short",
  day: "numeric",
  hour: "numeric",
  minute: "2-digit",
});
const DAY = new Intl.DateTimeFormat("en-US", {
  month: "short",
  day: "numeric",
});
const DAY_YEAR = new Intl.DateTimeFormat("en-US", {
  year: "numeric",
  month: "short",
  day: "numeric",
});
const WEEKDAY_TIME = new Intl.DateTimeFormat("en-US", {
  weekday: "short",
  hour: "numeric",
  minute: "2-digit",
});
const FULL = new Intl.DateTimeFormat("en-US", {
  dateStyle: "full",
  timeStyle: "short",
});

/** "Sep 25, 9:50 AM" (with the year when it isn't this year). Browser only. */
export function formatDateTime(date: Date, now: Date = new Date()): string {
  return date.getFullYear() === now.getFullYear()
    ? DATE_TIME.format(date)
    : DATE_TIME_YEAR.format(date);
}

/** "Sep 25" (with the year when it isn't this year). Browser only. */
export function formatDay(date: Date, now: Date = new Date()): string {
  return date.getFullYear() === now.getFullYear()
    ? DAY.format(date)
    : DAY_YEAR.format(date);
}

/** "Mon 2:00 PM" within the next week, else a date and time. Browser only. */
export function formatSoon(date: Date, now: Date = new Date()): string {
  const ahead = date.getTime() - now.getTime();
  return ahead >= 0 && ahead < 6 * 24 * 60 * 60 * 1000
    ? WEEKDAY_TIME.format(date)
    : formatDateTime(date, now);
}

function formatFull(date: Date): string {
  return FULL.format(date);
}

const PLACEHOLDER = "…";

/** "5 minutes ago", with the exact time on hover. */
export function RelativeTime({
  date,
  className,
}: {
  date: Date;
  className?: string;
}) {
  const now = useNow();
  return (
    <time
      dateTime={date.toISOString()}
      title={now ? formatFull(date) : undefined}
      className={className}
    >
      {now ? formatRelative(date, now) : PLACEHOLDER}
    </time>
  );
}

/** "Sep 25, 9:50 AM" in the viewer's time zone. */
export function DateTime({
  date,
  kind = "datetime",
  className,
}: {
  date: Date;
  kind?: "datetime" | "day";
  className?: string;
}) {
  const now = useNow();
  let text = PLACEHOLDER;
  if (now)
    text = kind === "day" ? formatDay(date, now) : formatDateTime(date, now);
  return (
    <time
      dateTime={date.toISOString()}
      title={now ? formatFull(date) : undefined}
      className={className}
    >
      {text}
    </time>
  );
}
