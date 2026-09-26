import { describe, expect, it } from "vitest";

import {
  filterFromParams,
  isAccountKey,
  projectForId,
  SESSIONS_PAGE_SIZE,
  sessionsInput,
  usageFrom,
  usageInput,
  USAGE_HISTORY_DAYS,
} from "~/app/accounts/[key]/queries";

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
