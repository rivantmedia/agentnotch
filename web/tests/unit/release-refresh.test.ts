/**
 * POST /api/releases/refresh: only a GitHub OIDC token of this repository's Release workflow on
 * main, in its release environment, meant for this site, clears the cached release list. Keys
 * are made here; nothing reaches GitHub.
 */
import {
  createLocalJWKSet,
  exportJWK,
  generateKeyPair,
  SignJWT,
  type JWK,
  type JWTPayload,
} from "jose";
import { beforeAll, describe, expect, it, vi } from "vitest";

import { TokenCheckUnavailable } from "~/server/auth/token-errors";
import {
  createReleaseRunVerifier,
  GITHUB_ACTIONS_ISSUER,
  isReleaseRun,
} from "~/server/auth/verify-release-run";
import { refreshReleases, releaseRunVerifier } from "~/server/releases-refresh";

const SITE = "https://notch.example.com";
const REPO = "rivantmedia/agentnotch";

/** What GitHub puts in a token for the release job of a run on main. */
const RELEASE_RUN = {
  repository: REPO,
  repository_owner: "rivantmedia",
  ref: "refs/heads/main",
  workflow_ref: `${REPO}/.github/workflows/release.yml@refs/heads/main`,
  job_workflow_ref: `${REPO}/.github/workflows/release.yml@refs/heads/main`,
  environment: "release",
  sub: `repo:${REPO}:environment:release`,
};

describe("isReleaseRun", () => {
  it("accepts the release workflow's job on main, in the release environment", () => {
    expect(isReleaseRun(RELEASE_RUN, REPO)).toBe(true);
    // GitHub compares owner and name without case; its claims spell them one way throughout.
    const upper = "RivantMedia/AgentNotch";
    const workflow = `${upper}/.github/workflows/release.yml@refs/heads/main`;
    expect(
      isReleaseRun(
        {
          ...RELEASE_RUN,
          repository: upper,
          workflow_ref: workflow,
          job_workflow_ref: workflow,
        },
        REPO,
      ),
    ).toBe(true);
  });

  it.each([
    ["another repository", { repository: "someone/agentnotch" }],
    [
      "a fork's workflow",
      {
        workflow_ref: `someone/agentnotch/.github/workflows/release.yml@refs/heads/main`,
      },
    ],
    ["another branch", { ref: "refs/heads/feature" }],
    ["a tag", { ref: "refs/tags/agentnotch-v1.0.0" }],
    ["a pull request", { ref: "refs/pull/7/merge" }],
    [
      "another workflow",
      { workflow_ref: `${REPO}/.github/workflows/fork.yml@refs/heads/main` },
    ],
    [
      "a look-alike workflow",
      {
        workflow_ref: `${REPO}/.github/workflows/release.yml.evil@refs/heads/main`,
      },
    ],
    [
      "the workflow file in other case",
      { workflow_ref: `${REPO}/.github/workflows/Release.yml@refs/heads/main` },
    ],
    [
      "the workflow at another ref",
      {
        workflow_ref: `${REPO}/.github/workflows/release.yml@refs/heads/feature`,
      },
    ],
    [
      "a job running another repository's reusable workflow",
      {
        job_workflow_ref:
          "someone/else/.github/workflows/release.yml@refs/heads/main",
      },
    ],
    ["no job workflow", { job_workflow_ref: undefined }],
    ["no environment", { environment: undefined }],
    ["another environment", { environment: "Production" }],
    ["a non-string repository", { repository: ["rivantmedia/agentnotch"] }],
  ])("refuses %s", (_, change) => {
    expect(isReleaseRun({ ...RELEASE_RUN, ...change }, REPO)).toBe(false);
  });
});

