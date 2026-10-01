import { describe, expect, it } from "vitest";

import {
  filterFromParams,
  isAccountKey,
  projectForId,
  projectUsageInput,
  SESSIONS_PAGE_SIZE,
  sessionsInput,
  usageFrom,
  usageInput,
  USAGE_HISTORY_DAYS,
} from "~/app/accounts/[key]/queries";
import {
  DASHBOARD_PROJECTS_LIMIT,
  dashboardProjectsInput,
} from "~/app/dashboard/queries";
import {
  asksForAccounts,
  combinedUsageInput,
  USAGE_PROJECTS_LIMIT,
  usageProjectsInput,
  usageTimelineInput,
} from "~/app/usage/queries";
import { selectionFromParam } from "~/lib/account-selection";
import {
  accountProjectHref,
  allProjectsInput,
  isProjectId,
  PROJECT_SESSIONS_PAGE_SIZE,
  projectDetailInput,
  projectSessionsInput,
} from "~/app/projects/queries";

const KEY = "8ca65b0df91fc776aded7f419f011fc1e99e2fd120e893692cfaf83f0fa994c0";

describe("account page queries", () => {
  it("read filters from the address bar and drop anything malformed", () => {
    expect(filterFromParams({})).toEqual({
      projectId: undefined,
      ownerId: undefined,
    });
    expect(
      filterFromParams({
        project: "cm1abcdef0000xyz",
        member: "3f1c2a9e-8b7d-4c6e-9a1b-2c3d4e5f6a7b",
      }),
    ).toEqual({
      projectId: "cm1abcdef0000xyz",
      ownerId: "3f1c2a9e-8b7d-4c6e-9a1b-2c3d4e5f6a7b",
    });
    expect(filterFromParams({ project: ["first", "second"] }).projectId).toBe(
      "first",
    );
    expect(
      filterFromParams({ project: "a b", member: "x".repeat(129) }),
    ).toEqual({
      projectId: undefined,
      ownerId: undefined,
    });
  });

  it("build the same sessions input on the server and in the browser", () => {
    // The server prefetches with a filter whose fields may be undefined; the browser builds it
    // from state. Both must hash to one query key, so absent fields are left out entirely.
    expect(sessionsInput(KEY, {})).toEqual({
      accountKey: KEY,
      limit: SESSIONS_PAGE_SIZE,
    });
    expect(
      Object.keys(
        sessionsInput(KEY, { projectId: undefined, ownerId: undefined }),
      ),
    ).toEqual(["accountKey", "limit"]);
    expect(sessionsInput(KEY, { projectId: "p1", ownerId: "u1" })).toEqual({
      accountKey: KEY,
      projectId: "p1",
      ownerId: "u1",
      limit: SESSIONS_PAGE_SIZE,
    });
  });

  it("start the usage charts 30 days back, on the hour", () => {
    const now = Date.parse("2026-09-25T10:47:12.345Z");
    const from = usageFrom(now);
    expect(new Date(from).toISOString()).toBe("2026-08-26T10:00:00.000Z");
    expect(now - from).toBeGreaterThanOrEqual(
      USAGE_HISTORY_DAYS * 24 * 3600_000,
    );
    expect(now - from).toBeLessThan((USAGE_HISTORY_DAYS * 24 + 1) * 3600_000);
    // Reloads within the hour ask for the same range.
    expect(usageFrom(now + 5 * 60_000)).toBe(from);
    expect(usageInput(KEY, from)).toEqual({
      accountKey: KEY,
      from: new Date(from),
    });
  });

  it("select a whole project group by any of its rows' ids", () => {
    // One person's "agentnotch" from two Macs, and another project.
    const projects = [
      { id: "p1", projectIds: ["p1", "p7"], name: "agentnotch" },
      { id: "p2", projectIds: ["p2"], name: "billing" },
    ];
    expect(projectForId(projects, "p1")?.name).toBe("agentnotch");
    expect(projectForId(projects, "p7")?.id).toBe("p1");
    expect(projectForId(projects, "p2")?.id).toBe("p2");
    expect(projectForId(projects, "gone")).toBeUndefined();
    expect(projectForId(projects, undefined)).toBeUndefined();
  });

  it("recognise account keys", () => {
    expect(isAccountKey(KEY)).toBe(true);
    expect(isAccountKey(KEY.toUpperCase())).toBe(false);
    expect(isAccountKey(KEY.slice(1))).toBe(false);
  });
});

