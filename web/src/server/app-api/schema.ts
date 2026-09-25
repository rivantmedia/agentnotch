/**
 * The app ⇄ website contract, version 1 (contract/README.md), as zod schemas.
 *
 * These validate the wire format only: strings stay strings (dates are not turned into `Date`),
 * so a fixture parses back to itself. Normalisation (lowercasing ids, parsing dates) happens in
 * the sync service. Unknown fields are stripped, which is how "unknown fields are ignored".
 *
 * Dates are bounded by the server's clock, so the sync request schema is built for a clock:
 * `makeSyncRequestSchema(clock)`; `syncRequestSchema` reads the real one at every parse.
 */
import { z } from "zod";

/** Contract limits, in one place so tests and the service can refer to them. */
export const LIMITS = {
  accounts: 50,
  sessions: 200,
  usage: 500,
  windowsPerReading: 20,
  modelsPerSession: 10,
  deviceName: 120,
  appVersion: 40,
  email: 320,
  organizationName: 200,
  plan: 60,
  label: 80,
  projectName: 120,
  title: 200,
  summaryText: 2000,
  summaryModel: 80,
  /** Not in the contract table; a sanity bound on one model id (real ones are ~30 chars). */
  modelId: 200,
  /** Request body cap for POST /sync. */
  bodyBytes: 5 * 1024 * 1024,
} as const;

/** The fixed redirect the app registers for its sign-in. */
export const APP_REDIRECT_URL = "agentnotch://auth-callback";

const HEX64 = /^[0-9a-f]{64}$/;
// Any RFC 4122 layout, either case: Swift's UUID().uuidString is uppercase.
const UUID =
  /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;
// `weekly_<model>`: the app folds a model family's name to lowercase letters, digits and `_`
// ("Sonnet 4.5" → weekly_sonnet_4_5). 60 characters is far beyond any real one.
const WINDOW_ID = /^(session|weekly_all|extra_usage|weekly_[a-z0-9._-]{1,60})$/;

const DAY_MS = 24 * 60 * 60 * 1000;

/**
 * Every date in a request lies between this and one day after the server's clock
 * (contract/README.md). Claude Code didn't exist before it; the day ahead covers clock skew.
 */
export const DATE_FLOOR = "2023-01-01T00:00:00Z";
export const DATE_FLOOR_MS = Date.parse(DATE_FLOOR);
export const MAX_AHEAD_MS = DAY_MS;
/**
 * A usage window's reset time is the one date that is in the future by nature: a weekly window
 * resets up to 7 days ahead (the contract fixture's does, 4 days after its reading). It gets the
 * same floor and a ceiling that still keeps nonsense out.
 */
export const RESETS_AT_MAX_AHEAD_MS = 32 * DAY_MS;

/** A lowercase hex SHA-256 digest (accountKey, project key). */
export const hexKey = z
  .string()
  .regex(HEX64, "must be 64 lowercase hex characters");

export const uuid = z.string().regex(UUID, "must be a UUID");

/** ISO 8601 in UTC with a `Z` suffix; fractional seconds allowed. */
export const isoDate = z
  .string()
  .datetime({
    offset: false,
    message: "must be an ISO 8601 UTC date ending in Z",
  })
  .refine((value) => Number.isFinite(Date.parse(value)), "must be a real date");

/** Milliseconds since the epoch, as the server sees it now. */
export type Clock = () => number;

/** An `isoDate` no earlier than 2023 and at most `aheadMs` past the clock. */
export function boundedDate(clock: Clock, aheadMs: number = MAX_AHEAD_MS) {
  return isoDate.superRefine((value, ctx) => {
    const time = Date.parse(value);
    if (time < DATE_FLOOR_MS) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message: "must not be before 2023-01-01",
      });
    } else if (time > clock() + aheadMs) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message:
          aheadMs === MAX_AHEAD_MS
            ? "must not be more than a day after the server's clock"
            : "is too far in the future",
      });
    }
  });
}

const nonNegativeInt = z
  .number()
  .int()
  .nonnegative()
  .max(Number.MAX_SAFE_INTEGER);

const nullableText = (max: number) => z.string().max(max).nullable();

export const sessionSourceSchema = z.enum([
  "cli",
  "vscode",
  "desktop",
  "sdk",
  "other",
]);
export const usageSourceSchema = z.enum([
  "probe",
  "statusLine",
  "claudeJson",
  "desktop",
]);

export const deviceSchema = z.object({
  id: uuid,
  name: z.string().max(LIMITS.deviceName),
  appVersion: z.string().max(LIMITS.appVersion),
});

export const accountSchema = z.object({
  key: hexKey,
  email: nullableText(LIMITS.email),
  organizationName: nullableText(LIMITS.organizationName),
  plan: nullableText(LIMITS.plan),
  label: nullableText(LIMITS.label),
});

export const tokensSchema = z.object({
  input: nonNegativeInt,
  output: nonNegativeInt,
  cacheCreation: nonNegativeInt,
  cacheRead: nonNegativeInt,
});

function makeSummarySchema(clock: Clock) {
  return z.object({
    text: z.string().max(LIMITS.summaryText),
    model: z.string().max(LIMITS.summaryModel),
    generatedAt: boundedDate(clock),
  });
}

