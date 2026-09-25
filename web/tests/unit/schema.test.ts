/**
 * The sync request schema holds every limit and format in contract/README.md, and nothing
 * stricter than it says.
 */
import { describe, expect, it } from "vitest";

import {
  LIMITS,
  makeSyncRequestSchema,
  syncRequestSchema as realClockSchema,
  unknownAccountRefs,
} from "~/server/app-api/schema";

import { syncFixture, type SyncFixture } from "../support/fixtures";

/** The server's clock in these tests: just after everything in the fixture. */
const SERVER_NOW = Date.parse("2026-09-25T11:21:00Z");
const syncRequestSchema = makeSyncRequestSchema(() => SERVER_NOW);

const KEY_A =
  "8ca65b0df91fc776aded7f419f011fc1e99e2fd120e893692cfaf83f0fa994c0";

function accepts(edit: (r: SyncFixture) => void): boolean {
  const request = syncFixture();
  edit(request);
  return syncRequestSchema.safeParse(request).success;
}

function issuePaths(edit: (r: SyncFixture) => void): string[] {
  const request = syncFixture();
  edit(request);
  const result = syncRequestSchema.safeParse(request);
  return result.success ? [] : result.error.issues.map((i) => i.path.join("."));
}

const s0 = (r: SyncFixture) => r.sessions[0]!;
const u0 = (r: SyncFixture) => r.usage[0]!;
const a0 = (r: SyncFixture) => r.accounts[0]!;
const text = (n: number) => "x".repeat(n);

function session(i: number): SyncFixture["sessions"][number] {
  const base = syncFixture().sessions[0]!;
  const hex = i.toString(16).padStart(12, "0");
  return { ...base, sessionId: `a1b2c3d4-e5f6-4789-8abc-${hex}` };
}

describe("sync request limits", () => {
  const cases: Array<
    [string, (r: SyncFixture) => void, (r: SyncFixture) => void]
  > = [
    [
      "accounts ≤ 50",
      (r) => {
        r.accounts = Array.from({ length: LIMITS.accounts }, (_, i) => ({
          ...a0(r),
          key: i === 0 ? KEY_A : i.toString(16).padStart(64, "0"),
        }));
        r.sessions = [s0(r)];
        r.usage = [u0(r)];
      },
      (r) => {
        r.accounts = Array.from({ length: LIMITS.accounts + 1 }, (_, i) => ({
          ...a0(r),
          key: i === 0 ? KEY_A : i.toString(16).padStart(64, "0"),
        }));
        r.sessions = [s0(r)];
        r.usage = [u0(r)];
      },
    ],
    [
      "sessions ≤ 200",
      (r) =>
        (r.sessions = Array.from({ length: LIMITS.sessions }, (_, i) =>
          session(i),
        )),
      (r) =>
        (r.sessions = Array.from({ length: LIMITS.sessions + 1 }, (_, i) =>
          session(i),
        )),
    ],
    [
      "usage ≤ 500",
      (r) => (r.usage = Array.from({ length: LIMITS.usage }, () => u0(r))),
      (r) => (r.usage = Array.from({ length: LIMITS.usage + 1 }, () => u0(r))),
    ],
    [
      "windows ≤ 20",
      (r) =>
        (u0(r).windows = Array.from({ length: 20 }, () => u0(r).windows[0]!)),
      (r) =>
        (u0(r).windows = Array.from({ length: 21 }, () => u0(r).windows[0]!)),
    ],
    [
      "models ≤ 10",
      (r) =>
        (s0(r).models = Array.from({ length: 10 }, (_, i) => `model-${i}`)),
      (r) =>
        (s0(r).models = Array.from({ length: 11 }, (_, i) => `model-${i}`)),
    ],
    [
      "device.name ≤ 120",
      (r) => (r.device.name = text(120)),
      (r) => (r.device.name = text(121)),
    ],
    [
      "device.appVersion ≤ 40",
      (r) => (r.device.appVersion = text(40)),
      (r) => (r.device.appVersion = text(41)),
    ],
    [
      "email ≤ 320",
      (r) => (a0(r).email = text(320)),
      (r) => (a0(r).email = text(321)),
    ],
    [
      "organizationName ≤ 200",
      (r) => (a0(r).organizationName = text(200)),
      (r) => (a0(r).organizationName = text(201)),
    ],
    [
      "plan ≤ 60",
      (r) => (a0(r).plan = text(60)),
      (r) => (a0(r).plan = text(61)),
    ],
    [
      "label ≤ 80",
      (r) => (a0(r).label = text(80)),
      (r) => (a0(r).label = text(81)),
    ],
    [
      "project.name ≤ 120",
      (r) => (s0(r).project.name = text(120)),
      (r) => (s0(r).project.name = text(121)),
    ],
    [
      "title ≤ 200",
      (r) => (s0(r).title = text(200)),
      (r) => (s0(r).title = text(201)),
    ],
    [
      "summary.text ≤ 2000",
      (r) => (s0(r).summary!.text = text(2000)),
      (r) => (s0(r).summary!.text = text(2001)),
    ],
    [
      "summary.model ≤ 80",
      (r) => (s0(r).summary!.model = text(80)),
      (r) => (s0(r).summary!.model = text(81)),
    ],
  ];

  for (const [name, atLimit, overLimit] of cases) {
    it(name, () => {
      expect(accepts(atLimit)).toBe(true);
      expect(accepts(overLimit)).toBe(false);
    });
  }

  it("counts string length in UTF-16 units, as the app clamps them", () => {
    // "é" is one unit, an emoji two.
    expect(accepts((r) => (s0(r).title = "é".repeat(200)))).toBe(true);
    expect(accepts((r) => (s0(r).title = "😀".repeat(100)))).toBe(true);
    expect(accepts((r) => (s0(r).title = `${"😀".repeat(100)}x`))).toBe(false);
  });
});

