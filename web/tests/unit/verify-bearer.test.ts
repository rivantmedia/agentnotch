/**
 * Bearer verification against a locally generated key set (no network): signature, issuer,
 * audience, expiry and algorithm all have to hold; HS256 only with a configured secret.
 */
import {
  createLocalJWKSet,
  createRemoteJWKSet,
  customFetch,
  errors,
  exportJWK,
  generateKeyPair,
  SignJWT,
  type CryptoKey,
  type JWK,
  type JWTPayload,
} from "jose";
import { beforeAll, describe, expect, it } from "vitest";

import { isAcceptedSignIn } from "~/lib/auth-claims";
import { resolveIdentity } from "~/server/auth/resolve-identity";
import { TokenCheckUnavailable } from "~/server/auth/token-errors";
import {
  bearerToken,
  createBearerVerifier,
  identityFromClaims,
  supabaseIssuer,
  type BearerVerifier,
} from "~/server/auth/verify-bearer";

const SUPABASE_URL = "https://abcdefghijklmnop.supabase.co";
const ISSUER = `${SUPABASE_URL}/auth/v1`;
const SECRET = "legacy-jwt-secret-of-at-least-thirty-two-characters";
const USER_ID = "5b0c1d2e-3f40-4a5b-8c6d-7e8f9a0b1c2d";

let es256: { privateKey: CryptoKey; publicJwk: JWK };
let rs256: { privateKey: CryptoKey; publicJwk: JWK };
let stranger: { privateKey: CryptoKey };
let verify: BearerVerifier;
let verifyWithSecret: BearerVerifier;

async function keyPair(alg: "ES256" | "RS256", kid: string) {
  const { privateKey, publicKey } = await generateKeyPair(alg, {
    extractable: true,
  });
  const publicJwk = { ...(await exportJWK(publicKey)), kid, alg, use: "sig" };
  return { privateKey, publicJwk };
}

const now = () => Math.floor(Date.now() / 1000);

function claims(overrides: JWTPayload = {}): JWTPayload {
  return {
    sub: USER_ID,
    aud: "authenticated",
    iss: ISSUER,
    role: "authenticated",
    email: "me@example.com",
    // Supabase writes app_metadata; users can't.
    app_metadata: { provider: "google", providers: ["google"] },
    user_metadata: { full_name: "Me Example", name: "me" },
    is_anonymous: false,
    iat: now() - 10,
    exp: now() + 3600,
    ...overrides,
  };
}

async function sign(
  payload: JWTPayload,
  key: CryptoKey | Uint8Array,
  header: { alg: string; kid?: string },
): Promise<string> {
  return new SignJWT(payload).setProtectedHeader(header).sign(key);
}

const signEs256 = (payload: JWTPayload = claims(), kid = "es-key") =>
  sign(payload, es256.privateKey, { alg: "ES256", kid });
const signHs256 = (payload: JWTPayload = claims(), secret = SECRET) =>
  sign(payload, new TextEncoder().encode(secret), { alg: "HS256" });

beforeAll(async () => {
  es256 = await keyPair("ES256", "es-key");
  rs256 = await keyPair("RS256", "rs-key");
  stranger = await keyPair("ES256", "es-key"); // same kid, different key
  const jwks = createLocalJWKSet({ keys: [es256.publicJwk, rs256.publicJwk] });
  verify = createBearerVerifier({ supabaseUrl: SUPABASE_URL, jwks });
  verifyWithSecret = createBearerVerifier({
    supabaseUrl: `${SUPABASE_URL}/`,
    jwks,
    jwtSecret: SECRET,
  });
});

