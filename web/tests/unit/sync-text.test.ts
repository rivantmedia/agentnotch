/**
 * Text from the Mac is made storable before it reaches Postgres, which refuses NUL in `text` and
 * `jsonb` and anything that isn't valid UTF-8. Refusing it instead would fail the whole batch,
 * and the app would send the same batch again forever.
 */
import { describe, expect, it } from "vitest";

import { clean, SYNC_QUOTAS, TOKEN_CAP } from "~/server/services/sync";

const REPLACEMENT = String.fromCharCode(0xfffd);

describe("clean", () => {
  it("drops NUL and replaces unpaired surrogates", () => {
    expect(clean("a\u0000b")).toBe("ab");
    expect(clean("x\uD800y")).toBe(`x${REPLACEMENT}y`);
    expect(clean("x\uDC00y")).toBe(`x${REPLACEMENT}y`);
    expect(clean("\uDC00\uD800")).toBe(`${REPLACEMENT}${REPLACEMENT}`);
  });

  it("leaves ordinary text alone, emoji included", () => {
    for (const text of ["Fix billing retries", "é ü 中文", "😀 done", ""]) {
      expect(clean(text)).toBe(text);
    }
  });

  it("leaves nothing in the JSON the bulk statements carry that jsonb refuses", () => {
    const json = JSON.stringify(clean("a\u0000\uD83D"));
    expect(json).toBe(`"a${REPLACEMENT}"`);
    expect(json).not.toMatch(/\\u(0000|d[89a-f])/i);
  });
});

describe("per-user limits", () => {
  it("are 50 accounts, 5,000 new sessions and 20,000 new usage readings a day, and 32 window ids an account", () => {
    expect(SYNC_QUOTAS).toEqual({
      accounts: 50,
      newSessionsPerDay: 5000,
      newUsageReadingsPerDay: 20_000,
      windowsPerAccount: 32,
    });
  });

  it("caps a session's token counts so sums stay inside bigint", () => {
    expect(TOKEN_CAP).toBe(1e12);
    // Even a million capped sessions, all four counts, fit in a signed 64-bit sum.
    expect(BigInt(TOKEN_CAP) * 4n * 1_000_000n).toBeLessThan(2n ** 63n);
  });
});
