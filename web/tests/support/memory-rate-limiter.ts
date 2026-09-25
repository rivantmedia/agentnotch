/**
 * An in-memory stand-in for the database rate limiter (src/server/services/rate-limits.ts), with
 * the same bucket arithmetic, for tests of the handlers. Production never keeps limits in memory:
 * on a serverless host that would be per instance.
 */
import {
  refilledTokens,
  retryAfterSeconds,
  type RateLimiter,
  type TokenBucketOptions,
} from "~/server/app-api/rate-limit";

export function memoryRateLimiter(
  options: TokenBucketOptions & { now?: () => number },
): RateLimiter {
  const clock = options.now ?? (() => Date.now());
  const buckets = new Map<string, { tokens: number; updatedAt: Date }>();
  return {
    async take(key) {
      const now = new Date(clock());
      const bucket = buckets.get(key);
      const tokens = bucket
        ? refilledTokens(bucket, now, options)
        : options.capacity;
      if (tokens >= 1) {
        buckets.set(key, { tokens: tokens - 1, updatedAt: now });
        return { ok: true };
      }
      return {
        ok: false,
        retryAfterSeconds: retryAfterSeconds(tokens, options),
      };
    },
  };
}
