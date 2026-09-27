import { describe, expect, it } from "vitest";

import {
  accountSubtitle,
  accountTitle,
  formatCost,
  formatDuration,
  formatExact,
  formatPercent,
  formatShare,
  formatRelative,
  formatTokens,
  isSessionActive,
  meterLevel,
  meterLevelText,
  modelLabel,
  plural,
  sessionDurationMs,
  sessionSourceLabel,
  shareOf,
  usageSourceLabel,
  windowDescription,
  windowLabel,
} from "~/lib/format";

const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

describe("token counts", () => {
  it("read compactly, from bigint or number alike", () => {
    expect(formatTokens(0n)).toBe("0");
    expect(formatTokens(999n)).toBe("999");
    expect(formatTokens(1000n)).toBe("1K");
    expect(formatTokens(18_234n)).toBe("18.2K");
    expect(formatTokens(12_873_120n)).toBe("12.9M");
    expect(formatTokens(4_200_000_000n)).toBe("4.2B");
    expect(formatTokens(845_000)).toBe("845K");
  });

  it("stay exact where asked, beyond Number's safe range too", () => {
    expect(formatExact(12_873_120n)).toBe("12,873,120");
    expect(formatExact(9_007_199_254_740_993n)).toBe("9,007,199,254,740,993");
  });
});

describe("costs", () => {
  it("are dollars and cents, and unknown stays unknown", () => {
    expect(formatCost(14.82)).toBe("$14.82");
    expect(formatCost(1234.5)).toBe("$1,234.50");
    expect(formatCost(0)).toBe("$0.00");
    expect(formatCost(0.004)).toBe("<$0.01");
    expect(formatCost(null)).toBeNull();
    expect(formatCost(undefined)).toBeNull();
    expect(formatCost(Number.NaN)).toBeNull();
  });
});

describe("percentages", () => {
  it("never round a reading up to or past the limit", () => {
    expect(formatPercent(42)).toBe("42%");
    expect(formatPercent(61.5)).toBe("61%");
    expect(formatPercent(99.6)).toBe("99%");
    expect(formatPercent(100)).toBe("100%");
    expect(formatPercent(112.4)).toBe("112%");
    expect(formatPercent(-3)).toBe("0%");
    expect(formatPercent(Number.NaN)).toBe("–");
  });

  it("mark high and critical levels with words, not only colour", () => {
    expect(meterLevel(74.9)).toBe("normal");
    expect(meterLevelText(74.9)).toBeNull();
    expect(meterLevel(75)).toBe("high");
    expect(meterLevelText(80)).toBe("High");
    expect(meterLevel(90)).toBe("critical");
    expect(meterLevelText(95)).toBe("Near the limit");
    expect(meterLevelText(100)).toBe("Limit reached");
    expect(meterLevel(130)).toBe("critical");
  });
});

describe("durations and relative times", () => {
  it("read as the largest two units", () => {
    expect(formatDuration(45_000)).toBe("45 s");
    expect(formatDuration(12 * MIN + 30_000)).toBe("12 min");
    expect(formatDuration(HOUR)).toBe("1 h");
    expect(formatDuration(HOUR + 45 * MIN)).toBe("1 h 45 min");
    expect(formatDuration(2 * DAY + 3 * HOUR + 10 * MIN)).toBe("2 d 3 h");
    expect(formatDuration(3 * DAY)).toBe("3 d");
    expect(formatDuration(-1)).toBe("–");
  });

  it("describe past and future moments", () => {
    const now = new Date("2026-09-25T12:00:00Z");
    const at = (ms: number) => new Date(now.getTime() + ms);
    expect(formatRelative(at(-10_000), now)).toBe("just now");
    expect(formatRelative(at(-5 * MIN), now)).toBe("5 minutes ago");
    expect(formatRelative(at(-50_000), now)).toBe("1 minute ago");
    expect(formatRelative(at(-3 * HOUR), now)).toBe("3 hours ago");
    expect(formatRelative(at(-DAY), now)).toBe("yesterday");
    expect(formatRelative(at(-3 * DAY), now)).toBe("3 days ago");
    expect(formatRelative(at(-14 * DAY), now)).toBe("2 weeks ago");
    expect(formatRelative(at(-60 * DAY), now)).toBe("2 months ago");
    expect(formatRelative(at(-800 * DAY), now)).toBe("2 years ago");
    expect(formatRelative(at(2 * HOUR), now)).toBe("in 2 hours");
    expect(formatRelative(at(DAY), now)).toBe("tomorrow");
  });
});

describe("sessions", () => {
  const start = new Date("2026-09-25T08:00:00Z");

  it("last until their end, or their last activity while they have none", () => {
    expect(
      sessionDurationMs({
        startedAt: start,
        lastActivityAt: new Date(start.getTime() + HOUR),
        endedAt: new Date(start.getTime() + 2 * HOUR),
      }),
    ).toBe(2 * HOUR);
    expect(
      sessionDurationMs({
        startedAt: start,
        lastActivityAt: new Date(start.getTime() + HOUR),
        endedAt: null,
      }),
    ).toBe(HOUR);
    // Clock skew between Macs never makes a negative duration.
    expect(
      sessionDurationMs({
        startedAt: start,
        lastActivityAt: new Date(start.getTime() - MIN),
        endedAt: null,
      }),
    ).toBe(0);
  });

  it("count as active only without an end and with recent activity", () => {
    const now = new Date(start.getTime() + 10 * HOUR);
    const recent = new Date(now.getTime() - 5 * MIN);
    const stale = new Date(now.getTime() - 2 * HOUR);
    expect(
      isSessionActive({ endedAt: null, lastActivityAt: recent }, now),
    ).toBe(true);
    expect(isSessionActive({ endedAt: null, lastActivityAt: stale }, now)).toBe(
      false,
    );
    expect(
      isSessionActive({ endedAt: recent, lastActivityAt: recent }, now),
    ).toBe(false);
  });

  it("name where they ran and who reported usage", () => {
    expect(sessionSourceLabel("cli")).toBe("CLI");
    expect(sessionSourceLabel("vscode")).toBe("VS Code");
    expect(sessionSourceLabel("desktop")).toBe("Claude Desktop");
    expect(sessionSourceLabel("sdk")).toBe("Agent SDK");
    expect(sessionSourceLabel("other")).toBe("Other");
    expect(sessionSourceLabel("something-new")).toBe("Other");
    expect(usageSourceLabel("desktop")).toBe("Claude Desktop");
    expect(usageSourceLabel("probe")).toBe("Claude Code");
    expect(usageSourceLabel("statusLine")).toBe("Claude Code status line");
    expect(usageSourceLabel("claudeJson")).toBe("Claude Code's cache");
  });
});

