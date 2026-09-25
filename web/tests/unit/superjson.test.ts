/**
 * tRPC's transformer carries the router outputs' BigInt token totals and Dates intact.
 */
import superjson from "superjson";
import { describe, expect, it } from "vitest";

import { tokenTotals } from "~/server/services/totals";

describe("superjson", () => {
  it("round-trips BigInt totals beyond 2^53 and dates", () => {
    const value = {
      tokens: tokenTotals({
        inputTokens: 9_007_199_254_740_993n,
        outputTokens: 1n,
        cacheCreationTokens: null,
        cacheReadTokens: 2n,
      }),
      at: new Date("2026-09-25T09:47:03.000Z"),
    };
    const wire = JSON.stringify(superjson.serialize(value));
    const back = superjson.deserialize<typeof value>(
      JSON.parse(wire) as Parameters<typeof superjson.deserialize>[0],
    );
    expect(back).toEqual(value);
    expect(back.tokens.total).toBe(9_007_199_254_740_996n);
    expect(back.tokens.cacheCreation).toBe(0n);
  });
});
