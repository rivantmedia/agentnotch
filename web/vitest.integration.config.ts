/**
 * Integration tests (`npm run test:integration`): the services against a real Postgres. Run them
 * through scripts/test-integration.mjs, which starts a throwaway container, applies the
 * migrations, sets TEST_DATABASE_URL and always stops the container afterwards.
 */
import path from "node:path";
import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    alias: {
      "~": path.resolve(import.meta.dirname, "src"),
      "server-only": path.resolve(
        import.meta.dirname,
        "tests/support/server-only.ts",
      ),
    },
  },
  test: {
    include: ["tests/integration/**/*.test.ts"],
    environment: "node",
    env: { SKIP_ENV_VALIDATION: "1" },
    // One database: files take turns, and each test starts from empty tables.
    fileParallelism: false,
    testTimeout: 60_000,
    hookTimeout: 60_000,
  },
});
