/**
 * How numbers, times and ids read on the website. Pure and locale-fixed ("en-US" digits), so the
 * server and the browser print the same string for the same value; only calendar dates depend on
 * the viewer's time zone, and those are rendered in the browser (see app/_components/time.tsx).
 */

const WHOLE = new Intl.NumberFormat("en-US");
const COMPACT = new Intl.NumberFormat("en-US", {
  notation: "compact",
  maximumFractionDigits: 1,
});
const USD = new Intl.NumberFormat("en-US", {
  style: "currency",
  currency: "USD",
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
});

/** 999 · 12.9K · 13.4M: token counts at a glance. */
export function formatTokens(value: bigint | number): string {
  const small = typeof value === "bigint" ? value < 1000n : value < 1000;
  return small ? WHOLE.format(value) : COMPACT.format(value);
}

/** 12,873,120: the exact count, for titles and tables. */
export function formatExact(value: bigint | number): string {
  return WHOLE.format(value);
}

/** "$14.82"; costs under a cent read "<$0.01"; unknown is null. */
export function formatCost(usd: number | null | undefined): string | null {
  if (usd == null || !Number.isFinite(usd)) return null;
  if (usd > 0 && usd < 0.01) return "<$0.01";
  return USD.format(usd);
}

/**
 * "42%". Below 100 the value is rounded down, so a reading never looks closer to (or at) the
 * limit than it is; 99.6 reads "99%", not "100%".
 */
export function formatPercent(utilization: number): string {
  if (!Number.isFinite(utilization)) return "–";
  const shown =
    utilization < 100
      ? Math.max(0, Math.floor(utilization))
      : Math.round(utilization);
  return `${shown}%`;
}

/** A part's share of a whole, from 0 to 1; 0 when the whole is nothing. */
export function shareOf(part: bigint, whole: bigint): number {
  if (whole <= 0n || part <= 0n) return 0;
  if (part >= whole) return 1;
  // As Numbers: past the safe range they round, but keep ~16 significant digits, plenty for a
  // share. And a part that is there never divides down to 0 (fixed-point bigint division would
  // truncate a small project to none), so it reads "<1%" with a sliver.
  return Number(part) / Number(whole);
}

/**
 * "42%" of a whole. A share that is there but rounds to nothing reads "<1%", and only the whole
 * reads "100%", so a breakdown never shows a part as all or none of it when it isn't.
 */
export function formatShare(share: number): string {
  if (!Number.isFinite(share) || share <= 0) return "0%";
  if (share >= 1) return "100%";
  if (share < 0.01) return "<1%";
  return `${Math.min(99, Math.round(share * 100))}%`;
}

const SECOND = 1000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** "45 s", "12 min", "1 h 45 min", "2 d 3 h". */
export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "–";
  if (ms < MINUTE) return `${Math.floor(ms / SECOND)} s`;
  if (ms < HOUR) return `${Math.floor(ms / MINUTE)} min`;
  if (ms < DAY) {
    const hours = Math.floor(ms / HOUR);
    const minutes = Math.floor((ms % HOUR) / MINUTE);
    return minutes > 0 ? `${hours} h ${minutes} min` : `${hours} h`;
  }
  const days = Math.floor(ms / DAY);
  const hours = Math.floor((ms % DAY) / HOUR);
  return hours > 0 ? `${days} d ${hours} h` : `${days} d`;
}

const RELATIVE = new Intl.RelativeTimeFormat("en-US", { numeric: "auto" });

/** "just now", "5 minutes ago", "in 3 hours", "yesterday", "2 weeks ago". */
export function formatRelative(date: Date, now: Date): string {
  const diff = date.getTime() - now.getTime();
  const abs = Math.abs(diff);
  if (abs < 45 * SECOND) return "just now";
  const sign = diff < 0 ? -1 : 1;
  if (abs < HOUR)
    return RELATIVE.format(
      sign * Math.max(1, Math.round(abs / MINUTE)),
      "minute",
    );
  if (abs < DAY) return RELATIVE.format(sign * Math.round(abs / HOUR), "hour");
  if (abs < 7 * DAY)
    return RELATIVE.format(sign * Math.round(abs / DAY), "day");
  if (abs < 30 * DAY)
    return RELATIVE.format(sign * Math.round(abs / (7 * DAY)), "week");
  if (abs < 365 * DAY)
    return RELATIVE.format(sign * Math.round(abs / (30 * DAY)), "month");
  return RELATIVE.format(sign * Math.round(abs / (365 * DAY)), "year");
}

/** The usage windows every Claude account has, in the order meters and charts start with. */
export const FIXED_WINDOW_IDS: readonly string[] = [
  "session",
  "weekly_all",
  "extra_usage",
];

export function isFixedWindow(windowId: string): boolean {
  return FIXED_WINDOW_IDS.includes(windowId);
}

/** Orders usage window ids: the fixed windows first, in their order, then the rest by id. */
export function compareWindowIds(a: string, b: string): number {
  const rank = (id: string) => {
    const index = FIXED_WINDOW_IDS.indexOf(id);
    return index === -1 ? FIXED_WINDOW_IDS.length : index;
  };
  return rank(a) - rank(b) || (a < b ? -1 : a > b ? 1 : 0);
}

