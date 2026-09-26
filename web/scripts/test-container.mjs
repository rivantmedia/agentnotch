/**
 * Which throwaway Postgres container scripts/test-integration.mjs starts, stops and removes.
 *
 * `agentnotch-web-test` on 127.0.0.1:55432 unless the environment names another:
 * Scripts/cloud-contract-e2e.sh runs its test in `agentnotch-web-test-e2e` on 55439
 * (AGENTNOTCH_WEB_TEST_CONTAINER, AGENTNOTCH_WEB_TEST_PORT), so it never stops a normal run's
 * container. Only `agentnotch-web-test` and names beginning `agentnotch-web-test-`, on ports
 * 55432 to 55439, are accepted: the runner must never touch any other container or port.
 */

export const DEFAULT_CONTAINER = "agentnotch-web-test";
export const DEFAULT_PORT = 55432;
export const PORT_RANGE = /** @type {const} */ ([55432, 55439]);

const NAME = /^agentnotch-web-test(-[a-z0-9]+)*$/;

/**
 * @param {Record<string, string | undefined>} env
 * @returns {{ name: string; port: number; databaseUrl: string }}
 */
export function testContainer(env) {
  const name = env.AGENTNOTCH_WEB_TEST_CONTAINER?.trim() || DEFAULT_CONTAINER;
  const portText = env.AGENTNOTCH_WEB_TEST_PORT?.trim() || String(DEFAULT_PORT);
  if (!NAME.test(name)) {
    throw new Error(
      `AGENTNOTCH_WEB_TEST_CONTAINER must be ${DEFAULT_CONTAINER} or start with ${DEFAULT_CONTAINER}- (lowercase letters and digits), not "${name}"`,
    );
  }
  const port = /^\d+$/.test(portText) ? Number(portText) : NaN;
  if (!(port >= PORT_RANGE[0] && port <= PORT_RANGE[1])) {
    throw new Error(
      `AGENTNOTCH_WEB_TEST_PORT must be from ${PORT_RANGE[0]} to ${PORT_RANGE[1]}, not "${portText}"`,
    );
  }
  return {
    name,
    port,
    databaseUrl: `postgresql://postgres:test@127.0.0.1:${port}/postgres`,
  };
}
