/**
 * The viewer's controls over what the website keeps of theirs (/settings). Each runs in one
 * transaction, under the lock a sync of theirs takes (sync.ts), so it never interleaves with one,
 * and touches only rows the viewer owns: every statement is filtered by their id. The exceptions
 * are what the viewer's own pools hold, which goes with the pools, and bare Claude account keys
 * that no row of anyone's refers to any more.
 */
import { Prisma, type PrismaClient } from "~/server/db-types";
import { lockUserData } from "~/server/services/sync";

/** Someone with years of sessions has a lot to go through; Prisma's default is 5 s. */
const LONG_TRANSACTION = { maxWait: 5_000, timeout: 60_000 };

/**
 * Clears the summary (text, model, time) of every session the viewer synced. Everything else
 * about the sessions stays. A Mac sends a summary again only with a session that changes while
 * its summaries switch is on.
 */
export async function removeSummaries(
  db: PrismaClient,
  userId: string,
): Promise<{ sessions: number }> {
  return db.$transaction(async (tx) => {
    await lockUserData(tx, userId);
    // Raw SQL, because Prisma's update would also bump "updatedAt", which means "synced last":
    // which of two people's copies of a session counts depends on it (sql.ts), and removing a
    // summary must not change that.
    const cleared = await tx.$executeRaw`
      UPDATE "Session"
      SET "summaryText" = NULL, "summaryModel" = NULL, "summaryAt" = NULL
      WHERE "userId" = ${userId}
        AND ("summaryText" IS NOT NULL OR "summaryModel" IS NOT NULL OR "summaryAt" IS NOT NULL)`;
    return { sessions: cleared };
  }, LONG_TRANSACTION);
}

/** What `deleteSyncedData` removed, by kind. */
export type DeletedData = {
  sessions: number;
  projects: number;
  usageReadings: number;
  usageWindows: number;
  /** The viewer's reports of Claude accounts (`UserAccount`). */
  accounts: number;
  /** Claude account keys nothing referred to any more once the viewer's rows were gone. */
  accountKeys: number;
  devices: number;
  /** Pools the viewer created, deleted with everyone's membership of them. */
  poolsDeleted: number;
  /** Other people's pools the viewer was in. */
  poolsLeft: number;
};

/**
 * Deletes everything the viewer's Macs synced, and what the website keeps about them besides:
 * sessions, projects, usage readings and window ids, account reports, Macs, pool memberships,
 * and the pools they created (with those pools' memberships). The Claude account keys they synced
 * go too, unless someone else's row still refers to one.
 *
 * Their user row stays (they are still signed in), and so do their rate limits: their sync bucket
 * and failed share-code attempts, which would otherwise start again after every deletion; they
 * empty on their own (rate-limits.ts, pools.ts). The per-IP limits' rows name nobody and stay
 * too, until they expire.
 */
export async function deleteSyncedData(
  db: PrismaClient,
  userId: string,
): Promise<DeletedData> {
  return db.$transaction(async (tx) => {
    await lockUserData(tx, userId);
    // The account keys the viewer's rows refer to, read before those rows go.
    const keys = await accountKeysOf(tx, userId);
    // Members go with each pool (ON DELETE CASCADE).
    const poolsDeleted = await tx.pool.deleteMany({
      where: { createdById: userId },
    });
    const poolsLeft = await tx.poolMember.deleteMany({ where: { userId } });
    // Sessions before projects and devices: they point at both.
    const sessions = await tx.session.deleteMany({ where: { userId } });
    const projects = await tx.project.deleteMany({ where: { userId } });
    const usageReadings = await tx.usageReading.deleteMany({
      where: { userId },
    });
    const usageWindows = await tx.usageWindow.deleteMany({ where: { userId } });
    const accounts = await tx.userAccount.deleteMany({ where: { userId } });
    const devices = await tx.device.deleteMany({ where: { userId } });
    const accountKeys = await deleteUnusedAccountKeys(tx, keys);
    return {
      sessions: sessions.count,
      projects: projects.count,
      usageReadings: usageReadings.count,
      usageWindows: usageWindows.count,
      accounts: accounts.count,
      accountKeys,
      devices: devices.count,
      poolsDeleted: poolsDeleted.count,
      poolsLeft: poolsLeft.count,
    };
  }, LONG_TRANSACTION);
}

/** Every Claude account key a row of `userId`'s refers to. */
async function accountKeysOf(
  tx: Prisma.TransactionClient,
  userId: string,
): Promise<string[]> {
  const rows = await tx.$queryRaw<Array<{ key: string }>>`
    SELECT "accountKey" AS "key" FROM "UserAccount" WHERE "userId" = ${userId}
    UNION SELECT "accountKey" FROM "Project" WHERE "userId" = ${userId}
    UNION SELECT "accountKey" FROM "Session" WHERE "userId" = ${userId}
    UNION SELECT "accountKey" FROM "UsageReading" WHERE "userId" = ${userId}
    UNION SELECT "accountKey" FROM "UsageWindow" WHERE "userId" = ${userId}
    UNION SELECT "accountKey" FROM "Pool" WHERE "createdById" = ${userId}`;
  return rows.map((r) => r.key);
}

/**
 * Deletes those of `keys` that no row refers to any more, and says how many.
 *
 * Every table's foreign key to ClaudeAccount cascades, so deleting a key someone else's sync is
 * storing rows for at this moment would delete their rows with it. Two statements prevent that.
 * The first locks the keys, skipping any another transaction holds: a sync storing a row for a
 * key holds it (the foreign key check locks it) and will refer to it, so it stays. The second is
 * a statement of its own, so it sees every row committed before the locks were taken; while they
 * are held, nobody can add a row for those keys. At worst, another person's sync that is just
 * about to store its first row for a key only the viewer had is refused once and sent again.
 */
async function deleteUnusedAccountKeys(
  tx: Prisma.TransactionClient,
  keys: readonly string[],
): Promise<number> {
  if (keys.length === 0) return 0;
  const locked = await tx.$queryRaw<Array<{ key: string }>>`
    SELECT "key" FROM "ClaudeAccount" WHERE "key" IN (${Prisma.join(keys)})
    ORDER BY "key" FOR UPDATE SKIP LOCKED`;
  if (locked.length === 0) return 0;
  return tx.$executeRaw`
    DELETE FROM "ClaudeAccount" a
    WHERE a."key" IN (${Prisma.join(locked.map((r) => r.key))})
      AND NOT EXISTS (SELECT 1 FROM "UserAccount" x WHERE x."accountKey" = a."key")
      AND NOT EXISTS (SELECT 1 FROM "Project" x WHERE x."accountKey" = a."key")
      AND NOT EXISTS (SELECT 1 FROM "Session" x WHERE x."accountKey" = a."key")
      AND NOT EXISTS (SELECT 1 FROM "UsageReading" x WHERE x."accountKey" = a."key")
      AND NOT EXISTS (SELECT 1 FROM "UsageWindow" x WHERE x."accountKey" = a."key")
      AND NOT EXISTS (SELECT 1 FROM "Pool" x WHERE x."accountKey" = a."key")`;
}
