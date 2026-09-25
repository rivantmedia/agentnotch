/**
 * Pool share codes: 12 characters of Crockford base32, shown as `XXXX-XXXX-XXXX`.
 *
 * The alphabet leaves out I, L, O and U, so a code read aloud or copied by hand can't be
 * misread; when typed back, O counts as 0 and I or L as 1 (Crockford's decoding rules), case and
 * separators don't matter. 32^12 = 2^60 ≈ 1.2 × 10^18 codes, drawn from the platform CSPRNG. A
 * code works for 7 days (contract/README.md, "Pooling"), and failed redemptions are rate-limited
 * per user (pools.ts), so guessing one is hopeless.
 */
export const POOL_CODE_ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
export const POOL_CODE_LENGTH = 12;
/** How long a code can be redeemed after it was issued. */
export const POOL_CODE_TTL_MS = 7 * 24 * 60 * 60 * 1000;

const CODE = /^[0-9A-HJKMNP-TV-Z]{12}$/;

export type RandomBytes = (length: number) => Uint8Array;

const cryptoRandom: RandomBytes = (length) => {
  const bytes = new Uint8Array(length);
  globalThis.crypto.getRandomValues(bytes);
  return bytes;
};

export function generatePoolCode(random: RandomBytes = cryptoRandom): string {
  const bytes = random(POOL_CODE_LENGTH);
  let code = "";
  for (let i = 0; i < POOL_CODE_LENGTH; i++) {
    // 256 is a multiple of 32, so the low five bits are uniform.
    code += POOL_CODE_ALPHABET[(bytes[i] ?? 0) & 31];
  }
  return code;
}

/** The stored form of a typed code, or null when it can't be one. */
export function normalizePoolCode(input: string): string | null {
  const cleaned = input
    .normalize("NFKC")
    .toUpperCase()
    .replace(/[\s\-_.]/g, "")
    .replace(/O/g, "0")
    .replace(/[IL]/g, "1");
  return CODE.test(cleaned) ? cleaned : null;
}

export function isPoolCode(value: string): boolean {
  return CODE.test(value);
}

/** How a code is shown: three groups of four, `7K3M-9QX2-H4TB`. */
export function formatPoolCode(code: string): string {
  return code.match(/.{1,4}/g)?.join("-") ?? code;
}

/** An example in the shown form, for hints and placeholders. */
export const POOL_CODE_EXAMPLE = "7K3M-9QX2-H4TB";
