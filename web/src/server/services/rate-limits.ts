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

/**
 * A token bucket per key, taken in one atomic statement: the row is created full (minus the
 * token being taken), or refilled and decremented only when a whole token is there. A refused
 * call changes nothing, so the refill keeps running from the last successful take.
 */
export function createDbRateLimiter(
  db: Db,
  options: TokenBucketOptions & {
    /** Namespaces the keys, e.g. "sync". */
    name: string;
    now?: () => Date;
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
      if (taken.length > 0) return { ok: true };

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