describe("createBearerVerifier", () => {
  it("accepts a valid ES256 token and reads the identity", async () => {
    expect(await verify(await signEs256())).toEqual({
      id: USER_ID,
      email: "me@example.com",
      name: "Me Example",
    });
  });

  it("accepts a valid RS256 token from the key set", async () => {
    const token = await sign(claims(), rs256.privateKey, {
      alg: "RS256",
      kid: "rs-key",
    });
    expect((await verify(token))?.id).toBe(USER_ID);
  });

  it("rejects the wrong issuer", async () => {
    for (const iss of [
      "https://other.supabase.co/auth/v1",
      SUPABASE_URL,
      `${ISSUER}/`,
      undefined,
    ]) {
      expect(await verify(await signEs256(claims({ iss })))).toBeNull();
    }
  });

  it("rejects the wrong audience", async () => {
    for (const aud of ["anon", "service_role", ["other"], undefined]) {
      expect(await verify(await signEs256(claims({ aud })))).toBeNull();
    }
  });

  it("rejects expired tokens, beyond a short clock tolerance", async () => {
    expect(
      await verify(await signEs256(claims({ exp: now() - 120 }))),
    ).toBeNull();
    expect(
      await verify(await signEs256(claims({ exp: now() - 5 }))),
    ).not.toBeNull();
    expect(
      await verify(await signEs256(claims({ exp: undefined }))),
    ).toBeNull();
  });

  it("rejects tokens not valid yet", async () => {
    expect(
      await verify(await signEs256(claims({ nbf: now() + 600 }))),
    ).toBeNull();
  });

  it("rejects a signature from a key outside the set, even with a known kid", async () => {
    const token = await sign(claims(), stranger.privateKey, {
      alg: "ES256",
      kid: "es-key",
    });
    expect(await verify(token)).toBeNull();
    expect(await verify(await signEs256(claims(), "unknown-kid"))).toBeNull();
  });

  it("rejects a tampered payload", async () => {
    const [header, , signature] = (await signEs256()).split(".");
    const forged = Buffer.from(
      JSON.stringify(claims({ sub: "someone-else" })),
    ).toString("base64url");
    expect(await verify(`${header}.${forged}.${signature}`)).toBeNull();
  });

  it("rejects tokens without a subject, and anonymous sign-ins", async () => {
    expect(
      await verify(await signEs256(claims({ sub: undefined }))),
    ).toBeNull();
    expect(await verify(await signEs256(claims({ sub: "" })))).toBeNull();
    expect(
      await verify(await signEs256(claims({ is_anonymous: true }))),
    ).toBeNull();
  });

  it("rejects unsigned and malformed tokens", async () => {
    const header = Buffer.from(
      JSON.stringify({ alg: "none", typ: "JWT" }),
    ).toString("base64url");
    const payload = Buffer.from(JSON.stringify(claims())).toString("base64url");
    expect(await verify(`${header}.${payload}.`)).toBeNull();
    expect(await verifyWithSecret(`${header}.${payload}.`)).toBeNull();
    for (const token of ["", "abc", "a.b.c", "....", "Bearer x"]) {
      expect(await verify(token)).toBeNull();
    }
  });

  it("accepts HS256 only when the legacy secret is configured", async () => {
    const token = await signHs256();
    expect(await verify(token)).toBeNull();
    expect((await verifyWithSecret(token))?.id).toBe(USER_ID);
    expect(
      await verifyWithSecret(await signHs256(claims(), `${SECRET}-wrong`)),
    ).toBeNull();
    expect(
      await verifyWithSecret(await signHs256(claims({ aud: "anon" }))),
    ).toBeNull();
    expect(
      await verifyWithSecret(await signHs256(claims({ exp: now() - 120 }))),
    ).toBeNull();
    expect(
      await verifyWithSecret(
        await signHs256(claims({ iss: "https://evil.example/auth/v1" })),
      ),
    ).toBeNull();
  });

  it("still verifies asymmetric tokens when a secret is configured", async () => {
    expect((await verifyWithSecret(await signEs256()))?.id).toBe(USER_ID);
  });

  it("does not accept the public key as an HMAC secret", async () => {
    // Algorithm confusion: an HS256 token "signed" with the published key.
    const publicKeyBytes = new TextEncoder().encode(
      JSON.stringify(es256.publicJwk),
    );
    const token = await sign(claims(), publicKeyBytes, {
      alg: "HS256",
      kid: "es-key",
    });
    expect(await verify(token)).toBeNull();
    expect(await verifyWithSecret(token)).toBeNull();
  });
});

