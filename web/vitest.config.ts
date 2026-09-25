/**
 * Unit tests (`npm test`): pure logic, contract fixtures, handlers with fakes. No database, no
 * network, no real env values. Integration tests have their own config and runner
 * (vitest.integration.config.ts, scripts/test-integration.mjs); keep `resolve` the same in both.
 */
import path from "node:path";
import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    alias: {
      "~": path.resolve(import.meta.dirname, "src"),
      // Next.js resolves this to an empty module on the server; outside Next it would throw.
      "server-only": path.resolve(
        import.meta.dirname,
        "tests/support/server-only.ts",
      ),
    },
  },
  test: {
    include: ["tests/unit/**/*.test.ts"],
    environment: "node",
    env: { SKIP_ENV_VALIDATION: "1" },
  },
});
