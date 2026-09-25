/**
 * The Mac app's routes as plain functions with fake dependencies: status codes, the error shape,
 * the body cap and the rate limit.
 */
import { describe, expect, it, vi } from "vitest";

import { AppApiError } from "~/server/app-api/errors";
import {
  handleMe,
  handleSync,
  type AppApiDeps,
} from "~/server/app-api/handlers";
import { SYNC_RATE } from "~/server/app-api/rate-limit";
import { errorResponseSchema, LIMITS } from "~/server/app-api/schema";
import { TokenCheckUnavailable } from "~/server/auth/token-errors";

import { syncFixture } from "../support/fixtures";
import { memoryRateLimiter } from "../support/memory-rate-limiter";

const URL_SYNC = "https://agentnotch.example.com/api/app/v1/sync";
const VIEWER = { id: "user-1", email: "me@example.com", name: null };
const SERVER_NOW = new Date("2026-09-25T11:21:00.000Z");

function deps(overrides: Partial<AppApiDeps> = {}): AppApiDeps {
  return {
    resolveViewer: async (headers) =>
      headers.get("authorization") === "Bearer good" ? VIEWER : null,
    applySync: vi.fn(async () => ({ sessions: 2, usage: 2 })),
    limiter: memoryRateLimiter(SYNC_RATE),
    siteUrl: "https://agentnotch.example.com",
    // The fixture's dates are 2026-09-25; the server's clock is pinned just after them.
    now: () => SERVER_NOW,
    ...overrides,
  };
}

function syncRequest(
  body: BodyInit | null,
  headers: Record<string, string> = {},
): Request {
  return new Request(URL_SYNC, {
    method: "POST",
    headers: {
      authorization: "Bearer good",
      "content-type": "application/json",
      ...headers,
    },
    body,
    // Needed by undici for a streamed body.
    ...(body instanceof ReadableStream ? { duplex: "half" } : {}),
  });
}

async function expectError(response: Response, status: number, code: string) {
  expect(response.status).toBe(status);
  expect(response.headers.get("cache-control")).toBe("no-store");
  const body: unknown = await response.json();
  const parsed = errorResponseSchema.parse(body);
  expect(parsed.error.code).toBe(code);
  expect(parsed.error.message.length).toBeGreaterThan(0);
  // Exactly the contract's shape, nothing more.
  expect(Object.keys(body as object)).toEqual(["error"]);
  expect(Object.keys(parsed.error).sort()).toEqual(["code", "message"]);
  return parsed.error;
}