function makeSessionSchema(clock: Clock) {
  return z.object({
    accountKey: hexKey,
    sessionId: uuid,
    project: z.object({
      key: hexKey,
      name: z.string().max(LIMITS.projectName),
    }),
    title: nullableText(LIMITS.title),
    source: sessionSourceSchema,
    // No minimum length per id: one odd entry must not cost the whole batch.
    models: z
      .array(z.string().max(LIMITS.modelId))
      .max(LIMITS.modelsPerSession),
    startedAt: boundedDate(clock),
    lastActivityAt: boundedDate(clock),
    endedAt: boundedDate(clock).nullable(),
    // Postgres `integer`.
    messageCount: z.number().int().nonnegative().max(2_147_483_647),
    tokens: tokensSchema,
    // Postgres `numeric(14, 6)`; no session costs a hundred million dollars.
    costUsd: z.number().nonnegative().finite().lt(1e8).nullable(),
    // Absent: keep the stored summary. `null` is treated like absent (the contract defines only
    // absent and an object; a client that encodes "no value" as null must not wipe anything).
    summary: makeSummarySchema(clock).nullish(),
  });
}

function makeUsageWindowSchema(clock: Clock) {
  return z.object({
    id: z
      .string()
      .regex(
        WINDOW_ID,
        "must be session, weekly_all, weekly_<model> or extra_usage",
      ),
    // 0–100, and may exceed 100.
    utilization: z.number().nonnegative().finite(),
    resetsAt: boundedDate(clock, RESETS_AT_MAX_AHEAD_MS).nullable(),
  });
}

function makeUsageReadingSchema(clock: Clock) {
  return z.object({
    accountKey: hexKey,
    source: usageSourceSchema,
    observedAt: boundedDate(clock),
    windows: z
      .array(makeUsageWindowSchema(clock))
      .max(LIMITS.windowsPerReading),
  });
}

const realClock: Clock = () => Date.now();

export const summarySchema = makeSummarySchema(realClock);
export const sessionSchema = makeSessionSchema(realClock);
export const usageWindowSchema = makeUsageWindowSchema(realClock);
export const usageReadingSchema = makeUsageReadingSchema(realClock);

type AccountRefs = {
  accounts: { key: string }[];
  sessions: { accountKey: string }[];
  usage: { accountKey: string }[];
};

/**
 * Paths of every session or usage reading whose `accountKey` is missing from `accounts[]`.
 * The contract requires each to appear there.
 */
export function unknownAccountRefs(request: AccountRefs): string[] {
  const known = new Set(request.accounts.map((a) => a.key));
  const problems: string[] = [];
  request.sessions.forEach((s, i) => {
    if (!known.has(s.accountKey)) problems.push(`sessions.${i}.accountKey`);
  });
  request.usage.forEach((u, i) => {
    if (!known.has(u.accountKey)) problems.push(`usage.${i}.accountKey`);
  });
  return problems;
}

/** The sync request schema, with every date bounded by `clock`. */
export function makeSyncRequestSchema(clock: Clock = realClock) {
  return z
    .object({
      schemaVersion: z.literal(1),
      device: deviceSchema,
      accounts: z.array(accountSchema).max(LIMITS.accounts),
      sessions: z.array(makeSessionSchema(clock)).max(LIMITS.sessions),
      usage: z.array(makeUsageReadingSchema(clock)).max(LIMITS.usage),
    })
    .superRefine((request, ctx) => {
      for (const path of unknownAccountRefs(request)) {
        ctx.addIssue({
          code: z.ZodIssueCode.custom,
          path: path.split(".").map((p) => (/^\d+$/.test(p) ? Number(p) : p)),
          message: "must appear in accounts[]",
        });
      }
    });
}

/** The sync request schema against the real clock, read at each parse. */
export const syncRequestSchema = makeSyncRequestSchema();

export const syncResponseSchema = z.object({
  accepted: z.object({
    sessions: nonNegativeInt,
    usage: nonNegativeInt,
  }),
  serverTime: isoDate,
});

export const configResponseSchema = z.object({
  supabaseUrl: z.string().url(),
  supabasePublishableKey: z.string().min(1),
  redirectUrl: z.literal(APP_REDIRECT_URL),
  dashboardUrl: z.string().url(),
});

export const meResponseSchema = z.object({
  user: z.object({
    id: z.string().min(1),
    email: z.string(),
    name: z.string().nullable(),
  }),
  dashboardUrl: z.string().url(),
});

export const APP_ERROR_CODES = [
  "UNAUTHORIZED",
  "FORBIDDEN",
  "BAD_REQUEST",
  "PAYLOAD_TOO_LARGE",
  "RATE_LIMITED",
  "INTERNAL",
] as const;

export const errorResponseSchema = z.object({
  error: z.object({
    code: z.enum(APP_ERROR_CODES),
    message: z.string(),
  }),
});

export type SyncRequest = z.infer<typeof syncRequestSchema>;
export type SyncSession = z.infer<typeof sessionSchema>;
export type SyncUsageReading = z.infer<typeof usageReadingSchema>;
export type SyncResponse = z.infer<typeof syncResponseSchema>;
export type ConfigResponse = z.infer<typeof configResponseSchema>;
export type MeResponse = z.infer<typeof meResponseSchema>;
export type ErrorResponse = z.infer<typeof errorResponseSchema>;
export type AppErrorCode = (typeof APP_ERROR_CODES)[number];
export type SessionSource = z.infer<typeof sessionSourceSchema>;
export type UsageSource = z.infer<typeof usageSourceSchema>;
