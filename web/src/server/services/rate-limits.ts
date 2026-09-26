/**
 * Rate limits kept in Postgres (the `RateLimit` table), so they hold across every server
 * instance: an in-memory limit on a serverless host is per instance, and each new instance
 * starts with a full bucket.
 */
import {
  refilledTokens,
  retryAfterSeconds,
  type RateLimiter,
  type TokenBucketOptions,
} from "~/server/app-api/rate-limit";
import { type Db } from "~/server/db-types";
import { sqlTime } from "~/server/services/sql";

/** At most this many idle buckets are deleted per take (see `pruneIdleBuckets`). */
export const RATE_LIMIT_PRUNE_BATCH = 100;

/**
 * A token bucket per key, taken in one atomic statement: the row is created full (minus the
 * token being taken), or refilled and decremented only when a whole token is there. A refused
 * call changes nothing, so the refill keeps running from the last successful take.
 *
 * Each successful take also deletes some of this limiter's idle buckets (`pruneIdleBuckets`), so
 * the table holds only buckets in use: without that, every address that ever synced would leave
 * a row, with the time it last did, for good.
 */
export function createDbRateLimiter(
  db: Db,
  options: TokenBucketOptions & {
    /** Namespaces the keys, e.g. "sync". */
    name: string;
    now?: () => Date;
    /** How many idle buckets one take may delete (tests make it small). */
    pruneBatch?: number;
  },
): RateLimiter {
  const { capacity, perMs } = options;
  const perMsRate = capacity / perMs;
  const clock = options.now ?? (() => new Date());

  return {
    async take(key) {
      const now = clock();
      const bucketKey = `${options.name}:${key}`;
      const at = now.toISOString();
      // Timestamps are passed as ISO strings and read as UTC, whatever the session's time zone
      // (Prisma stores DateTime as UTC in `timestamp(3)`).
      const taken = await db.$queryRaw<Array<{ tokens: number }>>`
        INSERT INTO "RateLimit" AS r ("key", "tokens", "updatedAt")
        VALUES (${bucketKey}, ${capacity - 1}::float8, (${at}::timestamptz AT TIME ZONE 'UTC'))
        ON CONFLICT ("key") DO UPDATE SET
          "tokens" = LEAST(${capacity}::float8, r."tokens" + GREATEST(0::float8,
              EXTRACT(EPOCH FROM ((${at}::timestamptz AT TIME ZONE 'UTC') - r."updatedAt"))::float8 * 1000)
              * ${perMsRate}::float8) - 1,
          "updatedAt" = GREATEST(r."updatedAt", (${at}::timestamptz AT TIME ZONE 'UTC'))
        WHERE LEAST(${capacity}::float8, r."tokens" + GREATEST(0::float8,
            EXTRACT(EPOCH FROM ((${at}::timestamptz AT TIME ZONE 'UTC') - r."updatedAt"))::float8 * 1000)
            * ${perMsRate}::float8) >= 1
        RETURNING r."tokens"`;
      if (taken.length > 0) {
        await pruneIdleBuckets(db, {
          name: options.name,
          perMs,
          now,
          limit: options.pruneBatch ?? RATE_LIMIT_PRUNE_BATCH,
        });
        return { ok: true };
      }

      // Refused: say when the next token is back.
      const bucket = await db.rateLimit.findUnique({
        where: { key: bucketKey },
        select: { tokens: true, updatedAt: true },
      });
      const tokens = bucket ? refilledTokens(bucket, now, options) : capacity;
      return {
        ok: false,
        retryAfterSeconds: retryAfterSeconds(tokens, options),
      };
    },
  };
}

/**
 * Deletes up to `limit` of limiter `name`'s buckets untouched for `perMs` or longer. Such a bucket
 * has refilled to capacity whatever it held (tokens never go below zero), and a missing bucket
 * is created at capacity, so deleting it changes no decision. Rows another call has locked are
 * skipped rather than waited for, and each is checked again as it is deleted, so a take that
 * lands in between keeps its row.
 */
export async function pruneIdleBuckets(
  db: Db,
  options: { name: string; perMs: number; now: Date; limit: number },
): Promise<number> {
  const idleSince = sqlTime(new Date(options.now.getTime() - options.perMs));
  const prefix = `${options.name}:`;
  return db.$executeRaw`
    DELETE FROM "RateLimit"
    WHERE "key" IN (
        SELECT "key" FROM "RateLimit"
        WHERE starts_with("key", ${prefix}) AND "updatedAt" <= ${idleSince}
        LIMIT ${options.limit}
        FOR UPDATE SKIP LOCKED)
      AND "updatedAt" <= ${idleSince}`;
}