describe("createReleaseRunVerifier", () => {
  let privateKey: CryptoKey;
  let otherKey: CryptoKey;
  let jwk: JWK;

  beforeAll(async () => {
    const pair = await generateKeyPair("RS256");
    privateKey = pair.privateKey;
    jwk = {
      ...(await exportJWK(pair.publicKey)),
      kid: "k1",
      alg: "RS256",
      use: "sig",
    };
    otherKey = (await generateKeyPair("RS256")).privateKey;
  });

  function sign(
    claims: JWTPayload,
    {
      key = privateKey,
      issuer = GITHUB_ACTIONS_ISSUER,
      audience = SITE,
      expires = "5m",
      kid = "k1",
    }: {
      key?: CryptoKey;
      issuer?: string;
      audience?: string;
      expires?: string | number;
      kid?: string;
    } = {},
  ) {
    return new SignJWT(claims)
      .setProtectedHeader({ alg: "RS256", kid })
      .setIssuer(issuer)
      .setAudience(audience)
      .setIssuedAt()
      .setExpirationTime(expires)
      .sign(key);
  }

  function verifier() {
    return createReleaseRunVerifier({
      audience: SITE,
      repo: REPO,
      jwks: createLocalJWKSet({ keys: [jwk] }),
    });
  }

  it("accepts a release run's token", async () => {
    expect(await verifier()(await sign(RELEASE_RUN))).toBe(true);
  });

  it("refuses a token for another site, from another issuer, or expired", async () => {
    const verify = verifier();
    expect(
      await verify(
        await sign(RELEASE_RUN, { audience: "https://evil.example" }),
      ),
    ).toBe(false);
    expect(
      await verify(
        await sign(RELEASE_RUN, {
          issuer: "https://token.actions.example.com",
        }),
      ),
    ).toBe(false);
    const hourAgo = Math.floor(Date.now() / 1000) - 3600;
    expect(await verify(await sign(RELEASE_RUN, { expires: hourAgo }))).toBe(
      false,
    );
  });

  it("refuses a well-signed token of any other run", async () => {
    expect(
      await verifier()(
        await sign({ ...RELEASE_RUN, ref: "refs/heads/feature" }),
      ),
    ).toBe(false);
  });

  it("refuses a token signed with another key, an unknown key id, or garbage", async () => {
    const verify = verifier();
    expect(await verify(await sign(RELEASE_RUN, { key: otherKey }))).toBe(
      false,
    );
    expect(await verify(await sign(RELEASE_RUN, { kid: "unknown" }))).toBe(
      false,
    );
    expect(await verify("not.a.token")).toBe(false);
    const unsigned = `${btoa(JSON.stringify({ alg: "none" }))}.${btoa(JSON.stringify(RELEASE_RUN))}.`;
    expect(await verify(unsigned)).toBe(false);
  });

  it("refuses a token GitHub wouldn't sign: other algorithms, no key id, a critical header", async () => {
    const verify = verifier();
    const hs256 = await new SignJWT(RELEASE_RUN)
      .setProtectedHeader({ alg: "HS256", kid: "k1" })
      .setIssuer(GITHUB_ACTIONS_ISSUER)
      .setAudience(SITE)
      .setIssuedAt()
      .setExpirationTime("5m")
      .sign(new TextEncoder().encode("a-shared-secret-of-enough-length-32b"));
    expect(await verify(hs256)).toBe(false);
    const noKid = await new SignJWT(RELEASE_RUN)
      .setProtectedHeader({ alg: "RS256" })
      .setIssuer(GITHUB_ACTIONS_ISSUER)
      .setAudience(SITE)
      .setIssuedAt()
      .setExpirationTime("5m")
      .sign(privateKey);
    expect(await verify(noKid)).toBe(false);
    // An unknown critical extension is refused as a bad token (401), never a key-set failure
    // (503); the header alone decides, so the signature needn't be real.
    const critical = [
      btoa(
        JSON.stringify({
          alg: "RS256",
          kid: "k1",
          crit: ["x-unknown"],
          "x-unknown": 1,
        }),
      ),
      btoa(JSON.stringify(RELEASE_RUN)),
      "c2lnbmF0dXJl",
    ].join(".");
    expect(await verify(critical)).toBe(false);
  });

  it("needs exp and iat, and allows 30 s of clock skew", async () => {
    const verify = verifier();
    const noExp = await new SignJWT(RELEASE_RUN)
      .setProtectedHeader({ alg: "RS256", kid: "k1" })
      .setIssuer(GITHUB_ACTIONS_ISSUER)
      .setAudience(SITE)
      .setIssuedAt()
      .sign(privateKey);
    expect(await verify(noExp)).toBe(false);
    const noIat = await new SignJWT(RELEASE_RUN)
      .setProtectedHeader({ alg: "RS256", kid: "k1" })
      .setIssuer(GITHUB_ACTIONS_ISSUER)
      .setAudience(SITE)
      .setExpirationTime("5m")
      .sign(privateKey);
    expect(await verify(noIat)).toBe(false);
    const now = Math.floor(Date.now() / 1000);
    expect(await verify(await sign(RELEASE_RUN, { expires: now - 20 }))).toBe(
      true,
    );
    expect(await verify(await sign(RELEASE_RUN, { expires: now - 40 }))).toBe(
      false,
    );
  });

  it("says the check couldn't happen for an unknown key id while the key set can't be refetched", async () => {
    const local = createLocalJWKSet({ keys: [jwk] });
    const coolingDown = Object.assign(
      (...args: Parameters<typeof local>) => local(...args),
      { coolingDown: true },
    );
    const verify = createReleaseRunVerifier({
      audience: SITE,
      repo: REPO,
      jwks: coolingDown,
    });
    await expect(
      verify(await sign(RELEASE_RUN, { kid: "new" })),
    ).rejects.toBeInstanceOf(TokenCheckUnavailable);
  });

  it("says the check couldn't happen when GitHub's key set can't be read", async () => {
    const verify = createReleaseRunVerifier({
      audience: SITE,
      repo: REPO,
      jwks: () => Promise.reject(new Error("network down")),
    });
    await expect(verify(await sign(RELEASE_RUN))).rejects.toBeInstanceOf(
      TokenCheckUnavailable,
    );
  });
});

