/**
 * The site's own address (src/lib/site-url.js, which src/env.js reads): NEXT_PUBLIC_SITE_URL when
 * it is set, else `https://` and the Vercel project's production domain, else unset.
 */
import { afterEach, describe, expect, it, vi } from "vitest";

import { siteUrlFrom } from "~/lib/site-url";
import { releaseRunVerifier } from "~/server/releases-refresh";

describe("siteUrlFrom", () => {
  it("is NEXT_PUBLIC_SITE_URL when it is set, before Vercel's domain", () => {
    expect(
      siteUrlFrom("https://notch.example.com", "agentnotch.example.com"),
    ).toBe("https://notch.example.com");
    expect(siteUrlFrom("http://localhost:3000", undefined, undefined)).toBe(
      "http://localhost:3000",
    );
  });

  it("keeps an override's path, without trailing slashes or padding", () => {
    expect(siteUrlFrom("  https://notch.example.com/  ")).toBe(
      "https://notch.example.com",
    );
    expect(siteUrlFrom("https://example.com/agentnotch//")).toBe(
      "https://example.com/agentnotch",
    );
  });

  it("passes a wrong override on, so src/env.js refuses it instead of replacing it", () => {
    expect(siteUrlFrom("not a url", "agentnotch.example.com")).toBe(
      "not a url",
    );
    // Slashes alone aren't blank: trimming them to "" would make the value count as unset.
    expect(siteUrlFrom("//", "agentnotch.example.com")).toBe("//");
  });

  it("is https:// and Vercel's production domain when there is no override", () => {
    expect(siteUrlFrom(undefined, "agentnotch.example.com")).toBe(
      "https://agentnotch.example.com",
    );
    // Only the Next.js copy (NEXT_PUBLIC_VERCEL_PROJECT_PRODUCTION_URL), as browser code sees it.
    expect(siteUrlFrom(undefined, undefined, "agentnotch.example.com")).toBe(
      "https://agentnotch.example.com",
    );
    expect(siteUrlFrom(undefined, "my-site.vercel.app", "other.example")).toBe(
      "https://my-site.vercel.app",
    );
  });

  it("treats blank values as unset", () => {
    expect(siteUrlFrom("", "agentnotch.example.com")).toBe(
      "https://agentnotch.example.com",
    );
    expect(siteUrlFrom("   ", "  ", "agentnotch.example.com")).toBe(
      "https://agentnotch.example.com",
    );
    expect(siteUrlFrom("", "", "")).toBeUndefined();
    expect(siteUrlFrom("  \n")).toBeUndefined();
  });

  it("is unset with neither (local development without .env)", () => {
    expect(siteUrlFrom(undefined)).toBeUndefined();
    expect(siteUrlFrom(undefined, undefined, undefined)).toBeUndefined();
  });

  it("reads the domain as an origin: lowercase, no trailing slash or path, no default port", () => {
    expect(siteUrlFrom(undefined, "AgentNotch.Example.com/")).toBe(
      "https://agentnotch.example.com",
    );
    expect(siteUrlFrom(undefined, "agentnotch.example.com/some/path?x=1")).toBe(
      "https://agentnotch.example.com",
    );
    expect(siteUrlFrom(undefined, "agentnotch.example.com:443")).toBe(
      "https://agentnotch.example.com",
    );
    expect(siteUrlFrom(undefined, "agentnotch.example.com:8443")).toBe(
      "https://agentnotch.example.com:8443",
    );
    // Vercel leaves the scheme out, but one given is kept when it is http(s).
    expect(siteUrlFrom(undefined, "https://agentnotch.example.com/")).toBe(
      "https://agentnotch.example.com",
    );
    expect(siteUrlFrom(undefined, "http://localhost:3000")).toBe(
      "http://localhost:3000",
    );
  });

  it("skips a production domain that isn't one, for the next", () => {
    for (const bad of [
      "not a domain",
      "ftp://agentnotch.example.com",
      "file:///srv/site",
      "javascript:alert(1)",
      "user:pass@agentnotch.example.com",
      "https://",
    ]) {
      expect(siteUrlFrom(undefined, bad)).toBeUndefined();
      expect(siteUrlFrom(undefined, bad, "agentnotch.example.com")).toBe(
        "https://agentnotch.example.com",
      );
    }
  });

  it("gives the release refresh the origin of the address as its tokens' audience", () => {
    const REPO = "rivantmedia/agentnotch";
    // One verifier per audience and repository: the same instance means the same audience.
    const fromVercel = releaseRunVerifier(
      siteUrlFrom(undefined, "AgentNotch.example.com/"),
      REPO,
    );
    expect(fromVercel).not.toBeNull();
    expect(releaseRunVerifier("https://agentnotch.example.com", REPO)).toBe(
      fromVercel,
    );
    const fromOverride = releaseRunVerifier(
      siteUrlFrom("https://example.com/agentnotch/", "agentnotch.example.com"),
      REPO,
    );
    expect(releaseRunVerifier("https://example.com", REPO)).toBe(fromOverride);
    expect(releaseRunVerifier(siteUrlFrom(undefined), REPO)).toBeNull();
  });
});

