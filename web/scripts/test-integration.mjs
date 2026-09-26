#!/usr/bin/env node
/**
 * `npm run test:integration`: runs tests/integration against a real, throwaway Postgres.
 *
 * 1. Starts `postgres:16-alpine` as the container `agentnotch-web-test` on 127.0.0.1:55432
 *    (`--rm`, so stopping it deletes it). A leftover container of that name from an interrupted
 *    run is stopped first. No other container is ever touched. Scripts/cloud-contract-e2e.sh
 *    names its own (`agentnotch-web-test-e2e` on 55439) through the environment, and runs only
 *    tests/integration/contract-e2e.test.ts in it; scripts/test-container.mjs accepts nothing
 *    outside `agentnotch-web-test*` on ports 55432 to 55439.
 * 2. Applies prisma/migrations with `prisma migrate deploy`, then checks the result has no drift
 *    from prisma/schema.prisma.
 * 3. Runs Vitest with vitest.integration.config.ts (extra arguments are passed through).
 * 4. Always stops the container, including on failure and Ctrl-C.
 */
import { spawn, spawnSync } from "node:child_process";
import { setTimeout as delay } from "node:timers/promises";

import { testContainer } from "./test-container.mjs";

const {
  name: CONTAINER,
  port: PORT,
  databaseUrl: DATABASE_URL,
} = testContainer(process.env);
const IMAGE = "postgres:16-alpine";

/** @param {string[]} args */
function docker(args, { quiet = false } = {}) {
  return spawnSync("docker", args, {
    encoding: "utf8",
    stdio: quiet ? ["ignore", "pipe", "pipe"] : ["ignore", "pipe", "inherit"],
  });
}

function containerExists() {
  const result = docker(
    ["ps", "-a", "--filter", `name=^/${CONTAINER}$`, "--format", "{{.Names}}"],
    { quiet: true },
  );
  return result.status === 0 && result.stdout.trim() === CONTAINER;
}

let stopped = false;
function stopContainer() {
  if (stopped) return;
  stopped = true;
  if (!containerExists()) return;
  console.log(`\n[test:integration] stopping ${CONTAINER}`);
  docker(["stop", "--time", "2", CONTAINER], { quiet: true });
  // `--rm` removes it on stop; this only covers a container started without it.
  if (containerExists()) docker(["rm", "-f", CONTAINER], { quiet: true });
}

/**
 * @param {string} command
 * @param {string[]} args
 * @param {NodeJS.ProcessEnv} env
 * @returns {Promise<number>}
 */
function run(command, args, env) {
  return new Promise((resolve) => {
    const child = spawn(command, args, {
      stdio: "inherit",
      env: { ...process.env, ...env },
    });
    child.on("exit", (code, signal) => resolve(code ?? (signal ? 1 : 0)));
    child.on("error", () => resolve(1));
  });
}

async function waitForPostgres() {
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    // TCP, not the socket: the image's init phase runs a socket-only server first.
    const ready = docker(
      ["exec", CONTAINER, "pg_isready", "-h", "127.0.0.1", "-U", "postgres"],
      { quiet: true },
    );
    if (ready.status === 0) return;
    await delay(500);
  }
  throw new Error("Postgres did not become ready within 60 s");
}

for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"]) {
  process.on(signal, () => {
    stopContainer();
    process.exit(130);
  });
}

let exitCode = 1;
try {
  if (docker(["info"], { quiet: true }).status !== 0) {
    throw new Error("Docker is not running.");
  }
  if (containerExists()) {
    console.log(`[test:integration] removing a leftover ${CONTAINER}`);
    docker(["stop", "--time", "2", CONTAINER], { quiet: true });
    if (containerExists()) docker(["rm", "-f", CONTAINER], { quiet: true });
  }

  console.log(
    `[test:integration] starting ${CONTAINER} (${IMAGE}) on 127.0.0.1:${PORT}`,
  );
  const started = docker([
    "run",
    "-d",
    "--rm",
    "--name",
    CONTAINER,
    "-p",
    `127.0.0.1:${PORT}:5432`,
    "-e",
    "POSTGRES_PASSWORD=test",
    IMAGE,
  ]);
  if (started.status !== 0) throw new Error("docker run failed");
  await waitForPostgres();

  const dbEnv = {
    DATABASE_URL,
    DIRECT_URL: DATABASE_URL,
    TEST_DATABASE_URL: DATABASE_URL,
    SKIP_ENV_VALIDATION: "1",
  };
  const migrated = await run("npx", ["prisma", "migrate", "deploy"], dbEnv);
  if (migrated !== 0) throw new Error("prisma migrate deploy failed");

  // The migrated database must be exactly what schema.prisma describes: a model change without
  // a migration fails here (exit code 2 means "there is a difference").
  const drift = await run(
    "npx",
    [
      "prisma",
      "migrate",
      "diff",
      "--from-url",
      DATABASE_URL,
      "--to-schema-datamodel",
      "prisma/schema.prisma",
      "--exit-code",
    ],
    dbEnv,
  );
  if (drift !== 0) {
    throw new Error(
      "prisma/schema.prisma and prisma/migrations disagree (see the diff above)",
    );
  }

  exitCode = await run(
    "npx",
    [
      "vitest",
      "run",
      "--config",
      "vitest.integration.config.ts",
      ...process.argv.slice(2),
    ],
    dbEnv,
  );
} catch (error) {
  console.error(
    `[test:integration] ${error instanceof Error ? error.message : String(error)}`,
  );
  exitCode = 1;
} finally {
  stopContainer();
}
process.exit(exitCode);
