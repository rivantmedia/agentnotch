/**
 * Rate limits, as token buckets: `capacity` calls at once, refilled continuously at `capacity`
 * per `perMs`. The buckets live in the database (src/server/services/rate-limits.ts), so every
 * server instance shares them; this file holds the shape and the arithmetic.
 */
export type RateLimitResult =
  { ok: true } | { ok: false; retryAfterSeconds: number };

export type RateLimiter = {
  /** Takes one token from `key`'s bucket, or says how long until one is back. */
  take(key: string): Promise<RateLimitResult>;
};

export type TokenBucketOptions = {
  /** Burst size, and the number of calls allowed per `perMs`. */
  capacity: number;
  perMs: number;
};

/**
 * Sync calls per user: a burst of 12, refilled at one every 10 s (6 a minute). The app syncs every
 * 5 minutes, 30 s after a session ends, and in back-to-back batches while it catches up: one pass
 * sends at most 10 requests, so a whole pass fits the burst. It honours Retry-After on a 429.
 */
export const SYNC_RATE = { capacity: 12, perMs: 120_000 } as const;

/**
 * Sync calls per client IP address, whoever is signed in: 60 a minute (a burst of 60, refilled at
 * one a second), on top of each user's own limit. Many people's Macs behind one office address
 * fit; one person with many accounts can't multiply their rate.
 */
export const SYNC_IP_RATE = { capacity: 60, perMs: 60_000 } as const;

/** Limiter names: a bucket's key is `<name>:<user id or IP key>`. */
export const SYNC_LIMITER = "sync";
export const SYNC_IP_LIMITER = "sync-ip";

/** The limiters keyed by user id. */
export const USER_RATE_LIMITS = [SYNC_LIMITER] as const;

/** Every rate-limit bucket that belongs to one user. */
export function userRateLimitKeys(userId: string): string[] {
  return USER_RATE_LIMITS.map((name) => `${name}:${userId}`);
}

/** The tokens in a bucket that held `tokens` at `updatedAt`, as of `now`. */
export function refilledTokens(
  bucket: { tokens: number; updatedAt: Date },
  now: Date,
  options: TokenBucketOptions,
): number {
  // Server instances' clocks differ a little; time never runs backwards for a bucket.
  const elapsed = Math.max(0, now.getTime() - bucket.updatedAt.getTime());
  return Math.min(
    options.capacity,
    bucket.tokens + (elapsed * options.capacity) / options.perMs,
  );
}

/** Whole seconds until a bucket holding `tokens` has one to give (at least 1). */
export function retryAfterSeconds(
  tokens: number,
  options: TokenBucketOptions,
): number {
  const waitMs = ((1 - tokens) * options.perMs) / options.capacity;
  return Math.max(1, Math.ceil(waitMs / 1000));
}
