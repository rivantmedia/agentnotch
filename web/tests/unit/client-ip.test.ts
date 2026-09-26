/**
 * How the per-IP limits know a client (src/server/client-ip.ts): the last X-Forwarded-For hop
 * (never one a client wrote), else X-Real-IP; one spelling per address; an IPv6 client by its
 * /64; and only ever a hash of it.
 */
import { createHash, createHmac } from "node:crypto";

import { describe, expect, it } from "vitest";

import {
  clientIp,
  clientIpKey,
  ipKey,
  limitNetwork,
  normalizeIp,
} from "~/server/client-ip";

const headers = (entries: Record<string, string>) => new Headers(entries);

describe("clientIp", () => {
  it("takes the proxy's X-Forwarded-For hop over an X-Real-IP a client may have sent", () => {
    // Load balancers append to X-Forwarded-For but can pass a client's X-Real-IP through.
    expect(
      clientIp(
        headers({
          "x-forwarded-for": "6.6.6.6, 203.0.113.7",
          "x-real-ip": "198.51.100.1",
        }),
      ),
    ).toBe("203.0.113.7");
    // Without X-Forwarded-For, X-Real-IP is what there is.
    expect(clientIp(headers({ "x-real-ip": "198.51.100.1" }))).toBe(
      "198.51.100.1",
    );
    expect(
      clientIp(
        headers({
          "x-forwarded-for": "203.0.113.7, unknown",
          "x-real-ip": "198.51.100.1",
        }),
      ),
    ).toBe("198.51.100.1");
  });

  it("else takes the last X-Forwarded-For hop, never one the client wrote", () => {
    // A proxy appends the address it saw: whatever came before is the client's word.
    expect(
      clientIp(headers({ "x-forwarded-for": "6.6.6.6, 203.0.113.7" })),
    ).toBe("203.0.113.7");
    expect(
      clientIp(
        headers({ "x-forwarded-for": " 10.0.0.1 ,10.0.0.2, 203.0.113.7 " }),
      ),
    ).toBe("203.0.113.7");
    expect(clientIp(headers({ "x-forwarded-for": "203.0.113.7" }))).toBe(
      "203.0.113.7",
    );
  });

  it("keeps the limit when a client sends garbage in front of the proxy's hop", () => {
    expect(clientIp(headers({ "x-forwarded-for": "x, 203.0.113.7" }))).toBe(
      "203.0.113.7",
    );
    expect(
      clientIp(headers({ "x-forwarded-for": "unknown,, 203.0.113.7" })),
    ).toBe("203.0.113.7");
  });

  it("falls through a header that holds no address to the next", () => {
    expect(
      clientIp(
        headers({
          "x-real-ip": "not an ip",
          "x-forwarded-for": "6.6.6.6, 203.0.113.7",
        }),
      ),
    ).toBe("203.0.113.7");
    expect(
      clientIp(headers({ "x-real-ip": "", "x-forwarded-for": "203.0.113.7" })),
    ).toBe("203.0.113.7");
  });

  it("is null without an address", () => {
    expect(clientIp(headers({}))).toBeNull();
    expect(clientIp(headers({ "x-forwarded-for": "" }))).toBeNull();
    expect(clientIp(headers({ "x-real-ip": "not an ip" }))).toBeNull();
    // The last hop is the proxy's; an earlier one is never used in its place.
    expect(
      clientIp(headers({ "x-forwarded-for": "203.0.113.7, unknown" })),
    ).toBeNull();
  });
});

