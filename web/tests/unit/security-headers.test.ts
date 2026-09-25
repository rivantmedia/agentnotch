/**
 * Every response carries the anti-framing headers (so /pools' buttons can't be clickjacked) and
 * the other basics, through next.config.js.
 */
import { describe, expect, it } from "vitest";

import config from "../../next.config.js";
import {
  SECURITY_HEADERS,
  SECURITY_HEADERS_SOURCE,
} from "../../security-headers.js";

describe("security headers", () => {
  it("forbid framing, content sniffing and full referrers", () => {
    expect(
      Object.fromEntries(SECURITY_HEADERS.map((h) => [h.key, h.value])),
    ).toEqual({
      "X-Frame-Options": "DENY",
      "Content-Security-Policy": "frame-ancestors 'none'",
      "X-Content-Type-Options": "nosniff",
      "Referrer-Policy": "strict-origin-when-cross-origin",
    });
  });

  it("apply to every path through next.config.js", async () => {
    expect(SECURITY_HEADERS_SOURCE).toBe("/:path*");
    const rules = await config.headers!();
    expect(rules).toEqual([
      { source: "/:path*", headers: [...SECURITY_HEADERS] },
    ]);
    // Next.js matches `/:path*` against the root too.
    const matcher = new RegExp(
      `^${rules[0]!.source.replace("/:path*", "(?:/.*)?")}$`,
    );
    for (const path of ["/", "/pools", "/api/app/v1/sync", "/accounts/x/y"]) {
      expect(matcher.test(path), path).toBe(true);
    }
  });
});
