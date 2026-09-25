/**
 * The migrated database itself: every table keeps row-level security on (Supabase's Data API
 * would otherwise serve it to anyone holding the publishable key), new tables included.
 */
import { describe, expect, it } from "vitest";

import { db } from "./db";

describe("migrations", () => {
  it("turn on row-level security for every table in public", async () => {
    const tables = await db.$queryRaw<
      Array<{ name: string; rls: boolean }>
    >`SELECT c.relname AS name, c.relrowsecurity AS rls
      FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
      WHERE n.nspname = 'public' AND c.relkind = 'r'
      ORDER BY c.relname`;
    const names = tables.map((t) => t.name);
    for (const expected of [
      "Device",
      "PoolJoinFailure",
      "RateLimit",
      "Session",
      "UsageWindow",
    ]) {
      expect(names).toContain(expected);
    }
    expect(tables.filter((t) => !t.rls).map((t) => t.name)).toEqual([]);
  });
});
