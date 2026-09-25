/**
 * The website side of the app ⇄ website contract: every fixture in contract/fixtures validates
 * against the zod schemas, parses back to itself, and is exactly what the handlers produce.
 */
import { createHash, createHmac } from "node:crypto";

import { describe, expect, it } from "vitest";

import {
  errorResponse,
  tokenCheckUnavailable,
  toErrorResponse,
} from "~/server/app-api/errors";
import {
  buildConfig,
  handleMe,
  handleSync,
  type AppApiDeps,
} from "~/server/app-api/handlers";
import { SYNC_RATE } from "~/server/app-api/rate-limit";
import {
  configResponseSchema,
  errorResponseSchema,
  makeSyncRequestSchema,
  meResponseSchema,
  syncResponseSchema,
} from "~/server/app-api/schema";

import {
  fixture,
  FIXTURE_NAMES,
  keysFixture,
  syncFixture,
  type FixtureName,
} from "../support/fixtures";
import { memoryRateLimiter } from "../support/memory-rate-limiter";

/** The server's clock for the fixtures: sync-response.json's serverTime. */
const SERVER_NOW = new Date("2026-09-25T11:21:00.000Z");
const syncRequestSchema = makeSyncRequestSchema(() => SERVER_NOW.getTime());

const SCHEMAS = {
  "config.json": configResponseSchema,
  "error.json": errorResponseSchema,
  "me.json": meResponseSchema,
  "sync-request.json": syncRequestSchema,
  "sync-response.json": syncResponseSchema,
} as const;

const SITE_URL = "https://agentnotch.example.com";

function deps(overrides: Partial<AppApiDeps> = {}): AppApiDeps {
  const me = fixture("me.json") as {
    user: { id: string; email: string; name: string | null };
  };
  return {
    resolveViewer: async (headers) =>
      headers.get("authorization") === "Bearer good" ? me.user : null,
    applySync: async (_viewer, request) => ({
      sessions: request.sessions.length,
      usage: request.usage.length,
    }),
    limiter: memoryRateLimiter(SYNC_RATE),
    siteUrl: SITE_URL,
    now: () => SERVER_NOW,
    ...overrides,
  };
}

const sha256 = (text: string) =>
  createHash("sha256").update(text, "utf8").digest("hex");
const hmacSha256 = (keyHex: string, text: string) =>
  createHmac("sha256", Buffer.from(keyHex, "hex"))
    .update(text, "utf8")
    .digest("hex");

