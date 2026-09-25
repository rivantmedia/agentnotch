import { describe, expect, it } from "vitest";

import { requestOrigin, siteAddressFrom } from "~/lib/site-address";

const headers = (entries: Record<string, string>) => new Headers(entries);

describe("siteAddressFrom", () => {
  it("prefers the configured site URL, without a trailing slash", () => {
    const h = headers({ host: "evil.example" });
    expect(siteAddressFrom("https://notch.example.com/", h)).toBe(
      "https://notch.example.com",
    );
    expect(siteAddressFrom("https://example.com/agentnotch//", h)).toBe(
      "https://example.com/agentnotch",
    );
    expect(siteAddressFrom("http://localhost:3000", h)).toBe(
      "http://localhost:3000",
    );
  });

  it("drops a configured value's query and fragment", () => {
    expect(
      siteAddressFrom("https://notch.example.com/?x=1#top", headers({})),
    ).toBe("https://notch.example.com");
  });

  it("falls back to the request's address when nothing usable is configured", () => {
    expect(
      siteAddressFrom(undefined, headers({ host: "notch.example.com" })),
    ).toBe("https://notch.example.com");
    expect(siteAddressFrom("", headers({ host: "localhost:3000" }))).toBe(
      "http://localhost:3000",
    );
    expect(
      siteAddressFrom("not a url", headers({ host: "127.0.0.1:3000" })),
    ).toBe("http://127.0.0.1:3000");
    expect(
      siteAddressFrom(
        "ftp://files.example.com",
        headers({ host: "a.example" }),
      ),
    ).toBe("https://a.example");
  });

  it("uses the first forwarded host and protocol behind a proxy", () => {
    expect(
      siteAddressFrom(
        null,
        headers({
          host: "internal:8080",
          "x-forwarded-host": "Notch.Example.com, proxy.local",
          "x-forwarded-proto": "https, http",
        }),
      ),
    ).toBe("https://notch.example.com");
    expect(
      siteAddressFrom(
        null,
        headers({ host: "notch.example.com", "x-forwarded-proto": "gopher" }),
      ),
    ).toBe("https://notch.example.com");
  });

  it("refuses a host header that isn't a host", () => {
    expect(
      siteAddressFrom(null, headers({ host: "evil.example/path?x" })),
    ).toBeNull();
    expect(
      siteAddressFrom(null, headers({ host: "user@evil.example" })),
    ).toBeNull();
    expect(siteAddressFrom(null, headers({}))).toBeNull();
  });
});

describe("requestOrigin", () => {
  const listen = "http://localhost:3000";

  it("is the host the browser asked for, not the address Next.js listens on", () => {
    expect(requestOrigin(headers({ host: "127.0.0.1:3100" }), listen)).toBe(
      "http://127.0.0.1:3100",
    );
    expect(
      requestOrigin(
        headers({
          host: "localhost:3000",
          "x-forwarded-host": "Notch.Example.com",
          "x-forwarded-proto": "https",
        }),
        listen,
      ),
    ).toBe("https://notch.example.com");
  });

  it("keeps the request's own scheme when no proxy names one", () => {
    expect(
      requestOrigin(
        headers({ host: "notch.example.com" }),
        "https://x.vercel.app",
      ),
    ).toBe("https://notch.example.com");
    expect(
      requestOrigin(
        headers({ host: "notch.example.com", "x-forwarded-proto": "ftp" }),
        listen,
      ),
    ).toBe("http://notch.example.com");
  });

  it("falls back to the request's origin when the headers hold no plain host", () => {
    expect(requestOrigin(headers({}), listen)).toBe(listen);
    expect(
      requestOrigin(headers({ host: "evil.example/login?x" }), listen),
    ).toBe(listen);
    expect(
      requestOrigin(
        headers({ "x-forwarded-host": "a b", host: "ok.example" }),
        listen,
      ),
    ).toBe(listen);
  });
});
