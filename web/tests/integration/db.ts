/**
 * The throwaway database scripts/test-integration.mjs starts. Never a real one: the URL must be
 * the local test container's.
 */
import { afterAll } from "vitest";

import { PrismaClient } from "~/server/db-types";
import { ensureUser } from "~/server/services/users";

const url = process.env.TEST_DATABASE_URL;
if (!url) {
  throw new Error(
    "TEST_DATABASE_URL is not set. Run `npm run test:integration`.",
  );
}
if (!/@(127\.0\.0\.1|localhost):55432\//.test(url)) {
  throw new Error(
    "Integration tests only run against the local test container (port 55432).",
  );
}

export const db = new PrismaClient({ datasourceUrl: url });

afterAll(async () => {
  await db.$disconnect();
});

/** Empties every table (in one statement, so foreign keys don't matter). */
export async function resetDb(): Promise<void> {
  await db.$executeRawUnsafe(
    `TRUNCATE TABLE "PoolJoinFailure", "PoolJoinIpFailure", "PoolMember", "Pool",
       "UsageReading", "UsageWindow", "Session", "Project", "UserAccount", "ClaudeAccount",
       "Device", "RateLimit", "User"
       RESTART IDENTITY CASCADE`,
  );
}

export async function createUser(id: string, name: string | null = null) {
  return ensureUser(db, { id, email: `${id}@example.com`, name });
}