describe("contract fixtures", () => {
  it("every fixture is covered by a schema or the key check", () => {
    const covered = new Set<FixtureName>([
      ...(Object.keys(SCHEMAS) as FixtureName[]),
      "keys.json",
    ]);
    expect([...covered].sort()).toEqual([...FIXTURE_NAMES].sort());
  });

  for (const [name, schema] of Object.entries(SCHEMAS)) {
    it(`${name} validates and round-trips`, () => {
      const raw = fixture(name as FixtureName);
      const parsed = schema.safeParse(raw);
      expect(parsed.error?.issues ?? []).toEqual([]);
      // Nothing added, nothing dropped, nothing converted (dates stay strings).
      expect(parsed.data).toEqual(raw);
      expect(JSON.parse(JSON.stringify(parsed.data))).toEqual(raw);
    });
  }

  it("config.json is what GET /config builds", () => {
    const expected = fixture("config.json") as {
      supabaseUrl: string;
      supabasePublishableKey: string;
    };
    for (const siteUrl of [SITE_URL, `${SITE_URL}/`]) {
      expect(
        buildConfig({
          supabaseUrl: `${expected.supabaseUrl}/`,
          supabasePublishableKey: expected.supabasePublishableKey,
          siteUrl,
        }),
      ).toEqual(expected);
    }
  });

  it("error.json is what an unauthorized call returns", async () => {
    const response = errorResponse("UNAUTHORIZED", "Sign in again.");
    expect(response.status).toBe(401);
    expect(await response.json()).toEqual(fixture("error.json"));

    const me = await handleMe(new Request(`${SITE_URL}/api/app/v1/me`), deps());
    expect(me.status).toBe(401);
    expect(await me.json()).toEqual(fixture("error.json"));
  });

  it("a token that can't be checked right now is 503 INTERNAL, in the error shape", async () => {
    const response = toErrorResponse(tokenCheckUnavailable());
    expect(response.status).toBe(503);
    const body = errorResponseSchema.parse(await response.json());
    expect(body.error.code).toBe("INTERNAL");
  });

  it("me.json is what GET /me returns", async () => {
    const response = await handleMe(
      new Request(`${SITE_URL}/api/app/v1/me`, {
        headers: { authorization: "Bearer good" },
      }),
      deps(),
    );
    expect(response.status).toBe(200);
    expect(response.headers.get("cache-control")).toBe("no-store");
    expect(await response.json()).toEqual(fixture("me.json"));
  });

  it("sync-response.json is what POST /sync returns for sync-request.json", async () => {
    const response = await handleSync(
      new Request(`${SITE_URL}/api/app/v1/sync`, {
        method: "POST",
        headers: {
          authorization: "Bearer good",
          "content-type": "application/json",
        },
        body: JSON.stringify(fixture("sync-request.json")),
      }),
      deps(),
    );
    expect(response.status).toBe(200);
    expect(await response.json()).toEqual(fixture("sync-response.json"));
  });

  it("ignores unknown fields at every level", () => {
    const request = syncFixture();
    request.futureField = { nested: true };
    request.sessions[0]!.futureField = 1;
    (request.accounts[0] as Record<string, unknown>).futureField = "x";
    (request.usage[0]!.windows[0] as Record<string, unknown>).futureField =
      null;
    const parsed = syncRequestSchema.parse(request);
    expect(parsed).toEqual(syncFixture());
  });
});

describe("keys.json", () => {
  const keys = keysFixture();

  it("account keys are SHA-256 of the lowercased accountUuid[/organizationUuid]", () => {
    for (const account of keys.accounts) {
      const base = account.organizationUuid
        ? `${account.accountUuid}/${account.organizationUuid}`
        : account.accountUuid;
      expect(sha256(base.toLowerCase())).toBe(account.key);
      // Case never changes the key.
      expect(sha256(base.toUpperCase().toLowerCase())).toBe(account.key);
    }
  });

  it("project keys are HMAC-SHA256 of `<accountKey>:<path>` with the install secret", () => {
    expect(keys.installSecretHex).toMatch(/^[0-9a-f]{64}$/); // 32 bytes
    for (const project of keys.projects) {
      const message = `${project.accountKey}:${project.path}`;
      expect(hmacSha256(keys.installSecretHex, message)).toBe(project.key);
      // Not guessable from the path alone: the unkeyed digest is something else.
      expect(sha256(message)).not.toBe(project.key);
    }
  });

  it("the install secret never travels", () => {
    for (const name of FIXTURE_NAMES) {
      if (name === "keys.json") continue;
      expect(JSON.stringify(fixture(name))).not.toContain(
        keys.installSecretHex,
      );
    }
  });

  it("sync-request.json uses exactly these keys", () => {
    const request = syncFixture();
    expect(request.accounts.map((a) => a.key)).toEqual(
      keys.accounts.map((a) => a.key),
    );
    expect(request.sessions.map((s) => [s.accountKey, s.project.key])).toEqual(
      keys.projects.map((p) => [p.accountKey, p.key]),
    );
    // No path reaches the website: only its last component, as the project name.
    for (const [i, session] of request.sessions.entries()) {
      const path = keys.projects[i]!.path;
      expect(session.project.name).toBe(path.split("/").at(-1));
      expect(JSON.stringify(request)).not.toContain(path);
    }
  });
});
