/**
 * The Mac app's own sync request, stored by the website's own sync route: the website's half of
 * Scripts/cloud-contract-e2e.sh. Skipped unless AGENTNOTCH_CONTRACT_OUT names that request.
 *
 * The request is the exact bytes the app's code built and encoded (Packages/ClaudeControl,
 * CloudContractE2ETests). It goes through POST /api/app/v1/sync as deployed: the route file and
 * its real dependencies (the bearer check against the project's key set, the database rate
 * limits, the schema, applySync) into the test container's Postgres. The token is signed here and
 * the key set is served from memory, so nothing reaches the network or Supabase. The rows stored
 * must be the request's, a second post must change nothing, and the route's own answer is written
 * to AGENTNOTCH_CONTRACT_RESPONSE for the app to read back.
 *
 * Every field the app sent must be compared with what was stored, or listed in NOT_STORED with the
 * reason (tests/support/contract-rows.ts): the route's schema drops a field it doesn't know without
 * a word, so a field the app adds or renames would otherwise be lost while this check passed.
 *
 * Only this test's own user's rows are read, and nothing is deleted.
 */
import { randomUUID } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";

import { exportJWK, generateKeyPair, SignJWT } from "jose";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";

import {
  makeSyncRequestSchema,
  syncResponseSchema,
  type SyncRequest,
  type SyncResponse,
} from "~/server/app-api/schema";
import { ipKey } from "~/server/client-ip";
import { PrismaClient } from "~/server/db-types";

import {
  expectedRows,
  projectKey,
  readingKey,
  sessionKey,
  sortedBy,
  unstoredFields,
  windowKey,
} from "../support/contract-rows";

const requestFile = process.env.AGENTNOTCH_CONTRACT_OUT?.trim();
const responseFile = process.env.AGENTNOTCH_CONTRACT_RESPONSE?.trim();

const SUPABASE_URL = "https://contractcheck.supabase.co";
const JWKS_URL = `${SUPABASE_URL}/auth/v1/.well-known/jwks.json`;
const SITE = "https://agentnotch.example.com";
const PEPPER = "contract-check-pepper-0123456789";
const CLIENT_IP = "203.0.113.24";
/**
 * The contract check's own throwaway container (agentnotch-web-test-e2e, which
 * scripts/test-integration.mjs starts for Scripts/cloud-contract-e2e.sh), and nothing else.
 */
const CONTRACT_TEST_DATABASE =
  /^postgresql:\/\/[^@/]+@(127\.0\.0\.1|localhost):55439\//;

type Stored = Awaited<ReturnType<typeof storedFor>>;

async function storedFor(db: PrismaClient, userId: string) {
  const [user, devices, accounts, projects, sessions, readings, windows] =
    await Promise.all([
      db.user.findUnique({ where: { id: userId } }),
      db.device.findMany({ where: { userId } }),
      db.userAccount.findMany({ where: { userId } }),
      db.project.findMany({ where: { userId } }),
      db.session.findMany({ where: { userId }, include: { project: true } }),
      db.usageReading.findMany({ where: { userId } }),
      db.usageWindow.findMany({ where: { userId } }),
    ]);
  return { user, devices, accounts, projects, sessions, readings, windows };
}