describe("models and windows", () => {
  it("shorten known model ids and keep unknown ones readable", () => {
    expect(modelLabel("claude-opus-4-5-20251101")).toBe("Opus 4.5");
    expect(modelLabel("claude-haiku-4-5-20251001")).toBe("Haiku 4.5");
    expect(modelLabel("claude-sonnet-4-20250514")).toBe("Sonnet 4");
    expect(modelLabel("claude-opus-4-1-20250805")).toBe("Opus 4.1");
    expect(modelLabel("claude-3-5-sonnet-20241022")).toBe("Sonnet 3.5");
    expect(modelLabel("claude-3-opus-20240229")).toBe("Opus 3");
    expect(modelLabel("claude-sonnet-4-5[1m]")).toBe("Sonnet 4.5");
    expect(modelLabel("gpt-something")).toBe("gpt-something");
    expect(modelLabel("")).toBe("");
  });

  it("name usage windows the way Claude does", () => {
    expect(windowLabel("session")).toBe("5-hour");
    expect(windowLabel("weekly_all")).toBe("Weekly");
    expect(windowLabel("weekly_opus")).toBe("Weekly · Opus");
    expect(windowLabel("weekly_sonnet")).toBe("Weekly · Sonnet");
    expect(windowLabel("weekly_")).toBe("Weekly");
    expect(windowLabel("extra_usage")).toBe("Extra usage");
    expect(windowLabel("something_new")).toBe("Something new");
    expect(windowDescription("session")).toBe("5-hour session limit");
    expect(windowDescription("weekly_all")).toBe("Weekly limit, all models");
    expect(windowDescription("weekly_opus")).toBe("Weekly · Opus limit");
  });
});

describe("accounts", () => {
  it("go by the viewer's label, else the email, else the organization", () => {
    expect(accountTitle({ label: "Personal", email: "me@example.com" })).toBe(
      "Personal",
    );
    expect(accountTitle({ label: "  ", email: "me@example.com" })).toBe(
      "me@example.com",
    );
    expect(accountTitle({ organizationName: "Company" })).toBe("Company");
    expect(accountTitle({})).toBe("Claude account");
  });

  it("add what the title didn't say underneath", () => {
    expect(
      accountSubtitle({
        label: "Work",
        email: "me@company.com",
        organizationName: "Company",
      }),
    ).toBe("me@company.com · Company");
    expect(
      accountSubtitle({ email: "me@company.com", organizationName: "Company" }),
    ).toBe("Company");
    expect(accountSubtitle({ email: "me@example.com" })).toBeNull();
  });

  it("count people in words", () => {
    expect(plural(1, "person", "people")).toBe("1 person");
    expect(plural(3, "person", "people")).toBe("3 people");
    expect(plural(1200, "session", "sessions")).toBe("1,200 sessions");
  });
});

describe("shares of a whole", () => {
  it("divide token counts, beyond Number's safe range too", () => {
    expect(shareOf(1n, 4n)).toBe(0.25);
    expect(shareOf(0n, 4n)).toBe(0);
    expect(shareOf(4n, 4n)).toBe(1);
    // Nothing to share: no part of it.
    expect(shareOf(5n, 0n)).toBe(0);
    expect(shareOf(-1n, 4n)).toBe(0);
    expect(shareOf(9n, 4n)).toBe(1);
    const big = 2n ** 70n;
    expect(shareOf(big / 3n, big)).toBeCloseTo(1 / 3, 10);
    // However small, a part that is there is never none of it.
    expect(shareOf(1n, 10n ** 12n)).toBeGreaterThan(0);
    expect(shareOf(1n, 2n ** 80n)).toBeGreaterThan(0);
  });

  it("show a small project as there, never as none", () => {
    // 800 tokens of 22.8M, and 30K of half a billion: both read "<1%", neither "0%".
    expect(formatShare(shareOf(800n, 22_800_000n))).toBe("<1%");
    expect(formatShare(shareOf(30_000n, 500_000_000n))).toBe("<1%");
    expect(formatShare(shareOf(9_999_999n, 10_000_000n))).toBe("99%");
  });

  it("read as whole percentages, never all or none of it when it isn't", () => {
    expect(formatShare(0)).toBe("0%");
    expect(formatShare(0.00001)).toBe("<1%");
    expect(formatShare(0.0099)).toBe("<1%");
    expect(formatShare(0.01)).toBe("1%");
    expect(formatShare(0.424)).toBe("42%");
    expect(formatShare(0.425)).toBe("43%");
    expect(formatShare(0.996)).toBe("99%");
    expect(formatShare(1)).toBe("100%");
    expect(formatShare(Number.NaN)).toBe("0%");
    expect(formatShare(-0.5)).toBe("0%");
  });
});
