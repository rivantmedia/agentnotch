/**
 * The website's tRPC endpoint over HTTP, with the real routers and a fake database: batches are
 * capped, mutations can't be batched, and in production an unexpected error's text (Prisma,
 * Postgres) never reaches the browser but is logged.
 */
import { beforeAll, describe, expect, it, vi } from "vitest";

import { type handleTrpcRequest as HandleTrpcRequest } from "~/server/api/http";
import { type appRouter as AppRouterValue } from "~/server/api/root";
import { type TRPCContext } from "~/server/api/trpc";

const VIEWER = {
  id: "ann",
  email: "ann@example.com",
  name: "Ann",
  via: "cookie" as const,
};
const PRISMA_TEXT =
  'Invalid `prisma.session.findMany()` invocation: ConnectorError(PostgresError { code: "22021" })';

/** A database whose every query fails the way Prisma does. */
const failingDb = new Proxy(
  {},
  {
    get: () =>
      new Proxy(
        {},
        {
          get: () => async () => {
            throw new Error(PRISMA_TEXT);
          },
        },
      ),
  },
) as TRPCContext["db"];

let handleTrpcRequest: typeof HandleTrpcRequest;
let appRouter: typeof AppRouterValue;
let GENERIC: string;
let MAX: number;

beforeAll(async () => {
  // The formatter reads NODE_ENV when env.js loads, so load everything as in production.
  vi.stubEnv("NODE_ENV", "production");
  vi.resetModules();
  const http = await import("~/server/api/http");
  handleTrpcRequest = http.handleTrpcRequest;
  GENERIC = http.GENERIC_INTERNAL_MESSAGE;
  MAX = http.TRPC_MAX_BATCH_SIZE;
  appRouter = (await import("~/server/api/root")).appRouter;
  vi.unstubAllEnvs();
});

function call(req: Request, log = vi.fn()) {
  const response = handleTrpcRequest(req, {
    router: appRouter,
    createContext: () => ({
      db: failingDb,
      viewer: VIEWER,
      headers: req.headers,
    }),
    log,
  });
  return { response, log };
}

const BASE = "https://agentnotch.example.com/api/trpc";

function batchGet(paths: string[]) {
  const input = Object.fromEntries(paths.map((_, i) => [i, { json: null }]));
  return new Request(
    `${BASE}/${paths.join(",")}?batch=1&input=${encodeURIComponent(JSON.stringify(input))}`,
  );
}

describe("tRPC over HTTP", () => {
  it("caps a batch at 10 procedures", async () => {
    expect(MAX).toBe(10);
    const ok = await call(batchGet(Array<string>(10).fill("viewer.current")))
      .response;
    expect(ok.status).toBe(200);

    const tooMany = await call(
      batchGet(Array<string>(11).fill("viewer.current")),
    ).response;
    expect(tooMany.status).toBe(400);
    expect(JSON.stringify(await tooMany.json())).toContain(
      "Batch call exceeds maximum size",
    );
  });

  it("never batches mutations: one code guess per request", async () => {
    const codes = ["7K3M9QX2H4TB", "7K3M9QX2H4TC"];
    const body = Object.fromEntries(
      codes.map((code, i) => [i, { json: { code } }]),
    );
    const { response, log } = call(
      new Request(`${BASE}/pools.join,pools.join?batch=1`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      }),
    );
    const refused = await response;
    expect(refused.status).toBe(400);
    expect(JSON.stringify(await refused.json())).toContain(
      "Batching is not enabled",
    );
    // Refused before any procedure ran.
    expect(log).not.toHaveBeenCalled();
  });

  it("hides an unexpected error's text in production, and logs it", async () => {
    const { response, log } = call(
      new Request(`${BASE}/pools.join`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ json: { code: "7K3M-9QX2-H4TB" } }),
      }),
    );
    const failed = await response;
    expect(failed.status).toBe(500);
    const text = await failed.text();
    expect(text).not.toContain("prisma");
    expect(text).not.toContain("Postgres");
    expect(text).toContain(GENERIC);
    expect(log).toHaveBeenCalledOnce();
    expect(String(log.mock.calls[0]![0])).toContain("pools.join");
  });

  it("keeps the messages written for people", async () => {
    const { response } = call(
      new Request(`${BASE}/pools.join`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ json: { code: "short" } }),
      }),
    );
    const refused = await response;
    expect(refused.status).toBe(400);
    expect(await refused.text()).toContain("12 letters and digits");
  });

  it("refuses NUL in text inputs as bad input, before Postgres sees it", async () => {
    const input = { 0: { json: { search: "a\u0000b" } } };
    const { response, log } = call(
      new Request(
        `${BASE}/sessions.list?batch=1&input=${encodeURIComponent(JSON.stringify(input))}`,
      ),
    );
    const refused = await response;
    expect(refused.status).toBe(400);
    expect(log).not.toHaveBeenCalled();
  });
});