describe("POST /api/app/v1/sync", () => {
  it("accepts the fixture and passes the parsed request on", async () => {
    const d = deps();
    const response = await handleSync(
      syncRequest(JSON.stringify(syncFixture())),
      d,
    );
    expect(response.status).toBe(200);
    expect(d.applySync).toHaveBeenCalledOnce();
    expect(vi.mocked(d.applySync).mock.calls[0]![0]).toEqual(VIEWER);
    const body = (await response.json()) as {
      accepted: unknown;
      serverTime: string;
    };
    expect(body.accepted).toEqual({ sessions: 2, usage: 2 });
    expect(body.serverTime).toMatch(/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/);
  });

  it("needs a valid bearer token", async () => {
    for (const authorization of [
      undefined,
      "Bearer bad",
      "Basic Zm9vOmJhcg==",
      "Bearer",
    ]) {
      const d = deps();
      const response = await handleSync(
        new Request(URL_SYNC, {
          method: "POST",
          headers: authorization ? { authorization } : {},
          body: JSON.stringify(syncFixture()),
        }),
        d,
      );
      await expectError(response, 401, "UNAUTHORIZED");
      expect(d.applySync).not.toHaveBeenCalled();
    }
  });

  it("answers BAD_REQUEST for invalid JSON, a non-UTF-8 body or no body", async () => {
    await expectError(
      await handleSync(syncRequest("{not json"), deps()),
      400,
      "BAD_REQUEST",
    );
    await expectError(
      await handleSync(syncRequest(new Uint8Array([0x7b, 0xff, 0x7d])), deps()),
      400,
      "BAD_REQUEST",
    );
    await expectError(
      await handleSync(syncRequest(null), deps()),
      400,
      "BAD_REQUEST",
    );
  });

  it("answers BAD_REQUEST with the failing paths for a schema violation", async () => {
    const request = syncFixture();
    request.sessions[0]!.accountKey = "f".repeat(64);
    request.device.name = "x".repeat(500);
    const d = deps();
    const error = await expectError(
      await handleSync(syncRequest(JSON.stringify(request)), d),
      400,
      "BAD_REQUEST",
    );
    expect(error.message).toContain("device.name");
    expect(error.message).toContain("sessions.0.accountKey");
    expect(d.applySync).not.toHaveBeenCalled();
  });

  it("refuses bodies over 5 MB by Content-Length without reading them", async () => {
    const d = deps();
    const response = await handleSync(
      syncRequest("{}", { "content-length": String(LIMITS.bodyBytes + 1) }),
      d,
    );
    await expectError(response, 413, "PAYLOAD_TOO_LARGE");
    expect(d.applySync).not.toHaveBeenCalled();
  });

  it("refuses streamed bodies once they pass 5 MB", async () => {
    const chunk = new Uint8Array(1024 * 1024).fill(0x20);
    let sent = 0;
    const stream = new ReadableStream<Uint8Array>({
      pull(controller) {
        // Endless: the handler must stop reading by itself.
        sent += chunk.byteLength;
        controller.enqueue(chunk);
        if (sent > 64 * 1024 * 1024) controller.close();
      },
    });
    const response = await handleSync(syncRequest(stream), deps());
    await expectError(response, 413, "PAYLOAD_TOO_LARGE");
    expect(sent).toBeLessThanOrEqual(LIMITS.bodyBytes + 2 * chunk.byteLength);
  });

  it("accepts a body just under the cap", async () => {
    const request = JSON.stringify(syncFixture());
    const padded = request.replace(
      /}\s*$/,
      `,"padding":"${"x".repeat(LIMITS.bodyBytes - request.length - 20)}"}`,
    );
    expect(padded.length).toBeLessThanOrEqual(LIMITS.bodyBytes);
    const response = await handleSync(syncRequest(padded), deps());
    expect(response.status).toBe(200);
  });

  it("rate-limits each user to a burst of 12 syncs, then one per 10 s", async () => {
    let now = 0;
    const d = deps({
      limiter: memoryRateLimiter({ ...SYNC_RATE, now: () => now }),
    });
    const body = JSON.stringify(syncFixture());
    for (let i = 0; i < 12; i++) {
      expect((await handleSync(syncRequest(body), d)).status).toBe(200);
    }
    const limited = await handleSync(syncRequest(body), d);
    await expectError(limited, 429, "RATE_LIMITED");
    expect(limited.headers.get("retry-after")).toBe("10");
    expect(d.applySync).toHaveBeenCalledTimes(12);

    now += 10_000; // one token back
    expect((await handleSync(syncRequest(body), d)).status).toBe(200);
    expect((await handleSync(syncRequest(body), d)).status).toBe(429);
  });

  it("answers 503, never 401, when the sign-in can't be checked right now", async () => {
    const logError = vi.fn();
    const d = deps({
      resolveViewer: async () => {
        throw new TokenCheckUnavailable("key set timed out");
      },
      logError,
    });
    const response = await handleSync(
      syncRequest(JSON.stringify(syncFixture())),
      d,
    );
    const error = await expectError(response, 503, "INTERNAL");
    expect(error.message).not.toContain("key set");
    expect(response.headers.get("retry-after")).toBe("30");
    expect(d.applySync).not.toHaveBeenCalled();
    expect(logError).toHaveBeenCalledOnce();

    const me = await handleMe(
      new Request("https://x.example/api/app/v1/me", {
        headers: { authorization: "Bearer good" },
      }),
      d,
    );
    await expectError(me, 503, "INTERNAL");
  });

  it("refuses dates outside 2023 … the server's clock plus a day", async () => {
    const d = deps();
    const late = syncFixture();
    late.usage[0]!.observedAt = "2026-09-26T11:21:01Z"; // a day and a second ahead
    const error = await expectError(
      await handleSync(syncRequest(JSON.stringify(late)), d),
      400,
      "BAD_REQUEST",
    );
    expect(error.message).toContain("usage.0.observedAt");

    const early = syncFixture();
    early.sessions[0]!.startedAt = "2022-12-31T23:59:59Z";
    await expectError(
      await handleSync(syncRequest(JSON.stringify(early)), d),
      400,
      "BAD_REQUEST",
    );

    // The bound follows the server's clock, not a fixed date.
    const later = deps({ now: () => new Date("2026-09-27T00:00:00Z") });
    expect(
      (await handleSync(syncRequest(JSON.stringify(late)), later)).status,
    ).toBe(200);
    expect(d.applySync).not.toHaveBeenCalled();
  });

  it("reports unexpected failures as INTERNAL without their details", async () => {
    const logError = vi.fn();
    const d = deps({
      applySync: async () => {
        throw new Error('relation "Session" does not exist');
      },
      logError,
    });
    const error = await expectError(
      await handleSync(syncRequest(JSON.stringify(syncFixture())), d),
      500,
      "INTERNAL",
    );
    expect(error.message).not.toContain("Session");
    expect(logError).toHaveBeenCalledOnce();
  });

  it("passes contract errors from the service through", async () => {
    const d = deps({
      applySync: async () => {
        throw new AppApiError("FORBIDDEN", "Not yours.");
      },
    });
    const error = await expectError(
      await handleSync(syncRequest(JSON.stringify(syncFixture())), d),
      403,
      "FORBIDDEN",
    );
    expect(error.message).toBe("Not yours.");
  });
});

describe("GET /api/app/v1/me", () => {
  it("returns the viewer and the dashboard URL", async () => {
    const response = await handleMe(
      new Request("https://x.example/api/app/v1/me", {
        headers: { authorization: "Bearer good" },
      }),
      deps({ siteUrl: "https://agentnotch.example.com///" }),
    );
    expect(await response.json()).toEqual({
      user: VIEWER,
      dashboardUrl: "https://agentnotch.example.com/dashboard",
    });
  });

  it("needs a valid bearer token", async () => {
    await expectError(
      await handleMe(new Request("https://x.example/api/app/v1/me"), deps()),
      401,
      "UNAUTHORIZED",
    );
  });
});