describe("normalizeIp", () => {
  it("spells each address one way", () => {
    expect(normalizeIp("203.0.113.7:5123")).toBe("203.0.113.7");
    expect(normalizeIp("[2001:DB8::1]:443")).toBe("2001:db8::1");
    expect(normalizeIp("2001:DB8::1")).toBe("2001:db8::1");
    expect(normalizeIp("::ffff:203.0.113.7")).toBe("203.0.113.7");
    expect(normalizeIp("::ffff:cb00:7107")).toBe("203.0.113.7");
    expect(normalizeIp("::1")).toBe("::1");
    expect(normalizeIp("::")).toBe("::");
  });

  it("writes IPv6 in its canonical form (RFC 5952)", () => {
    for (const spelling of [
      "2001:0db8:0001:0002:0000:0000:0000:0001",
      "2001:0db8:0001:0002::1",
      "2001:db8:1:2:0:0:0:1",
      "2001:DB8:1:2::0:1",
      "[2001:db8:1:2::1]",
      "2001:db8:1:2::1%en0",
    ]) {
      expect(normalizeIp(spelling), spelling).toBe("2001:db8:1:2::1");
    }
    // The first of two equally long runs of zeros is the one shortened; a lone zero isn't.
    expect(normalizeIp("2001:db8:0:0:1:0:0:1")).toBe("2001:db8::1:0:0:1");
    expect(normalizeIp("2001:db8:0:1:1:1:1:1")).toBe("2001:db8:0:1:1:1:1:1");
    expect(normalizeIp("1:2:3:4:5:6:1.2.3.4")).toBe("1:2:3:4:5:6:102:304");
  });

  it("refuses what isn't an address", () => {
    for (const value of [
      undefined,
      "",
      "999.1.1.1",
      "example.com",
      "1.2.3.4.5",
      "[nope]",
      "1.2.3.4%en0",
    ]) {
      expect(normalizeIp(value), String(value)).toBeNull();
    }
  });
});

describe("limitNetwork", () => {
  it("is an IPv4 address itself, and an IPv6 address's /64", () => {
    expect(limitNetwork("203.0.113.7")).toBe("203.0.113.7");
    expect(limitNetwork("2001:db8:1:2::1")).toBe("2001:db8:1:2::/64");
    expect(limitNetwork("2001:db8:1:2:ffff:ffff:ffff:9")).toBe(
      "2001:db8:1:2::/64",
    );
    expect(limitNetwork("2001:db8:0:0:1::1")).toBe("2001:db8::/64");
    expect(limitNetwork("::1")).toBe("::/64");
  });
});

describe("clientIpKey", () => {
  const request = headers({ "x-forwarded-for": "203.0.113.7" });

  it("is a SHA-256 of the address, never the address", () => {
    const key = clientIpKey(request, undefined);
    expect(key).toBe(createHash("sha256").update("203.0.113.7").digest("hex"));
    expect(key).toMatch(/^[0-9a-f]{64}$/);
    expect(key).not.toContain("203.0.113.7");
  });

  it("is an HMAC keyed with the pepper when there is one", () => {
    const pepper = "a-long-random-pepper-value";
    expect(clientIpKey(request, pepper)).toBe(
      createHmac("sha256", pepper).update("203.0.113.7").digest("hex"),
    );
    expect(clientIpKey(request, pepper)).not.toBe(
      clientIpKey(request, undefined),
    );
  });

  it("is the same for every spelling of one address, and null without one", () => {
    expect(
      clientIpKey(headers({ "x-forwarded-for": "203.0.113.7:9000" }), "p"),
    ).toBe(ipKey("203.0.113.7", "p"));
    expect(clientIpKey(headers({}), "p")).toBeNull();
  });

  it("is one key for a whole IPv6 /64, whatever address or spelling it sends from", () => {
    const key = ipKey("2001:db8:1:2::1", "p");
    expect(ipKey("2001:db8:1:2:ffff::9", "p")).toBe(key);
    expect(ipKey("2001:0db8:0001:0002::1", "p")).toBe(key);
    expect(
      clientIpKey(headers({ "x-real-ip": "2001:DB8:1:2:0:0:0:abcd" }), "p"),
    ).toBe(key);
    expect(
      clientIpKey(
        headers({ "x-forwarded-for": "6.6.6.6, [2001:db8:1:2::77]:443" }),
        "p",
      ),
    ).toBe(key);
    // The next /64 over is someone else.
    expect(ipKey("2001:db8:1:3::1", "p")).not.toBe(key);
    expect(key).toBe(
      createHmac("sha256", "p").update("2001:db8:1:2::/64").digest("hex"),
    );
  });
});
