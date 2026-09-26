/**
 * The browser's tRPC client logs to the console in development only: in production, refusals a
 * page expects (NOT_FOUND, TOO_MANY_REQUESTS) must not print as errors.
 */
import { readFileSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

import { shouldLogTrpc } from "~/trpc/logging";

describe("tRPC logging", () => {
  it("logs in development only", () => {
    expect(shouldLogTrpc("development")).toBe(true);
    expect(shouldLogTrpc("production")).toBe(false);
    expect(shouldLogTrpc("test")).toBe(false);
    expect(shouldLogTrpc(undefined)).toBe(false);
  });

  it("is what the client's logger link follows, for every call", () => {
    const source = readFileSync(
      path.resolve(import.meta.dirname, "../../src/trpc/react.tsx"),
      "utf8",
    );
    expect(source).toMatch(
      /loggerLink\(\{\s*enabled: \(\) => shouldLogTrpc\(process\.env\.NODE_ENV\)\s*\}\)/,
    );
  });
});