describe("sign-in providers", () => {
  it("accepts Google sign-ins only", async () => {
    expect(await verify(await signEs256())).not.toBeNull();
    for (const app_metadata of [
      { provider: "email", providers: ["email"] },
      { provider: "github", providers: ["github"] },
      { provider: "phone" },
      {},
      undefined,
      "google",
      ["google"],
    ]) {
      expect(
        await verify(await signEs256(claims({ app_metadata }))),
        JSON.stringify(app_metadata),
      ).toBeNull();
    }
    // Google linked to an account first made another way still counts.
    expect(
      await verify(
        await signEs256(
          claims({
            app_metadata: { provider: "email", providers: ["email", "google"] },
          }),
        ),
      ),
    ).not.toBeNull();
  });

  it("never takes the provider from user_metadata, which users can edit", () => {
    const forged = claims({
      app_metadata: { provider: "email", providers: ["email"] },
      user_metadata: { provider: "google", providers: ["google"] },
    });
    expect(isAcceptedSignIn(forged)).toBe(false);
    expect(identityFromClaims(forged)).toBeNull();
  });

  it("applies to cookie sessions too (the same predicate)", () => {
    expect(isAcceptedSignIn(null)).toBe(false);
    expect(isAcceptedSignIn(undefined)).toBe(false);
    expect(isAcceptedSignIn(claims())).toBe(true);
    expect(isAcceptedSignIn(claims({ is_anonymous: true }))).toBe(false);
    expect(isAcceptedSignIn(claims({ sub: "" }))).toBe(false);
  });
});

describe("bad tokens versus a check that couldn't happen", () => {
  const JWKS_URL = new URL(`${ISSUER}/.well-known/jwks.json`);

  /** The real remote key set, fetching through `fetchImpl` instead of the network. */
  function remote(
    fetchImpl: (url: string, init: RequestInit) => Promise<Response>,
    options: { cooldownDuration?: number; timeoutDuration?: number } = {},
  ) {
    return createRemoteJWKSet(JWKS_URL, {
      ...options,
      [customFetch]: fetchImpl,
    });
  }

  const serving = (keys: JWK[]) => async () => Response.json({ keys });

  it("a key set that fails to load is not the token's fault", async () => {
    const token = await signEs256();
    const failures: Array<
      (url: string, init: RequestInit) => Promise<Response>
    > = [
      async () => {
        throw new TypeError("fetch failed"); // DNS, TLS, connection refused
      },
      async () => new Response("upstream error", { status: 503 }),
      async () => new Response("<html>", { status: 200 }),
      // Never answers: jose gives up after its timeout.
      (_url, init) =>
        new Promise<Response>((_resolve, reject) => {
          init.signal?.addEventListener("abort", () =>
            reject(init.signal!.reason as Error),
          );
        }),
    ];
    for (const fetchImpl of failures) {
      const verifier = createBearerVerifier({
        supabaseUrl: SUPABASE_URL,
        jwks: remote(fetchImpl, { timeoutDuration: 50 }),
      });
      await expect(verifier(token)).rejects.toBeInstanceOf(
        TokenCheckUnavailable,
      );
    }
  });

  it("an unknown key id is a bad token after a fresh look, and undecided during the cooldown", async () => {
    let served: JWK[] = [es256.publicJwk];
    let fetches = 0;
    const fetchImpl = async () => {
      fetches++;
      return Response.json({ keys: served });
    };

    // Within jose's cooldown the key set isn't fetched again, so a new key id can't be judged.
    const cooling = createBearerVerifier({
      supabaseUrl: SUPABASE_URL,
      jwks: remote(fetchImpl, { cooldownDuration: 60_000 }),
    });
    expect((await cooling(await signEs256()))?.id).toBe(USER_ID);
    expect(fetches).toBe(1);
    const rotated = await signEs256(claims(), "new-key");
    await expect(cooling(rotated)).rejects.toBeInstanceOf(
      TokenCheckUnavailable,
    );
    expect(fetches).toBe(1);

    // Without a cooldown jose refetches on a miss; still missing means not this project's key.
    fetches = 0;
    const fresh = createBearerVerifier({
      supabaseUrl: SUPABASE_URL,
      jwks: remote(fetchImpl, { cooldownDuration: 0 }),
    });
    expect((await fresh(await signEs256()))?.id).toBe(USER_ID);
    expect(await fresh(rotated)).toBeNull();
    expect(fetches).toBe(2);

    // Once the new key is published, the refetch finds it.
    served = [es256.publicJwk, { ...es256.publicJwk, kid: "new-key" }];
    expect((await fresh(rotated))?.id).toBe(USER_ID);
  });

  it("a token that is bad stays a 401 whatever the key set does", async () => {
    const verifier = createBearerVerifier({
      supabaseUrl: SUPABASE_URL,
      jwks: remote(serving([es256.publicJwk])),
    });
    const expired = await signEs256(claims({ exp: now() - 120 }));
    const forged = await sign(claims(), stranger.privateKey, {
      alg: "ES256",
      kid: "es-key",
    });
    const wrongAudience = await signEs256(claims({ aud: "anon" }));
    for (const token of [expired, forged, wrongAudience, "a.b.c", "nope"]) {
      expect(await verifier(token)).toBeNull();
    }
  });

  it("classifies what the key source throws", async () => {
    const throwing = (error: unknown, coolingDown = false) =>
      Object.assign(
        async () => {
          throw error;
        },
        { coolingDown },
      );
    const token = await signEs256();
    const check = (error: unknown, coolingDown?: boolean) =>
      createBearerVerifier({
        supabaseUrl: SUPABASE_URL,
        jwks: throwing(error, coolingDown),
      })(token);

    await expect(check(new errors.JWKSTimeout())).rejects.toBeInstanceOf(
      TokenCheckUnavailable,
    );
    await expect(check(new errors.JWKSInvalid())).rejects.toBeInstanceOf(
      TokenCheckUnavailable,
    );
    await expect(
      check(new errors.JWKSNoMatchingKey(), true),
    ).rejects.toBeInstanceOf(TokenCheckUnavailable);
    expect(await check(new errors.JWKSNoMatchingKey(), false)).toBeNull();
    expect(await check(new errors.JWSSignatureVerificationFailed())).toBeNull();
  });
});