/** What a usage window is called: `session` is Claude's 5-hour window. */
export function windowLabel(windowId: string): string {
  if (windowId === "session") return "5-hour";
  if (windowId === "weekly_all") return "Weekly";
  if (windowId === "extra_usage") return "Extra usage";
  if (windowId.startsWith("weekly_")) {
    const model = windowId.slice("weekly_".length).replace(/[_-]+/g, " ");
    return model.trim() ? `Weekly · ${capitalize(model.trim())}` : "Weekly";
  }
  return capitalize(windowId.replace(/[_-]+/g, " ").trim()) || windowId;
}

/** A longer description, for chart titles and screen readers. */
export function windowDescription(windowId: string): string {
  if (windowId === "session") return "5-hour session limit";
  if (windowId === "weekly_all") return "Weekly limit, all models";
  if (windowId === "extra_usage") return "Extra usage";
  if (windowId.startsWith("weekly_")) return `${windowLabel(windowId)} limit`;
  return windowLabel(windowId);
}

/** Where a session ran. */
export function sessionSourceLabel(source: string): string {
  switch (source) {
    case "cli":
      return "CLI";
    case "vscode":
      return "VS Code";
    case "desktop":
      return "Claude Desktop";
    case "sdk":
      return "Agent SDK";
    default:
      return "Other";
  }
}

/** Who reported a usage reading. */
export function usageSourceLabel(source: string): string {
  switch (source) {
    case "probe":
      return "Claude Code";
    case "statusLine":
      return "Claude Code status line";
    case "claudeJson":
      return "Claude Code's cache";
    case "desktop":
      return "Claude Desktop";
    default:
      return source;
  }
}

/**
 * "claude-opus-4-5-20251101" → "Opus 4.5", "claude-3-5-sonnet-20241022" → "Sonnet 3.5".
 * Anything else is shown as sent, minus a "claude-" prefix and a date suffix.
 */
export function modelLabel(id: string): string {
  const bare = id
    .trim()
    .replace(/^claude-/i, "")
    .replace(/-\d{8}$/, "")
    .replace(/\[1m\]$/i, "");
  const family = "(opus|sonnet|haiku)";
  const newer = new RegExp(`^${family}-(\\d+)(?:-(\\d+))?$`, "i").exec(bare);
  if (newer) {
    const [, name, major, minor] = newer;
    return `${capitalize(name!)} ${major}${minor ? `.${minor}` : ""}`;
  }
  const older = new RegExp(`^(\\d+)(?:-(\\d+))?-${family}$`, "i").exec(bare);
  if (older) {
    const [, major, minor, name] = older;
    return `${capitalize(name!)} ${major}${minor ? `.${minor}` : ""}`;
  }
  return bare || id;
}

/** The name an account goes by: the viewer's label, else its email, else its organization. */
export function accountTitle(account: {
  label?: string | null;
  email?: string | null;
  organizationName?: string | null;
}): string {
  return (
    nonEmpty(account.label) ??
    nonEmpty(account.email) ??
    nonEmpty(account.organizationName) ??
    "Claude account"
  );
}

/** The line under an account's title: whatever the title didn't already say. */
export function accountSubtitle(account: {
  label?: string | null;
  email?: string | null;
  organizationName?: string | null;
}): string | null {
  const title = accountTitle(account);
  const parts = [nonEmpty(account.email), nonEmpty(account.organizationName)]
    .filter((part): part is string => part !== null)
    .filter((part) => part !== title);
  return parts.length > 0 ? parts.join(" · ") : null;
}

export type MeterLevel = "normal" | "high" | "critical";

/** How close to a limit a reading is: high from 75%, critical from 90%. */
export function meterLevel(utilization: number): MeterLevel {
  if (utilization >= 90) return "critical";
  if (utilization >= 75) return "high";
  return "normal";
}

/** The words that go with a level, so colour never carries it alone. */
export function meterLevelText(utilization: number): string | null {
  if (utilization >= 100) return "Limit reached";
  if (utilization >= 90) return "Near the limit";
  if (utilization >= 75) return "High";
  return null;
}

/** How long a session ran: to its end, or to its last activity while it has none. */
export function sessionDurationMs(session: {
  startedAt: Date;
  lastActivityAt: Date;
  endedAt: Date | null;
}): number {
  const end = session.endedAt ?? session.lastActivityAt;
  return Math.max(0, end.getTime() - session.startedAt.getTime());
}

/** A session with no end whose last activity is recent enough to call it running. */
export const ACTIVE_WINDOW_MS = 30 * MINUTE;

export function isSessionActive(
  session: { endedAt: Date | null; lastActivityAt: Date },
  now: Date,
): boolean {
  return (
    session.endedAt === null &&
    now.getTime() - session.lastActivityAt.getTime() < ACTIVE_WINDOW_MS
  );
}

/** "person" / "people". */
export function plural(count: number, one: string, many: string): string {
  return `${WHOLE.format(count)} ${count === 1 ? one : many}`;
}

function capitalize(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

function nonEmpty(value: string | null | undefined): string | null {
  const trimmed = value?.trim();
  return trimmed === undefined || trimmed === "" ? null : trimmed;
}