describe.skipIf(!requestFile)("the Mac app's sync request", () => {
  const userId = randomUUID();
  const fetched: string[] = [];
  let db: PrismaClient;
  let POST: (request: Request) => Promise<Response>;
  let token: string;
  let body: string;
  let sent: SyncRequest;
  let firstAnswer: SyncResponse | undefined;

  const stored = () => storedFor(db, userId);

  function post(): Promise<Response> {
    return POST(
      new Request(`${SITE}/api/app/v1/sync`, {
        method: "POST",
        headers: {
          authorization: `Bearer ${token}`,
          "content-type": "application/json",
          "user-agent": "AgentNotch/9.9",
          "x-forwarded-for": CLIENT_IP,
        },
        body,
      }),
    );
  }

  beforeAll(async () => {
    // The route writes through DATABASE_URL, this test reads through TEST_DATABASE_URL: both
    // must be the contract check's throwaway container, never a real database.
    for (const name of ["DATABASE_URL", "TEST_DATABASE_URL"]) {
      if (!CONTRACT_TEST_DATABASE.test(process.env[name] ?? "")) {
        throw new Error(
          `${name} must be the contract check's test container (127.0.0.1:55439). Run Scripts/cloud-contract-e2e.sh.`,
        );
      }
    }
    body = readFileSync(requestFile!, "utf8");
    sent = JSON.parse(body) as SyncRequest;

    // Read by src/env.js when the route is first imported (below).
    vi.stubEnv("NEXT_PUBLIC_SUPABASE_URL", SUPABASE_URL);
    vi.stubEnv("NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY", "sb_publishable_check");
    vi.stubEnv("NEXT_PUBLIC_SITE_URL", SITE);
    vi.stubEnv("SUPABASE_JWT_SECRET", "");
    vi.stubEnv("RATE_LIMIT_PEPPER", PEPPER);

    // A Supabase access token as the Mac app holds one, signed with a key of this project's set.
    const { privateKey, publicKey } = await generateKeyPair("ES256", {
      extractable: true,
    });
    const publicJwk = {
      ...(await exportJWK(publicKey)),
      kid: "contract-check",
      alg: "ES256",
      use: "sig",
    };
    // The project's key set, from memory: the only thing the route may fetch.
    vi.stubGlobal("fetch", async (input: string | URL | Request) => {
      const url =
        typeof input === "string"
          ? input
          : input instanceof URL
            ? input.href
            : input.url;
      fetched.push(url);
      if (url !== JWKS_URL) throw new Error(`No network in this test: ${url}`);
      return Response.json({ keys: [publicJwk] });
    });
    const now = Math.floor(Date.now() / 1000);
    token = await new SignJWT({
      sub: userId,
      aud: "authenticated",
      iss: `${SUPABASE_URL}/auth/v1`,
      role: "authenticated",
      email: `${userId}@example.com`,
      app_metadata: { provider: "google", providers: ["google"] },
      user_metadata: { full_name: "Contract Check" },
      is_anonymous: false,
      iat: now - 10,
      exp: now + 3600,
    })
      .setProtectedHeader({ alg: "ES256", kid: "contract-check" })
      .sign(privateKey);

    ({ POST } = await import("~/app/api/app/v1/sync/route"));
    db = new PrismaClient({ datasourceUrl: process.env.TEST_DATABASE_URL });
  });

  afterAll(async () => {
    await db?.$disconnect();
    const { db: routeDb } = await import("~/server/db");
    await routeDb.$disconnect();
    vi.unstubAllGlobals();
    vi.unstubAllEnvs();
  });

  it("accounts for every field the app sent", () => {
    // The scenario covers the optional summary, so its fields are checked too.
    expect(sent.sessions.some((s) => s.summary)).toBe(true);
    // Each field is either compared with the stored rows below or listed as not stored.
    expect(unstoredFields(sent, expectedRows)).toEqual([]);
    // And the route's schema passes the request on whole: it strips what it doesn't know.
    expect(makeSyncRequestSchema().parse(sent)).toEqual(sent);
  });

  it("stores exactly what the app sent", async () => {
    const response = await post();
    const text = await response.text();
    expect(response.status, text).toBe(200);
    expect(response.headers.get("content-type")).toMatch(/^application\/json/);
    const answer = syncResponseSchema.parse(JSON.parse(text));
    // Everything was new, so everything was stored.
    expect(answer.accepted).toEqual({
      sessions: sent.sessions.length,
      usage: sent.usage.length,
    });
    firstAnswer = answer;
    // The route's own answer, as bytes, for the app to read back.
    if (responseFile) writeFileSync(responseFile, text);

    const expected = expectedRows(sent);
    const rows = await stored();
    expect(rows.user).toMatchObject({
      id: userId,
      email: `${userId}@example.com`,
      name: "Contract Check",
    });
    expect(
      rows.devices.map((d) => ({
        id: d.id,
        name: d.name,
        appVersion: d.appVersion,
      })),
    ).toEqual(expected.devices);

    // Accounts: this user's report of each, and the shared key row.
    expect(
      sortedBy(rows.accounts, (a) => a.accountKey).map((a) => ({
        key: a.accountKey,
        email: a.email,
        organizationName: a.organizationName,
        plan: a.plan,
        label: a.label,
      })),
    ).toEqual(expected.accounts);
    const keys = await db.claudeAccount.findMany({
      where: { key: { in: expected.accountKeys } },
      select: { key: true },
    });
    expect(keys.map((k) => k.key).sort()).toEqual(expected.accountKeys);

    // Projects: one per account and project key.
    expect(
      sortedBy(rows.projects, projectKey).map((p) => ({
        accountKey: p.accountKey,
        key: p.key,
        name: p.name,
      })),
    ).toEqual(expected.projects);

    // Sessions: one row per account and session, every field as sent, token totals exact.
    expect(
      sortedBy(rows.sessions, sessionKey).map((s) => ({
        accountKey: s.accountKey,
        sessionId: s.sessionId,
        project: { key: s.project.key, name: s.project.name },
        title: s.title,
        source: s.source,
        models: s.models,
        startedAt: s.startedAt,
        lastActivityAt: s.lastActivityAt,
        endedAt: s.endedAt,
        messageCount: s.messageCount,
        tokens: {
          input: s.inputTokens,
          output: s.outputTokens,
          cacheCreation: s.cacheCreationTokens,
          cacheRead: s.cacheReadTokens,
        },
        costUsd: s.costUsd === null ? null : s.costUsd.toNumber(),
        summary:
          s.summaryAt === null
            ? null
            : { text: s.summaryText, model: s.summaryModel, at: s.summaryAt },
        deviceId: s.deviceId,
      })),
    ).toEqual(expected.sessions);

    // Usage: one row per reading and window, and the window ids each account has.
    expect(
      sortedBy(
        rows.readings.map((r) => ({
          accountKey: r.accountKey,
          source: r.source,
          windowId: r.windowId,
          utilization: r.utilization,
          resetsAt: r.resetsAt,
          observedAt: r.observedAt,
        })),
        readingKey,
      ),
    ).toEqual(expected.readings);
    expect(rows.windows.map(windowKey).sort()).toEqual(expected.windows);
  });

  it("changes nothing when the same request comes again", async () => {
    expect(firstAnswer, "the first post failed").toBeDefined();
    const before = await stored();
    const response = await post();
    const text = await response.text();
    expect(response.status, text).toBe(200);
    expect(syncResponseSchema.parse(JSON.parse(text)).accepted).toEqual(
      firstAnswer?.accepted,
    );
    // Only the times of the latest sighting move: a Mac's and an account's lastSeenAt, and a
    // session's updatedAt. Every row keeps its id: nothing was added or replaced.
    const stable = (rows: Stored) => ({
      user: rows.user,
      devices: rows.devices.map((d) => ({ ...d, lastSeenAt: null })),
      accounts: sortedBy(rows.accounts, (a) => a.accountKey).map((a) => ({
        ...a,
        lastSeenAt: null,
      })),
      projects: sortedBy(rows.projects, (p) => p.id),
      sessions: sortedBy(rows.sessions, (s) => s.id).map((s) => ({
        ...s,
        updatedAt: null,
      })),
      readings: sortedBy(rows.readings, (r) => String(r.id)),
      windows: sortedBy(rows.windows, (w) => `${w.accountKey}|${w.windowId}`),
    });
    expect(stable(await stored())).toEqual(stable(before));
  });

  it("checked the token against the project's key set and reached nothing else", async () => {
    expect(fetched.length).toBeGreaterThan(0);
    expect([...new Set(fetched)]).toEqual([JWKS_URL]);
    // The route's real limits, kept in Postgres: the user's bucket, and the address's under its
    // keyed hash.
    const buckets = await db.rateLimit.findMany({
      where: {
        key: { in: [`sync:${userId}`, `sync-ip:${ipKey(CLIENT_IP, PEPPER)}`] },
      },
    });
    expect(buckets).toHaveLength(2);
  });
});