describe("refreshReleases", () => {
  function post(authorization?: string) {
    return new Request(`${SITE}/api/releases/refresh`, {
      method: "POST",
      headers: authorization ? { authorization } : {},
    });
  }

  it("clears the release list's cache for a release run", async () => {
    const revalidate = vi.fn();
    const verify = vi.fn(async (token: string) => token === "good");
    const response = await refreshReleases(post("Bearer good"), {
      verify,
      revalidate,
    });
    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({ refreshed: true });
    expect(response.headers.get("cache-control")).toBe("no-store");
    expect(revalidate).toHaveBeenCalledExactlyOnceWith("github-releases");
  });

  it("answers 401 and clears nothing without a token, or with one that isn't a release run's", async () => {
    const revalidate = vi.fn();
    const verify = vi.fn(async () => false);
    for (const authorization of [
      undefined,
      "Basic abc",
      "Bearer ",
      "Bearer bad",
    ]) {
      const response = await refreshReleases(post(authorization), {
        verify,
        revalidate,
      });
      expect(response.status).toBe(401);
      const body = (await response.json()) as { error: { code: string } };
      expect(body.error.code).toBe("UNAUTHORIZED");
    }
    expect(revalidate).not.toHaveBeenCalled();
  });

  it("answers 503 when GitHub's keys can't be checked, or the site doesn't know its address", async () => {
    const revalidate = vi.fn();
    const unavailable = await refreshReleases(post("Bearer good"), {
      verify: async () => {
        throw new TokenCheckUnavailable("keys");
      },
      revalidate,
    });
    expect(unavailable.status).toBe(503);
    expect(unavailable.headers.get("retry-after")).toBe("30");
    const unconfigured = await refreshReleases(post("Bearer good"), {
      verify: null,
      revalidate,
    });
    expect(unconfigured.status).toBe(503);
    expect(revalidate).not.toHaveBeenCalled();
  });

  it("lets any other error through (a bug, not a refusal)", async () => {
    await expect(
      refreshReleases(post("Bearer good"), {
        verify: async () => {
          throw new TypeError("bug");
        },
        revalidate: vi.fn(),
      }),
    ).rejects.toBeInstanceOf(TypeError);
  });
});

describe("releaseRunVerifier", () => {
  it("is null without a usable site address", () => {
    expect(releaseRunVerifier(undefined, REPO)).toBeNull();
    expect(releaseRunVerifier("not a url", REPO)).toBeNull();
    // Their origin is "null": an audience any GitHub Actions job could ask for.
    expect(releaseRunVerifier("file:///srv/site", REPO)).toBeNull();
    expect(releaseRunVerifier("javascript:alert(1)", REPO)).toBeNull();
    expect(releaseRunVerifier("data:text/plain,x", REPO)).toBeNull();
  });

  it("keeps one verifier per origin and repository", () => {
    const first = releaseRunVerifier(`${SITE}/`, REPO);
    expect(first).not.toBeNull();
    expect(releaseRunVerifier(`${SITE}/dashboard`, REPO)).toBe(first);
    expect(releaseRunVerifier("https://other.example.com", REPO)).not.toBe(
      first,
    );
    expect(releaseRunVerifier(SITE, "someone/else")).not.toBe(first);
  });
});