describe("src/env.js", () => {
  // Made up: the check only needs well-formed values.
  const REQUIRED = {
    DATABASE_URL: "postgresql://user:pass@127.0.0.1:6543/postgres",
    DIRECT_URL: "postgresql://user:pass@127.0.0.1:5432/postgres",
    NEXT_PUBLIC_SUPABASE_URL: "https://project.supabase.example",
    NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY: "sb_publishable_check",
  };
  const UNSET = {
    SUPABASE_JWT_SECRET: undefined,
    RATE_LIMIT_PEPPER: undefined,
    RELEASES_REPO: undefined,
    GITHUB_RELEASES_TOKEN: undefined,
    NEXT_PUBLIC_SITE_URL: undefined,
    VERCEL_PROJECT_PRODUCTION_URL: undefined,
    NEXT_PUBLIC_VERCEL_PROJECT_PRODUCTION_URL: undefined,
  };

  /** src/env.js as a fresh import sees `vars`, validated (vitest.config.ts skips that otherwise). */
  async function loadEnv(vars: Record<string, string | undefined>) {
    for (const [name, value] of Object.entries({
      ...REQUIRED,
      ...UNSET,
      ...vars,
      SKIP_ENV_VALIDATION: "",
    })) {
      vi.stubEnv(name, value);
    }
    vi.resetModules();
    const { env } = await import("~/env");
    return env;
  }

  afterEach(() => {
    vi.unstubAllEnvs();
    vi.restoreAllMocks();
    vi.resetModules();
  });

  it("takes Vercel's production domain when NEXT_PUBLIC_SITE_URL isn't set", async () => {
    const env = await loadEnv({
      VERCEL_PROJECT_PRODUCTION_URL: "agentnotch.example.com",
      NEXT_PUBLIC_VERCEL_PROJECT_PRODUCTION_URL: "agentnotch.example.com",
    });
    expect(env.NEXT_PUBLIC_SITE_URL).toBe("https://agentnotch.example.com");
  });

  it("takes either of Vercel's variables alone", async () => {
    expect(
      (await loadEnv({ VERCEL_PROJECT_PRODUCTION_URL: "a.example.com" }))
        .NEXT_PUBLIC_SITE_URL,
    ).toBe("https://a.example.com");
    expect(
      (
        await loadEnv({
          NEXT_PUBLIC_VERCEL_PROJECT_PRODUCTION_URL: "b.example.com",
        })
      ).NEXT_PUBLIC_SITE_URL,
    ).toBe("https://b.example.com");
  });

  it("lets NEXT_PUBLIC_SITE_URL override it, and ignores a blank one", async () => {
    expect(
      (
        await loadEnv({
          NEXT_PUBLIC_SITE_URL: "http://localhost:3000/",
          VERCEL_PROJECT_PRODUCTION_URL: "agentnotch.example.com",
        })
      ).NEXT_PUBLIC_SITE_URL,
    ).toBe("http://localhost:3000");
    expect(
      (
        await loadEnv({
          NEXT_PUBLIC_SITE_URL: "",
          VERCEL_PROJECT_PRODUCTION_URL: "agentnotch.example.com",
        })
      ).NEXT_PUBLIC_SITE_URL,
    ).toBe("https://agentnotch.example.com");
  });

  it("passes with neither, leaving the address unset", async () => {
    expect((await loadEnv({})).NEXT_PUBLIC_SITE_URL).toBeUndefined();
  });

  it("refuses a NEXT_PUBLIC_SITE_URL that isn't a URL, even on Vercel", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    await expect(
      loadEnv({
        NEXT_PUBLIC_SITE_URL: "agentnotch.example.com",
        VERCEL_PROJECT_PRODUCTION_URL: "agentnotch.example.com",
      }),
    ).rejects.toThrow(/Invalid environment variables/);
  });
});
