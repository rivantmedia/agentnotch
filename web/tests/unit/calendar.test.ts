/**
 * Calendar buckets in the viewer's zone (lib/calendar.ts): local midnights through clock
 * changes and odd offsets, Monday weeks, months, and the buckets a period splits into.
 */
import { describe, expect, it } from "vitest";

import {
  bucketStarts,
  knownTimeZone,
  localDate,
  startOfDay,
  startOfUnit,
  unitForSpan,
} from "~/lib/calendar";

const iso = (t: number) => new Date(t).toISOString();
const at = (s: string) => Date.parse(s);
const DAY = 24 * 60 * 60 * 1000;

describe("time zones", () => {
  it("know the zones ICU knows, as named", () => {
    expect(knownTimeZone("Asia/Kolkata")).toBe("Asia/Kolkata");
    expect(knownTimeZone("UTC")).toBe("UTC");
    expect(knownTimeZone("america/new_york")).toBe("america/new_york");
    for (const name of ["", "Mars/Olympus_Mons", "Etc/Unknown", "not a zone"]) {
      expect(knownTimeZone(name), name).toBeNull();
    }
  });

  it("read the local date of an instant", () => {
    // 20:00 UTC on the 24th is 01:30 on the 25th in India, and still the 24th in New York.
    expect(localDate(at("2026-09-24T20:00:00Z"), "Asia/Kolkata")).toEqual({
      year: 2026,
      month: 9,
      day: 25,
    });
    expect(localDate(at("2026-09-24T20:00:00Z"), "America/New_York")).toEqual({
      year: 2026,
      month: 9,
      day: 24,
    });
  });
});

describe("the start of a local day", () => {
  it("is midnight, whatever the offset", () => {
    const day = { year: 2026, month: 9, day: 25 };
    expect(iso(startOfDay(day, "UTC"))).toBe("2026-09-25T00:00:00.000Z");
    expect(iso(startOfDay(day, "Asia/Kolkata"))).toBe(
      "2026-09-24T18:30:00.000Z",
    );
    expect(iso(startOfDay(day, "Asia/Kathmandu"))).toBe(
      "2026-09-24T18:15:00.000Z",
    );
    expect(iso(startOfDay(day, "Pacific/Chatham"))).toBe(
      "2026-09-24T11:15:00.000Z",
    );
    expect(iso(startOfDay(day, "Pacific/Kiritimati"))).toBe(
      "2026-09-24T10:00:00.000Z",
    );
  });

  it("follows clock changes, so a day can be 23 or 25 hours long", () => {
    const ny = (day: number, month: number) =>
      startOfDay({ year: 2026, month, day }, "America/New_York");
    expect(iso(ny(8, 3))).toBe("2026-03-08T05:00:00.000Z");
    expect(ny(9, 3) - ny(8, 3)).toBe(23 * 60 * 60 * 1000);
    expect(iso(ny(1, 11))).toBe("2026-11-01T04:00:00.000Z");
    expect(ny(2, 11) - ny(1, 11)).toBe(25 * 60 * 60 * 1000);
  });

  it("begins after the gap where a clock change skips midnight", () => {
    // Chile went from 00:00 straight to 01:00 on 8 September 2024: that day began at 01:00.
    const santiago = (day: number, month: number, year: number) =>
      iso(startOfDay({ year, month, day }, "America/Santiago"));
    expect(santiago(7, 9, 2024)).toBe("2024-09-07T04:00:00.000Z");
    expect(santiago(8, 9, 2024)).toBe("2024-09-08T04:00:00.000Z");
    expect(santiago(9, 9, 2024)).toBe("2024-09-09T03:00:00.000Z");
    // On 6 April 2025 its clocks went back from 00:00 to 23:00 the day before: the 6th began
    // at the second midnight.
    expect(santiago(6, 4, 2025)).toBe("2025-04-06T04:00:00.000Z");
  });
});

describe("units", () => {
  it("start weeks on Monday and months on the 1st, locally", () => {
    // Friday 25 September 2026, in India.
    const t = at("2026-09-25T12:00:00Z");
    expect(iso(startOfUnit(t, "day", "Asia/Kolkata"))).toBe(
      "2026-09-24T18:30:00.000Z",
    );
    expect(iso(startOfUnit(t, "week", "Asia/Kolkata"))).toBe(
      "2026-09-20T18:30:00.000Z",
    );
    expect(iso(startOfUnit(t, "month", "Asia/Kolkata"))).toBe(
      "2026-08-31T18:30:00.000Z",
    );
    // A Sunday belongs to the week that began the Monday before.
    expect(iso(startOfUnit(at("2026-09-27T12:00:00Z"), "week", "UTC"))).toBe(
      "2026-09-21T00:00:00.000Z",
    );
  });

  it("count all time in days, then weeks, then months", () => {
    expect(unitForSpan(0)).toBe("day");
    expect(unitForSpan(62 * DAY)).toBe("day");
    expect(unitForSpan(62 * DAY + 1)).toBe("week");
    expect(unitForSpan(62 * 7 * DAY)).toBe("week");
    expect(unitForSpan(62 * 7 * DAY + 1)).toBe("month");
  });
});

describe("bucket starts", () => {
  it("begin a rolling period partway through its first day", () => {
    const from = at("2026-09-18T15:00:00Z");
    const to = at("2026-09-25T15:00:00Z");
    const starts = bucketStarts(from, to, "day", "UTC").map(iso);
    expect(starts).toEqual([
      "2026-09-18T15:00:00.000Z",
      ...[19, 20, 21, 22, 23, 24, 25].map((d) => `2026-09-${d}T00:00:00.000Z`),
    ]);
  });

  it("leave out a start at or after the end", () => {
    const starts = bucketStarts(
      at("2026-09-19T00:00:00Z"),
      at("2026-09-26T00:00:00Z"),
      "day",
      "UTC",
    );
    expect(starts).toHaveLength(7);
    expect(iso(starts.at(-1)!)).toBe("2026-09-25T00:00:00.000Z");
    // Never empty, even with no time at all.
    expect(
      bucketStarts(
        at("2026-09-19T00:00:00Z"),
        at("2026-09-19T00:00:00Z"),
        "day",
        "UTC",
      ),
    ).toEqual([at("2026-09-19T00:00:00Z")]);
  });

  it("count weeks and months across clock changes", () => {
    expect(
      bucketStarts(
        at("2026-10-20T15:00:00Z"),
        at("2026-11-10T00:00:00Z"),
        "week",
        "America/New_York",
      ).map(iso),
    ).toEqual([
      "2026-10-20T15:00:00.000Z",
      "2026-10-26T04:00:00.000Z",
      "2026-11-02T05:00:00.000Z",
      "2026-11-09T05:00:00.000Z",
    ]);
    const from = startOfUnit(
      at("2026-01-15T00:00:00Z"),
      "month",
      "Asia/Kolkata",
    );
    expect(
      bucketStarts(
        from,
        at("2026-04-02T00:00:00Z"),
        "month",
        "Asia/Kolkata",
      ).map(iso),
    ).toEqual([
      "2025-12-31T18:30:00.000Z",
      "2026-01-31T18:30:00.000Z",
      "2026-02-28T18:30:00.000Z",
      "2026-03-31T18:30:00.000Z",
    ]);
  });
});
