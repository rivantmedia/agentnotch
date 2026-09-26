/**
 * The integration runner's container: the normal one by default, the contract check's own when
 * the environment names it, and never anything outside `agentnotch-web-test*` on 55432–55439
 * (the runner stops and removes the container it names).
 */
import { describe, expect, it } from "vitest";

import { testContainer } from "../../scripts/test-container.mjs";

describe("testContainer", () => {
  it("is agentnotch-web-test on 127.0.0.1:55432 by default", () => {
    expect(testContainer({})).toEqual({
      name: "agentnotch-web-test",
      port: 55432,
      databaseUrl: "postgresql://postgres:test@127.0.0.1:55432/postgres",
    });
    expect(
      testContainer({
        AGENTNOTCH_WEB_TEST_CONTAINER: " ",
        AGENTNOTCH_WEB_TEST_PORT: "",
      }).name,
    ).toBe("agentnotch-web-test");
  });

  it("takes the contract check's own container", () => {
    expect(
      testContainer({
        AGENTNOTCH_WEB_TEST_CONTAINER: "agentnotch-web-test-e2e",
        AGENTNOTCH_WEB_TEST_PORT: "55439",
      }),
    ).toEqual({
      name: "agentnotch-web-test-e2e",
      port: 55439,
      databaseUrl: "postgresql://postgres:test@127.0.0.1:55439/postgres",
    });
  });

  it("refuses any other container name", () => {
    for (const name of [
      "postgres",
      "agentnotch-web",
      "agentnotch-web-testing",
      "agentnotch-web-test-",
      "agentnotch-web-test-E2E",
      "agentnotch-web-test-e2e;rm",
      "my-agentnotch-web-test",
    ]) {
      expect(() =>
        testContainer({ AGENTNOTCH_WEB_TEST_CONTAINER: name }),
      ).toThrow(/AGENTNOTCH_WEB_TEST_CONTAINER/);
    }
  });

  it("refuses any port outside 55432–55439", () => {
    for (const port of ["5432", "55431", "55440", "55432.5", "-55432", "x"]) {
      expect(() => testContainer({ AGENTNOTCH_WEB_TEST_PORT: port })).toThrow(
        /AGENTNOTCH_WEB_TEST_PORT/,
      );
    }
  });
});
