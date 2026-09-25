/**
 * What a sync batch keeps once a quota is reached (src/server/services/sync.ts): the decisions are
 * pure, so every edge is checked here; tests/integration/sync.test.ts runs them against Postgres.
 */
import { describe, expect, it } from "vitest";

import {
  keepNewestWithinRoom,
  planUsage,
  usageRowKey,
  type UsageRow,
} from "~/server/services/sync";

const K1 = "a".repeat(64);
const K2 = "b".repeat(64);

function row(
  windowId: string,
  minute: number,
  accountKey = K1,
  source = "probe",
): UsageRow {
  return {
    accountKey,
    source,
    windowId,
    utilization: 10,
    resetsAt: null,
    observedAt: new Date(Date.UTC(2026, 8, 25, 12, minute)).toISOString(),
  };
}

const keys = (rows: readonly UsageRow[]) => rows.map(usageRowKey);

describe("keepNewestWithinRoom", () => {
  const items = [
    { id: "old-new", stored: false, at: 1 },
    { id: "stored", stored: true, at: 2 },
    { id: "newest-new", stored: false, at: 9 },
    { id: "middle-new", stored: false, at: 5 },
  ];
  const keep = (room: number) =>
    keepNewestWithinRoom(
      items,
      (i) => i.stored,
      (i) => i.at,
      room,
    ).map((i) => i.id);

  it("keeps what is stored already whatever the room", () => {
    expect(keep(0)).toEqual(["stored"]);
    expect(keep(-5)).toEqual(["stored"]);
  });

  it("fills the room with the newest new items, in their original order", () => {
    expect(keep(1)).toEqual(["stored", "newest-new"]);
    expect(keep(2)).toEqual(["stored", "newest-new", "middle-new"]);
    expect(keep(3)).toEqual(items.map((i) => i.id));
    expect(keep(100)).toEqual(items.map((i) => i.id));
  });
});

describe("planUsage", () => {
  const plan = (
    rows: UsageRow[],
    facts: Partial<Parameters<typeof planUsage>[1]> = {},
  ) =>
    planUsage(rows, {
      stored: new Set(),
      knownWindows: [],
      room: 20_000,
      windowsPerAccount: 32,
      ...facts,
    });

  it("inserts new readings and records their windows once", () => {
    const rows = [row("session", 1), row("weekly_all", 1), row("session", 2)];
    const result = plan(rows);
    expect(keys(result.insert)).toEqual(keys([rows[2]!, rows[0]!, rows[1]!])); // newest first
    expect(result.newWindows).toEqual([
      { accountKey: K1, windowId: "session" },
      { accountKey: K1, windowId: "weekly_all" },
    ]);
    expect([...result.kept].sort()).toEqual(keys(rows).sort());
  });

  it("keeps readings stored before without inserting or counting them", () => {
    const rows = [row("session", 1), row("session", 2)];
    const result = plan(rows, {
      stored: new Set([usageRowKey(rows[0]!)]),
      knownWindows: [{ accountKey: K1, windowId: "session" }],
      room: 1,
    });
    expect(keys(result.insert)).toEqual([usageRowKey(rows[1]!)]);
    expect(result.newWindows).toEqual([]);
    expect(result.kept).toEqual(new Set(keys(rows)));

    // A batch of nothing but stored readings needs no room at all.
    const again = plan(rows, { stored: new Set(keys(rows)), room: 0 });
    expect(again.insert).toEqual([]);
    expect(again.kept).toEqual(new Set(keys(rows)));
  });

  it("drops new readings past the day's room, keeping the newest", () => {
    const rows = [0, 1, 2, 3, 4].map((m) => row("session", m));
    const result = plan(rows, { room: 2 });
    expect(keys(result.insert)).toEqual(keys([rows[4]!, rows[3]!]));
    expect(result.kept).toEqual(new Set(keys([rows[3]!, rows[4]!])));

    expect(plan(rows, { room: 0 }).insert).toEqual([]);
    expect(plan(rows, { room: -3 }).kept.size).toBe(0);
  });

  it("drops readings of window ids past the per-account cap, ever", () => {
    const known = ["session", "weekly_all", "weekly_opus"].map((windowId) => ({
      accountKey: K1,
      windowId,
    }));
    const rows = [
      row("weekly_made_up_1", 5),
      row("session", 5),
      row("weekly_made_up_2", 4),
      row("weekly_made_up_3", 3),
      row("weekly_made_up_1", 2), // a window this batch already added: fine
      row("weekly_made_up_9", 1, K2), // another account has its own ids
    ];
    const result = plan(rows, { knownWindows: known, windowsPerAccount: 5 });
    expect(result.newWindows).toEqual([
      { accountKey: K1, windowId: "weekly_made_up_1" },
      { accountKey: K1, windowId: "weekly_made_up_2" },
      { accountKey: K2, windowId: "weekly_made_up_9" },
    ]);
    expect(result.insert.map((r) => `${r.windowId}@${r.accountKey}`)).toEqual([
      `weekly_made_up_1@${K1}`,
      `session@${K1}`,
      `weekly_made_up_2@${K1}`,
      `weekly_made_up_1@${K1}`,
      `weekly_made_up_9@${K2}`,
    ]);
    expect(result.kept.has(usageRowKey(rows[3]!))).toBe(false);

    // Already full: only the known windows get in.
    const full = plan(rows, { knownWindows: known, windowsPerAccount: 3 });
    expect(full.insert.map((r) => r.windowId)).toEqual([
      "session",
      "weekly_made_up_9",
    ]);
    expect(full.newWindows).toEqual([
      { accountKey: K2, windowId: "weekly_made_up_9" },
    ]);
  });

  it("gives room only to readings it stores, so a dropped window costs nothing", () => {
    const rows = [row("weekly_x", 9), row("session", 1)];
    const result = plan(rows, {
      knownWindows: [{ accountKey: K1, windowId: "session" }],
      windowsPerAccount: 1,
      room: 1,
    });
    expect(keys(result.insert)).toEqual([usageRowKey(rows[1]!)]);
  });

  it("records a window only when one of its readings is stored", () => {
    const rows = [row("session", 2), row("weekly_new", 1)];
    const result = plan(rows, { room: 1 });
    expect(result.newWindows).toEqual([
      { accountKey: K1, windowId: "session" },
    ]);
  });
});