describe("usage by project queries", () => {
  it("ask for the dashboard's and the account's first projects in the period", () => {
    expect(dashboardProjectsInput("30d")).toEqual({
      period: "30d",
      limit: DASHBOARD_PROJECTS_LIMIT,
    });
    // The account page lists every project in the period: no limit at all, not an undefined one.
    expect(projectUsageInput(KEY, "all")).toEqual({
      period: "all",
      accountKey: KEY,
    });
    expect(Object.keys(projectUsageInput(KEY, "7d"))).toEqual([
      "period",
      "accountKey",
    ]);
    // Every project on the projects page: no limit at all, not an undefined one.
    expect(Object.keys(allProjectsInput("7d"))).toEqual(["period"]);
  });

  it("ask for one project on every account", () => {
    expect(projectDetailInput("cm1abc", "7d")).toEqual({
      id: "cm1abc",
      period: "7d",
    });
    expect(projectSessionsInput("cm1abc")).toEqual({
      projectId: "cm1abc",
      acrossAccounts: true,
      limit: PROJECT_SESSIONS_PAGE_SIZE,
    });
  });

  it("link a project's account to its sessions there, over the same period", () => {
    expect(accountProjectHref(KEY, "cm1abc", "7d")).toBe(
      `/accounts/${KEY}?project=cm1abc#sessions`,
    );
    expect(accountProjectHref(KEY, "cm1abc", "all")).toBe(
      `/accounts/${KEY}?project=cm1abc&period=all#sessions`,
    );
    // What the account page reads back from it.
    const url = new URL(
      accountProjectHref(KEY, "cm1_a-b", "30d"),
      "https://x.invalid",
    );
    expect(
      filterFromParams({
        project: url.searchParams.get("project") ?? undefined,
      }),
    ).toEqual({ projectId: "cm1_a-b", ownerId: undefined });
    expect(url.searchParams.get("period")).toBe("30d");
  });

  it("recognise project ids", () => {
    expect(isProjectId("cm1abcdef0000xyz")).toBe(true);
    expect(isProjectId("0b8e1f7a-9c2d-4e3f-8a1b-2c3d4e5f6a7b")).toBe(true);
    expect(isProjectId("")).toBe(false);
    expect(isProjectId("a b")).toBe(false);
    expect(isProjectId("../accounts")).toBe(false);
    expect(isProjectId("x".repeat(129))).toBe(false);
  });
});

describe("usage page queries", () => {
  const K2 = "d8485d82cdbceb2582311953e97b1666022b76dcbb98ef283fece56b1b5b8874";

  it("ask about every account without a selection: no keys at all, not an undefined list", () => {
    expect(combinedUsageInput("7d", null)).toEqual({ period: "7d" });
    expect(Object.keys(combinedUsageInput("7d", null))).toEqual(["period"]);
    expect(usageProjectsInput("all", null)).toEqual({
      period: "all",
      limit: USAGE_PROJECTS_LIMIT,
    });
    expect(usageTimelineInput("30d", null, "Asia/Kolkata")).toEqual({
      period: "30d",
      timeZone: "Asia/Kolkata",
    });
  });

  it("build the same input from the address bar on the server and in the browser", () => {
    // The server reads `?accounts=` from its search params, the browser from useSearchParams:
    // both through selectionFromParam, so the keys arrive in one order.
    const fromServer = selectionFromParam(`${K2},${KEY}`);
    const fromBrowser = selectionFromParam(
      new URLSearchParams(`accounts=${KEY}%2C${K2}`).get("accounts"),
    );
    expect(combinedUsageInput("30d", fromServer)).toEqual(
      combinedUsageInput("30d", fromBrowser),
    );
    expect(combinedUsageInput("30d", fromServer)).toEqual({
      period: "30d",
      accountKeys: [KEY, K2],
    });
    expect(usageProjectsInput("7d", [K2])).toEqual({
      period: "7d",
      accountKeys: [K2],
      limit: USAGE_PROJECTS_LIMIT,
    });
  });

  it("ask nothing for a selection of no accounts", () => {
    expect(asksForAccounts(null)).toBe(true);
    expect(asksForAccounts([KEY])).toBe(true);
    expect(asksForAccounts([])).toBe(false);
    expect(asksForAccounts(selectionFromParam(""))).toBe(false);
  });
});