describe("sync request formats", () => {
  it("requires schemaVersion 1", () => {
    expect(accepts((r) => (r.schemaVersion = 2))).toBe(false);
    expect(
      accepts((r) => delete (r as Partial<SyncFixture>).schemaVersion),
    ).toBe(false);
  });

  it("requires 64 lowercase hex keys", () => {
    expect(accepts((r) => (a0(r).key = KEY_A.toUpperCase()))).toBe(false);
    expect(accepts((r) => (a0(r).key = KEY_A.slice(1)))).toBe(false);
    expect(accepts((r) => (a0(r).key = `${KEY_A}0`))).toBe(false);
    expect(accepts((r) => (s0(r).project.key = "g".repeat(64)))).toBe(false);
    expect(
      accepts((r) => (s0(r).project.key = "/Users/me/code/agentnotch")),
    ).toBe(false);
  });

  it("requires UUIDs for the device and session ids, in either case", () => {
    expect(accepts((r) => (r.device.id = r.device.id.toLowerCase()))).toBe(
      true,
    );
    expect(
      accepts((r) => (s0(r).sessionId = s0(r).sessionId.toUpperCase())),
    ).toBe(true);
    expect(accepts((r) => (r.device.id = "studio-macbook"))).toBe(false);
    expect(accepts((r) => (s0(r).sessionId = "not-a-uuid"))).toBe(false);
    expect(accepts((r) => (s0(r).sessionId = ""))).toBe(false);
  });

  it("requires ISO 8601 UTC dates ending in Z", () => {
    expect(accepts((r) => (s0(r).startedAt = "2026-09-25T08:02:11Z"))).toBe(
      true,
    );
    expect(
      accepts((r) => (s0(r).startedAt = "2026-09-25T08:02:11.482123Z")),
    ).toBe(true);
    expect(
      accepts((r) => (s0(r).startedAt = "2026-09-25T08:02:11+00:00")),
    ).toBe(false);
    expect(accepts((r) => (s0(r).startedAt = "2026-09-25T08:02:11"))).toBe(
      false,
    );
    expect(accepts((r) => (s0(r).startedAt = "2026-09-25"))).toBe(false);
    expect(accepts((r) => (s0(r).startedAt = "yesterday"))).toBe(false);
    expect(accepts((r) => (u0(r).observedAt = "2026-13-01T00:00:00Z"))).toBe(
      false,
    );
    expect(accepts((r) => (s0(r).summary!.generatedAt = "1727254800"))).toBe(
      false,
    );
    expect(
      accepts((r) => (u0(r).windows[0]!.resetsAt = "2026-09-25 13:00:00Z")),
    ).toBe(false);
  });

  it("bounds every date between 2023-01-01 and a day after the server's clock", () => {
    const dayAhead = "2026-09-26T11:21:00Z";
    const pastDayAhead = "2026-09-26T11:21:00.001Z";
    const setters: Array<[string, (r: SyncFixture, v: string) => void]> = [
      ["sessions.0.startedAt", (r, v) => (s0(r).startedAt = v)],
      ["sessions.0.lastActivityAt", (r, v) => (s0(r).lastActivityAt = v)],
      ["sessions.0.endedAt", (r, v) => (s0(r).endedAt = v)],
      [
        "sessions.0.summary.generatedAt",
        (r, v) => (s0(r).summary!.generatedAt = v),
      ],
      ["usage.0.observedAt", (r, v) => (u0(r).observedAt = v)],
    ];
    for (const [path, set] of setters) {
      expect(
        accepts((r) => set(r, dayAhead)),
        path,
      ).toBe(true);
      expect(
        accepts((r) => set(r, "2023-01-01T00:00:00Z")),
        path,
      ).toBe(true);
      expect(issuePaths((r) => set(r, pastDayAhead))).toEqual([path]);
      expect(issuePaths((r) => set(r, "2022-12-31T23:59:59.999Z"))).toEqual([
        path,
      ]);
      expect(issuePaths((r) => set(r, "9999-12-31T23:59:59Z"))).toEqual([path]);
    }
  });

  it("lets a window's reset lie ahead, as weekly ones do, within reason", () => {
    const reset = (v: string) => (r: SyncFixture) =>
      (u0(r).windows[1]!.resetsAt = v);
    expect(accepts(reset("2026-10-02T11:21:00Z"))).toBe(true); // a week ahead
    expect(accepts(reset("2026-10-27T11:21:00Z"))).toBe(true); // 32 days
    expect(accepts(reset("2026-10-27T11:21:00.001Z"))).toBe(false);
    expect(accepts(reset("9999-12-31T23:59:59Z"))).toBe(false);
    expect(accepts(reset("2022-12-31T23:59:59Z"))).toBe(false);
  });

  it("reads the real clock at every parse when none is given", () => {
    const request = syncFixture();
    request.usage[0]!.observedAt = new Date(
      Date.now() + 2 * 24 * 60 * 60 * 1000,
    ).toISOString();
    expect(realClockSchema.safeParse(request).success).toBe(false);
    request.usage[0]!.observedAt = new Date(Date.now() - 1000).toISOString();
    expect(realClockSchema.safeParse(request).success).toBe(true);
  });

  it("accepts null where the contract allows it, and nowhere else", () => {
    expect(accepts((r) => (s0(r).endedAt = null))).toBe(true);
    expect(accepts((r) => (s0(r).title = null))).toBe(true);
    expect(accepts((r) => (s0(r).costUsd = null))).toBe(true);
    expect(accepts((r) => (a0(r).email = null))).toBe(true);
    expect(accepts((r) => (u0(r).windows[0]!.resetsAt = null))).toBe(true);
    expect(
      accepts((r) => ((s0(r) as Record<string, unknown>).startedAt = null)),
    ).toBe(false);
    expect(
      accepts((r) => ((s0(r) as Record<string, unknown>).project = null)),
    ).toBe(false);
    expect(accepts((r) => ((r as Record<string, unknown>).device = null))).toBe(
      false,
    );
  });

  it("allows the summary to be absent, null or an object", () => {
    expect(accepts((r) => delete s0(r).summary)).toBe(true);
    expect(accepts((r) => (s0(r).summary = null))).toBe(true);
    expect(
      accepts((r) => ((s0(r) as Record<string, unknown>).summary = "text")),
    ).toBe(false);
    expect(
      accepts(
        (r) =>
          ((s0(r) as Record<string, unknown>).summary = { text: "only text" }),
      ),
    ).toBe(false);
  });

  it("restricts sources to the contract's enums", () => {
    for (const source of ["cli", "vscode", "desktop", "sdk", "other"]) {
      expect(accepts((r) => (s0(r).source = source))).toBe(true);
    }
    expect(accepts((r) => (s0(r).source = "web"))).toBe(false);
    expect(accepts((r) => (s0(r).source = "CLI"))).toBe(false);
    for (const source of ["probe", "statusLine", "claudeJson", "desktop"]) {
      expect(accepts((r) => (u0(r).source = source))).toBe(true);
    }
    expect(accepts((r) => (u0(r).source = "statusline"))).toBe(false);
  });

  it("requires non-negative integer counts", () => {
    expect(accepts((r) => (s0(r).tokens.input = 0))).toBe(true);
    expect(accepts((r) => (s0(r).tokens.cacheRead = 9_000_000_000))).toBe(true);
    expect(accepts((r) => (s0(r).tokens.input = -1))).toBe(false);
    expect(accepts((r) => (s0(r).tokens.output = 1.5))).toBe(false);
    expect(
      accepts((r) => ((s0(r).tokens as Record<string, unknown>).output = "12")),
    ).toBe(false);
    expect(
      accepts(
        (r) => (s0(r).tokens.cacheCreation = Number.MAX_SAFE_INTEGER + 2),
      ),
    ).toBe(false);
    expect(accepts((r) => (s0(r).messageCount = -1))).toBe(false);
    expect(accepts((r) => (s0(r).messageCount = 2.5))).toBe(false);
  });

  it("accepts a cost ≥ 0 or null", () => {
    expect(accepts((r) => (s0(r).costUsd = 0))).toBe(true);
    expect(accepts((r) => (s0(r).costUsd = 1234.567891))).toBe(true);
    expect(accepts((r) => (s0(r).costUsd = -0.01))).toBe(false);
    expect(accepts((r) => (s0(r).costUsd = 1e9))).toBe(false);
  });

  it("accepts the contract's window ids and utilization above 100", () => {
    for (const id of [
      "session",
      "weekly_all",
      "weekly_opus",
      "weekly_sonnet_4_5",
      "weekly_claude-4.5",
      `weekly_${"a".repeat(60)}`,
      "extra_usage",
    ]) {
      expect(accepts((r) => (u0(r).windows[0]!.id = id))).toBe(true);
    }
    for (const id of [
      "",
      "weekly_",
      "monthly",
      "SESSION",
      "weekly_Opus", // the app folds names to lowercase
      "weekly all",
      "weekly_ä",
      `weekly_${"a".repeat(61)}`,
    ]) {
      expect(accepts((r) => (u0(r).windows[0]!.id = id))).toBe(false);
    }
    expect(accepts((r) => (u0(r).windows[0]!.utilization = 0))).toBe(true);
    expect(accepts((r) => (u0(r).windows[0]!.utilization = 137.5))).toBe(true);
    expect(accepts((r) => (u0(r).windows[0]!.utilization = -1))).toBe(false);
  });

  it("accepts empty batches", () => {
    expect(
      accepts((r) => {
        r.accounts = [];
        r.sessions = [];
        r.usage = [];
      }),
    ).toBe(true);
  });

  it("rejects sessions and readings whose account is not in accounts[]", () => {
    const other = "f".repeat(64);
    expect(issuePaths((r) => (s0(r).accountKey = other))).toEqual([
      "sessions.0.accountKey",
    ]);
    expect(issuePaths((r) => (u0(r).accountKey = other))).toEqual([
      "usage.0.accountKey",
    ]);
    expect(
      issuePaths((r) => {
        r.accounts = [];
      }),
    ).toEqual([
      "sessions.0.accountKey",
      "sessions.1.accountKey",
      "usage.0.accountKey",
      "usage.1.accountKey",
    ]);
    expect(
      unknownAccountRefs({
        accounts: [{ key: KEY_A }],
        sessions: [{ accountKey: KEY_A }, { accountKey: other }],
        usage: [{ accountKey: other }],
      }),
    ).toEqual(["sessions.1.accountKey", "usage.0.accountKey"]);
  });

  it("rejects a body that is not an object", () => {
    for (const body of [null, [], "sync", 1, true]) {
      expect(syncRequestSchema.safeParse(body).success).toBe(false);
    }
  });
});
