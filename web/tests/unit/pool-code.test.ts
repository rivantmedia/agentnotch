import { describe, expect, it } from "vitest";

import {
  formatPoolCode,
  generatePoolCode,
  isPoolCode,
  normalizePoolCode,
  POOL_CODE_ALPHABET,
  POOL_CODE_EXAMPLE,
  POOL_CODE_LENGTH,
  POOL_CODE_TTL_MS,
} from "~/server/services/pool-code";

describe("pool codes", () => {
  it("use 32 unambiguous Crockford characters", () => {
    expect(POOL_CODE_ALPHABET).toHaveLength(32);
    expect(new Set(POOL_CODE_ALPHABET).size).toBe(32);
    for (const ambiguous of ["I", "L", "O", "U"]) {
      expect(POOL_CODE_ALPHABET).not.toContain(ambiguous);
    }
    expect(POOL_CODE_ALPHABET).toMatch(/^[0-9A-Z]+$/);
  });

  it("are 12 characters (60 bits) drawn from the alphabet", () => {
    expect(POOL_CODE_LENGTH).toBe(12);
    const codes = Array.from({ length: 2000 }, () => generatePoolCode());
    for (const code of codes) {
      expect(code).toHaveLength(12);
      expect(isPoolCode(code)).toBe(true);
      expect(normalizePoolCode(code)).toBe(code);
    }
    // 32^12 possibilities: 2000 draws colliding would mean a broken generator.
    expect(new Set(codes).size).toBe(codes.length);
  });

  it("expire after 7 days", () => {
    expect(POOL_CODE_TTL_MS).toBe(7 * 24 * 60 * 60 * 1000);
  });

  it("map every byte value evenly onto the alphabet", () => {
    // 64 codes of 12 bytes walk every byte value exactly three times.
    const counts = new Map<string, number>();
    for (let start = 0; start < 768; start += POOL_CODE_LENGTH) {
      const code = generatePoolCode((n) =>
        Uint8Array.from({ length: n }, (_, i) => (start + i) % 256),
      );
      for (const char of code) counts.set(char, (counts.get(char) ?? 0) + 1);
    }
    expect(counts.size).toBe(32);
    for (const count of counts.values()) expect(count).toBe(24);
  });

  it("normalize what people type", () => {
    expect(normalizePoolCode("7k3m-9qx2-h4tb")).toBe("7K3M9QX2H4TB");
    expect(normalizePoolCode(" 7K3M 9QX2 H4TB ")).toBe("7K3M9QX2H4TB");
    expect(normalizePoolCode("7k3m_9qx2_h4tb")).toBe("7K3M9QX2H4TB");
    expect(normalizePoolCode("7K3M.9QX2.H4TB")).toBe("7K3M9QX2H4TB");
    // Crockford's decoding: O reads as 0, I and L as 1.
    expect(normalizePoolCode("O0IL-1234-5678")).toBe("001112345678");
    expect(normalizePoolCode("ｏ0il12345678")).toBe("001112345678"); // full-width letters
  });

  it("refuse what can't be a code", () => {
    for (const input of [
      "",
      "7K3M9QX2",
      "7K3M-9QX2", // the old 8-character form
      "7K3M9QX2H4T",
      "7K3M9QX2H4TBB",
      "7K3M9QX2H4TU",
      "7K3M9QX2H4T!",
      "ÅÅÅÅÅÅÅÅÅÅÅÅ",
      "------------",
    ]) {
      expect(normalizePoolCode(input)).toBeNull();
    }
    expect(isPoolCode("7k3m9qx2h4tb")).toBe(false);
    expect(isPoolCode("7K3M-9QX2-H4TB")).toBe(false);
  });

  it("format as three groups of four", () => {
    expect(formatPoolCode("7K3M9QX2H4TB")).toBe("7K3M-9QX2-H4TB");
    expect(normalizePoolCode(formatPoolCode("7K3M9QX2H4TB"))).toBe(
      "7K3M9QX2H4TB",
    );
    expect(normalizePoolCode(POOL_CODE_EXAMPLE)).not.toBeNull();
  });
});
