import { describe, expect, it } from "vitest";

import {
  DEFAULT_USAGE_PERIOD,
  isUsagePeriod,
  periodDays,
  periodFromParam,
  periodLabel,
  periodPhrase,
  USAGE_PERIODS,
  withPeriod,
} from "~/lib/usage-period";
import { usagePeriodInput } from "~/server/api/routers/inputs";

describe("usage periods", () => {
  it("reach 7 days, 30 days or all the way back", () => {
    expect(USAGE_PERIODS).toEqual(["7d", "30d", "all"]);
    expect(USAGE_PERIODS.map(periodDays)).toEqual([7, 30, null]);
    expect(USAGE_PERIODS.map(periodLabel)).toEqual([
      "7 days",
      "30 days",
      "All time",
    ]);
    expect(USAGE_PERIODS.map(periodPhrase)).toEqual([
      "in the last 7 days",
      "in the last 30 days",
      "ever",
    ]);
    // The account cards' week.
    expect(DEFAULT_USAGE_PERIOD).toBe("7d");
  });

  it("read `?period=` and fall back to the default on anything else", () => {
    expect(periodFromParam(undefined)).toBe("7d");
    expect(periodFromParam("30d")).toBe("30d");
    expect(periodFromParam("all")).toBe("all");
    expect(periodFromParam(["all", "30d"])).toBe("all");
    for (const bad of ["", "90d", "ALL", "7", "constructor", "__proto__"]) {
      expect(periodFromParam(bad)).toBe("7d");
      expect(isUsagePeriod(bad)).toBe(false);
    }
    expect(periodFromParam([])).toBe("7d");
  });

  it("carry a period in links, leaving the default out", () => {
    expect(withPeriod("/projects", "7d")).toBe("/projects");
    expect(withPeriod("/projects", "30d")).toBe("/projects?period=30d");
    expect(withPeriod("/projects/cm1abc", "all")).toBe(
      "/projects/cm1abc?period=all",
    );
  });

  it("are exactly what the API takes", () => {
    for (const period of USAGE_PERIODS) {
      expect(usagePeriodInput.parse(period)).toBe(period);
    }
    expect(usagePeriodInput.safeParse("90d").success).toBe(false);
  });
});