describe("identityFromClaims", () => {
  it("prefers full_name, then name, and trims", () => {
    expect(
      identityFromClaims(claims({ user_metadata: { name: "  Me  " } }))?.name,
    ).toBe("Me");
    expect(
      identityFromClaims(claims({ user_metadata: { full_name: " " } }))?.name,
    ).toBeNull();
    expect(
      identityFromClaims(claims({ user_metadata: "nope" }))?.name,
    ).toBeNull();
    expect(identityFromClaims(claims({ email: undefined }))?.email).toBeNull();
  });

  it("refuses absurd subjects", () => {
    expect(identityFromClaims(claims({ sub: "x".repeat(129) }))).toBeNull();
    expect(identityFromClaims({ sub: 42 })).toBeNull();
  });
});

describe("bearerToken", () => {
  const headers = (authorization?: string) =>
    new Headers(authorization === undefined ? {} : { authorization });

  it("reads `Bearer <token>`, case-insensitively", () => {
    expect(bearerToken(headers("Bearer abc.def.ghi"))).toBe("abc.def.ghi");
    expect(bearerToken(headers("bearer abc"))).toBe("abc");
  });

  it("returns null for anything else", () => {
    for (const value of [
      undefined,
      "",
      "Bearer",
      "Bearer ",
      "Basic abc",
      "Bearer a b",
      "abc",
    ]) {
      expect(bearerToken(headers(value))).toBeNull();
    }
  });

  it("builds the issuer from the project URL", () => {
    expect(supabaseIssuer(`${SUPABASE_URL}//`)).toBe(ISSUER);
  });
});

describe("resolveIdentity", () => {
  const identity = { id: USER_ID, email: "me@example.com", name: null };
  const cookieIdentity = async () => ({ ...identity, id: "cookie-user" });

  it("uses a valid bearer token", async () => {
    const token = await signEs256();
    const result = await resolveIdentity(
      new Headers({ authorization: `Bearer ${token}` }),
      {
        verifyBearer: verify,
        cookieIdentity,
      },
    );
    expect(result).toEqual({
      identity: { ...identity, name: "Me Example" },
      via: "bearer",
    });
  });

  it("never falls back to cookies after a bad bearer or a malformed header", async () => {
    for (const authorization of ["Bearer not-a-jwt", "Basic abc", "Bearer"]) {
      const result = await resolveIdentity(
        new Headers({ authorization, cookie: "sb-access-token=x" }),
        { verifyBearer: verify, cookieIdentity },
      );
      expect(result).toBeNull();
    }
  });

  it("uses the cookie session when there is no Authorization header", async () => {
    const result = await resolveIdentity(new Headers({ cookie: "sb=x" }), {
      verifyBearer: verify,
      cookieIdentity,
    });
    expect(result?.via).toBe("cookie");
    expect(result?.identity.id).toBe("cookie-user");
  });

  it("ignores cookies when the caller doesn't accept them (the app API)", async () => {
    const result = await resolveIdentity(new Headers({ cookie: "sb=x" }), {
      verifyBearer: verify,
    });
    expect(result).toBeNull();
  });
});
