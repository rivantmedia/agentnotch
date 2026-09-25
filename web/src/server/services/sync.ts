/**
 * POST /api/app/v1/sync: stores one batch from a user's Mac, in one short transaction.
 *
 * Idempotent by design (contract/README.md): sessions carry absolute totals and replace what is
 * stored for (user, accountKey, sessionId); usage readings are deduplicated on
 * (user, accountKey, source, window id, observedAt). Only names and numbers arrive here: project
 * keys are keyed digests, never paths, and there are no prompts.
 *
 * Cost control: each table is written by one bulk statement (the rows travel as one JSON
 * parameter), so the transaction is a handful of round trips however big the batch. Per-user
 * quotas cap what one person can store (SYNC_QUOTAS), and readings past the retention window are
 * pruned. Past the account cap a batch is refused; past the others, what doesn't fit is dropped
 * and the rest is stored, so one busy day never holds up session updates or usage readings.
 * `accepted` counts what was stored.
 */
import { randomUUID } from "node:crypto";

import { AppApiError } from "~/server/app-api/errors";
import {
  unknownAccountRefs,
  type SyncRequest,
  type SyncSession,
} from "~/server/app-api/schema";
import { type Prisma, type PrismaClient } from "~/server/db-types";

/**
 * What a batch stored: `sessions` counts the request's sessions, and `usage` its usage readings
 * (entries of `usage[]`) with at least one window stored. Something stored before counts too
 * (syncing is idempotent); something a quota dropped, or past the retention window, doesn't.
 */
export type SyncResult = { sessions: number; usage: number };

/** What one person may store. */
export const SYNC_QUOTAS = {
  /** Distinct Claude accounts per user. A batch that would go past it is refused (403). */
  accounts: 50,
  /** Sessions first stored in any 24 hours. New sessions past it are dropped from the batch. */
  newSessionsPerDay: 5000,
  /**
   * Usage readings first stored in any 24 hours, counting each window's value at a moment
   * (one stored row). New ones past it are dropped from the batch.
   */
  newUsageReadingsPerDay: 20_000,
  /**
   * Distinct usage window ids per user and account, ever (they outlive the readings). Readings of
   * windows past it are dropped: the real ones are a handful, so ids made up to fill the database
   * or the shared meters stop here.
   */
  windowsPerAccount: 32,
} as const;

export type SyncQuotas = { [K in keyof typeof SYNC_QUOTAS]: number };

/** Usage readings older than this are dropped on arrival and pruned on every sync. */
export const USAGE_RETENTION_DAYS = 90;

/**
 * A ceiling on each token count of one session. Far above anything real (a very long session
 * reads a few billion cached tokens), and low enough that summing every session a viewer can see
 * stays well inside Postgres' bigint.
 */
export const TOKEN_CAP = 1_000_000_000_000;

const DAY_MS = 24 * 60 * 60 * 1000;

export async function applySync(
  db: PrismaClient,
  userId: string,
  request: SyncRequest,
  now: Date = new Date(),
  quotaOverrides: Partial<SyncQuotas> = {},
): Promise<SyncResult> {
  const quotas: SyncQuotas = { ...SYNC_QUOTAS, ...quotaOverrides };
  // The route validates this already; the service holds the rule itself too.
  const unknown = unknownAccountRefs(request);
  if (unknown.length > 0) {
    throw new AppApiError(
      "BAD_REQUEST",
      `${unknown.slice(0, 3).join(", ")}: must appear in accounts[]`,
    );
  }

  const deviceId = request.device.id.toLowerCase();
  const accounts = lastByKey(request.accounts, (a) => a.key);
  // One row per (account, session): the same session resumed under another account is another
  // row, and a duplicate inside the batch would trip the bulk upsert.
  const allSessions = lastByKey(request.sessions, sessionKey);
  const cutoff = new Date(now.getTime() - USAGE_RETENTION_DAYS * DAY_MS);
  const allReadings = firstByKey(
    request.usage
      .flatMap(usageRowsOf)
      .filter((r) => r.observedAt >= cutoff.toISOString()),
    usageRowKey,
  );
  const at = now.toISOString();

  const kept = await db.$transaction(
    async (tx) => {
      // One sync per user at a time: the quotas are counted and applied atomically, and two
      // batches never lock the same rows in opposite orders.
      await tx.$executeRaw`SELECT pg_advisory_xact_lock(hashtextextended(${`sync:${userId}`}, 0))`;
      await enforceAccountQuota(tx, userId, accounts, quotas);
      const sessions = await sessionsWithinQuota(
        tx,
        userId,
        allSessions,
        now,
        quotas,
      );
      const usage = await usageWithinQuota(
        tx,
        userId,
        allReadings,
        now,
        quotas,
      );
      // Only the kept sessions' projects: a dropped session leaves no row behind.
      const projects = lastByKey(sessions, projectKey);

      // Devices are per user: a Mac someone else also syncs from is their own row.
      const device = {
        name: clean(request.device.name),
        appVersion: clean(request.device.appVersion),
        lastSeenAt: now,
      };
      await tx.device.upsert({
        where: { userId_id: { userId, id: deviceId } },
        create: { userId, id: deviceId, ...device },
        update: device,
      });

      if (accounts.length > 0) {
        await tx.claudeAccount.createMany({
          data: accounts.map((a) => ({ key: a.key })),
          skipDuplicates: true,
        });
        const rows = accounts.map((a) => ({
          key: a.key,
          email: cleanOrNull(a.email),
          organizationName: cleanOrNull(a.organizationName),
          plan: cleanOrNull(a.plan),
          label: cleanOrNull(a.label),
        }));
        await tx.$executeRaw`
          INSERT INTO "UserAccount" AS cur
            ("userId", "accountKey", "email", "organizationName", "plan", "label",
             "firstSeenAt", "lastSeenAt")
          SELECT ${userId}, a."key", a."email", a."organizationName", a."plan", a."label",
                 (${at}::timestamptz AT TIME ZONE 'UTC'), (${at}::timestamptz AT TIME ZONE 'UTC')
          FROM jsonb_to_recordset(${JSON.stringify(rows)}::jsonb) AS a(
            "key" text, "email" text, "organizationName" text, "plan" text, "label" text)
          ORDER BY a."key"
          ON CONFLICT ("userId", "accountKey") DO UPDATE SET
            "email" = EXCLUDED."email",
            "organizationName" = EXCLUDED."organizationName",
            "plan" = EXCLUDED."plan",
            "label" = EXCLUDED."label",
            "lastSeenAt" = EXCLUDED."lastSeenAt"`;
      }

      if (projects.length > 0) {
        const rows = projects.map((s) => ({
          id: randomUUID(), // used only when the project is new
          accountKey: s.accountKey,
          key: s.project.key,
          name: clean(s.project.name),
        }));
        await tx.$executeRaw`
          INSERT INTO "Project" AS cur ("id", "userId", "accountKey", "key", "name")
          SELECT p."id", ${userId}, p."accountKey", p."key", p."name"
          FROM jsonb_to_recordset(${JSON.stringify(rows)}::jsonb) AS p(
            "id" text, "accountKey" text, "key" text, "name" text)
          ORDER BY p."accountKey", p."key"
          ON CONFLICT ("userId", "accountKey", "key") DO UPDATE SET
            "name" = EXCLUDED."name"`;
      }

      if (sessions.length > 0) {
        const rows = sessions.map((s) => sessionRow(s));
        // A session's project is found by (user, account, project key): the upsert above made it.
        // An absent summary is sent as nulls, and then the stored one is kept.
        await tx.$executeRaw`
          INSERT INTO "Session" AS cur
            ("id", "userId", "accountKey", "projectId", "sessionId", "title", "source", "models",
             "startedAt", "lastActivityAt", "endedAt", "messageCount",
             "inputTokens", "outputTokens", "cacheCreationTokens", "cacheReadTokens", "costUsd",
             "summaryText", "summaryModel", "summaryAt", "deviceId", "createdAt", "updatedAt")
          SELECT s."id", ${userId}, s."accountKey", p."id", s."sessionId", s."title", s."source",
                 ARRAY(SELECT m FROM jsonb_array_elements_text(s."models") WITH ORDINALITY AS t(m, i)
                       ORDER BY i),
                 (s."startedAt" AT TIME ZONE 'UTC'), (s."lastActivityAt" AT TIME ZONE 'UTC'),
                 (s."endedAt" AT TIME ZONE 'UTC'), s."messageCount",
                 s."inputTokens", s."outputTokens", s."cacheCreationTokens", s."cacheReadTokens",
                 s."costUsd", s."summaryText", s."summaryModel", (s."summaryAt" AT TIME ZONE 'UTC'),
                 ${deviceId}, (${at}::timestamptz AT TIME ZONE 'UTC'),
                 (${at}::timestamptz AT TIME ZONE 'UTC')
          FROM jsonb_to_recordset(${JSON.stringify(rows)}::jsonb) AS s(
            "id" text, "accountKey" text, "projectKey" text, "sessionId" text, "title" text,
            "source" text, "models" jsonb, "startedAt" timestamptz, "lastActivityAt" timestamptz,
            "endedAt" timestamptz, "messageCount" integer, "inputTokens" bigint,
            "outputTokens" bigint, "cacheCreationTokens" bigint, "cacheReadTokens" bigint,
            "costUsd" numeric, "summaryText" text, "summaryModel" text, "summaryAt" timestamptz)
          JOIN "Project" p
            ON p."userId" = ${userId} AND p."accountKey" = s."accountKey" AND p."key" = s."projectKey"
          ORDER BY s."accountKey", s."sessionId"
          ON CONFLICT ("userId", "accountKey", "sessionId") DO UPDATE SET
            "projectId" = EXCLUDED."projectId",
            "title" = EXCLUDED."title",
            "source" = EXCLUDED."source",
            "models" = EXCLUDED."models",
            "startedAt" = EXCLUDED."startedAt",
            "lastActivityAt" = EXCLUDED."lastActivityAt",
            "endedAt" = EXCLUDED."endedAt",
            "messageCount" = EXCLUDED."messageCount",
            "inputTokens" = EXCLUDED."inputTokens",
            "outputTokens" = EXCLUDED."outputTokens",
            "cacheCreationTokens" = EXCLUDED."cacheCreationTokens",
            "cacheReadTokens" = EXCLUDED."cacheReadTokens",
            "costUsd" = EXCLUDED."costUsd",
            "summaryText" = CASE WHEN EXCLUDED."summaryAt" IS NULL
              THEN cur."summaryText" ELSE EXCLUDED."summaryText" END,
            "summaryModel" = CASE WHEN EXCLUDED."summaryAt" IS NULL
              THEN cur."summaryModel" ELSE EXCLUDED."summaryModel" END,
            "summaryAt" = COALESCE(EXCLUDED."summaryAt", cur."summaryAt"),
            "deviceId" = EXCLUDED."deviceId",
            "updatedAt" = EXCLUDED."updatedAt"`;
      }

      if (usage.newWindows.length > 0) {
        await tx.$executeRaw`
          INSERT INTO "UsageWindow" ("userId", "accountKey", "windowId", "firstSeenAt")
          SELECT ${userId}, w."accountKey", w."windowId", (${at}::timestamptz AT TIME ZONE 'UTC')
          FROM jsonb_to_recordset(${JSON.stringify(usage.newWindows)}::jsonb) AS w(
            "accountKey" text, "windowId" text)
          ORDER BY w."accountKey", w."windowId"
          ON CONFLICT ("userId", "accountKey", "windowId") DO NOTHING`;
      }

      if (usage.insert.length > 0) {
        await tx.$executeRaw`
          INSERT INTO "UsageReading"
            ("userId", "accountKey", "source", "windowId", "utilization", "resetsAt", "observedAt",
             "createdAt")
          SELECT ${userId}, u."accountKey", u."source", u."windowId", u."utilization",
                 (u."resetsAt" AT TIME ZONE 'UTC'), (u."observedAt" AT TIME ZONE 'UTC'),
                 (${at}::timestamptz AT TIME ZONE 'UTC')
          FROM jsonb_to_recordset(${JSON.stringify(usage.insert)}::jsonb) AS u(
            "accountKey" text, "source" text, "windowId" text, "utilization" float8,
            "resetsAt" timestamptz, "observedAt" timestamptz)
          ON CONFLICT ("userId", "accountKey", "source", "windowId", "observedAt") DO NOTHING`;
      }

      return { sessions, readings: usage.kept };
    },
    // A handful of statements: this is generous, and still frees a stuck connection quickly.
    { maxWait: 5_000, timeout: 15_000 },
  );

  await pruneReadings(
    db,
    accounts.map((a) => a.key),
    cutoff,
  );

  const keptSessions = new Set(kept.sessions.map(sessionKey));
  return {
    sessions: request.sessions.filter((s) => keptSessions.has(sessionKey(s)))
      .length,
    usage: request.usage.filter((reading) =>
      usageRowsOf(reading).some((row) => kept.readings.has(usageRowKey(row))),
    ).length,
  };
}

/** Refuses a batch that would take the user past the account cap (nothing is stored). */
async function enforceAccountQuota(
  tx: Prisma.TransactionClient,
  userId: string,
  accounts: readonly { key: string }[],
  quotas: SyncQuotas,
): Promise<void> {
  if (accounts.length === 0) return;
  const known = await tx.userAccount.findMany({
    where: { userId },
    select: { accountKey: true },
  });
  const keys = new Set(known.map((r) => r.accountKey));
  for (const account of accounts) keys.add(account.key);
  if (keys.size > quotas.accounts) {
    throw new AppApiError(
      "FORBIDDEN",
      `This website keeps at most ${quotas.accounts} Claude accounts per person.`,
    );
  }
}

/**
 * The batch's sessions that may be stored: every one already stored (an update never counts),
 * and new ones while the day's quota lasts. The rest are dropped, not refused, so updates to
 * running sessions and the batch's usage readings still go through.
 */
async function sessionsWithinQuota(
  tx: Prisma.TransactionClient,
  userId: string,
  sessions: readonly SyncSession[],
  now: Date,
  quotas: SyncQuotas,
): Promise<SyncSession[]> {
  if (sessions.length === 0) return [];
  const known = await tx.session.findMany({
    where: {
      userId,
      sessionId: { in: [...new Set(sessions.map(normalizedSessionId))] },
    },
    select: { accountKey: true, sessionId: true },
  });
  const stored = new Set(known.map((k) => `${k.accountKey}:${k.sessionId}`));
  if (sessions.every((s) => stored.has(sessionKey(s)))) return [...sessions];
  const lastDay = await tx.session.count({
    where: { userId, createdAt: { gt: new Date(now.getTime() - DAY_MS) } },
  });
  return keepNewestWithinRoom(
    sessions,
    (s) => stored.has(sessionKey(s)),
    (s) => Date.parse(s.lastActivityAt),
    quotas.newSessionsPerDay - lastDay,
  );
}

/**
 * Items already stored, plus as many new ones as `room` allows, newest first (what a dashboard
 * shows first), in their original order.
 */
export function keepNewestWithinRoom<T>(
  items: readonly T[],
  isStored: (item: T) => boolean,
  time: (item: T) => number,
  room: number,
): T[] {
  const fresh = items
    .map((item, index) => ({ item, index }))
    .filter(({ item }) => !isStored(item))
    .sort((a, b) => time(b.item) - time(a.item) || a.index - b.index)
    .slice(0, Math.max(0, room));
  const keep = new Set(fresh.map(({ item }) => item));
  return items.filter((item) => isStored(item) || keep.has(item));
}

/** What to write of a batch's usage readings, and what counts as stored. */
export type UsagePlan = {
  /** New readings to insert. */
  insert: UsageRow[];
  /** Window ids to record as the user's on their account. */
  newWindows: Array<{ accountKey: string; windowId: string }>;
  /** Keys (`usageRowKey`) of every reading stored after this batch: before, or now. */
  kept: Set<string>;
};

async function usageWithinQuota(
  tx: Prisma.TransactionClient,
  userId: string,
  rows: readonly UsageRow[],
  now: Date,
  quotas: SyncQuotas,
): Promise<UsagePlan> {
  if (rows.length === 0) {
    return { insert: [], newWindows: [], kept: new Set() };
  }
  // Which of these are stored already, by the unique key's index.
  const probe = rows.map((r, i) => ({
    i,
    accountKey: r.accountKey,
    source: r.source,
    windowId: r.windowId,
    observedAt: r.observedAt,
  }));
  const found = await tx.$queryRaw<Array<{ i: number }>>`
    SELECT u."i"
    FROM jsonb_to_recordset(${JSON.stringify(probe)}::jsonb) AS u(
      "i" integer, "accountKey" text, "source" text, "windowId" text, "observedAt" timestamptz)
    WHERE EXISTS (
      SELECT 1 FROM "UsageReading" r
      WHERE r."userId" = ${userId} AND r."accountKey" = u."accountKey"
        AND r."source" = u."source" AND r."windowId" = u."windowId"
        AND r."observedAt" = (u."observedAt" AT TIME ZONE 'UTC'))`;
  const stored = new Set(found.map((f) => usageRowKey(rows[f.i]!)));
  const fresh = rows.filter((r) => !stored.has(usageRowKey(r)));
  if (fresh.length === 0) {
    return planUsage(rows, {
      stored,
      knownWindows: [],
      room: 0,
      windowsPerAccount: quotas.windowsPerAccount,
    });
  }

  const knownWindows = await tx.usageWindow.findMany({
    where: {
      userId,
      accountKey: { in: [...new Set(fresh.map((r) => r.accountKey))] },
    },
    select: { accountKey: true, windowId: true },
  });
  const lastDay = await tx.usageReading.count({
    where: { userId, createdAt: { gt: new Date(now.getTime() - DAY_MS) } },
  });
  return planUsage(rows, {
    stored,
    knownWindows,
    room: quotas.newUsageReadingsPerDay - lastDay,
    windowsPerAccount: quotas.windowsPerAccount,
  });
}

/**
 * Which of a batch's readings to store. Stored ones stay. A new one is dropped when the day's
 * quota is used up (the newest are kept first: the meters show them), or when it would give its
 * account more window ids than `windowsPerAccount`. Pure.
 */
export function planUsage(
  rows: readonly UsageRow[],
  facts: {
    stored: ReadonlySet<string>;
    knownWindows: ReadonlyArray<{ accountKey: string; windowId: string }>;
    room: number;
    windowsPerAccount: number;
  },
): UsagePlan {
  const windows = new Map<string, Set<string>>();
  for (const { accountKey, windowId } of facts.knownWindows) {
    const ids = windows.get(accountKey) ?? new Set<string>();
    ids.add(windowId);
    windows.set(accountKey, ids);
  }
  const kept = new Set<string>();
  for (const row of rows) {
    const key = usageRowKey(row);
    if (facts.stored.has(key)) kept.add(key);
  }

  const insert: UsageRow[] = [];
  const newWindows: UsagePlan["newWindows"] = [];
  let room = Math.max(0, facts.room);
  const newestFirst = rows
    .map((row, index) => ({ row, index }))
    .filter(({ row }) => !facts.stored.has(usageRowKey(row)))
    .sort(
      (a, b) =>
        Date.parse(b.row.observedAt) - Date.parse(a.row.observedAt) ||
        a.index - b.index,
    );
  for (const { row } of newestFirst) {
    if (room === 0) break;
    const ids = windows.get(row.accountKey) ?? new Set<string>();
    windows.set(row.accountKey, ids);
    if (!ids.has(row.windowId)) {
      if (ids.size >= facts.windowsPerAccount) continue;
      ids.add(row.windowId);
      newWindows.push({ accountKey: row.accountKey, windowId: row.windowId });
    }
    room -= 1;
    insert.push(row);
    kept.add(usageRowKey(row));
  }
  return { insert, newWindows, kept };
}

/**
 * Drops readings past the retention window on the accounts this batch touched, whoever sent
 * them (retention is the same for everyone). Uses the (accountKey, observedAt) index. After the
 * commit, and never fatal: the batch is already stored.
 */
async function pruneReadings(
  db: PrismaClient,
  accountKeys: string[],
  cutoff: Date,
): Promise<void> {
  if (accountKeys.length === 0) return;
  try {
    await db.usageReading.deleteMany({
      where: { accountKey: { in: accountKeys }, observedAt: { lt: cutoff } },
    });
  } catch (error) {
    console.warn("[sync] pruning old usage readings failed", error);
  }
}

/** Claude Code's ids are lowercase; store them that way so one session is one row. */
function normalizedSessionId(session: SyncSession): string {
  return session.sessionId.toLowerCase();
}

function sessionKey(session: SyncSession): string {
  return `${session.accountKey}:${normalizedSessionId(session)}`;
}

function projectKey(session: SyncSession): string {
  return `${session.accountKey}:${session.project.key}`;
}

/** Unique by key, the last occurrence winning, in first-seen order. */
function lastByKey<T>(items: readonly T[], key: (item: T) => string): T[] {
  const byKey = new Map<string, T>();
  for (const item of items) {
    const k = key(item);
    byKey.delete(k);
    byKey.set(k, item);
  }
  return [...byKey.values()];
}

/** Unique by key, the first occurrence winning (as a stored reading is never replaced). */
function firstByKey<T>(items: readonly T[], key: (item: T) => string): T[] {
  const byKey = new Map<string, T>();
  for (const item of items) {
    const k = key(item);
    if (!byKey.has(k)) byKey.set(k, item);
  }
  return [...byKey.values()];
}

/**
 * Text as Postgres can store it: without NUL (which `text` and `jsonb` refuse, and which would
 * fail the whole batch on every retry) and without unpaired surrogates (not valid UTF-8).
 */
export function clean(text: string): string {
  return text
    .replace(/\u0000/g, "")
    .replace(
      /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/g,
      "\uFFFD",
    );
}

function cleanOrNull(text: string | null): string | null {
  return text === null ? null : clean(text);
}

/** Dates as the database stores them: UTC, millisecond precision. */
function isoMillis(date: string): string {
  return new Date(date).toISOString();
}

function capTokens(count: number): number {
  return Math.min(count, TOKEN_CAP);
}

function sessionRow(session: SyncSession) {
  const summary = session.summary ?? null;
  return {
    id: randomUUID(), // used only when the session is new
    accountKey: session.accountKey,
    projectKey: session.project.key,
    sessionId: normalizedSessionId(session),
    title: cleanOrNull(session.title),
    source: session.source,
    models: session.models.map(clean),
    startedAt: isoMillis(session.startedAt),
    lastActivityAt: isoMillis(session.lastActivityAt),
    endedAt: session.endedAt === null ? null : isoMillis(session.endedAt),
    messageCount: session.messageCount,
    inputTokens: capTokens(session.tokens.input),
    outputTokens: capTokens(session.tokens.output),
    cacheCreationTokens: capTokens(session.tokens.cacheCreation),
    cacheReadTokens: capTokens(session.tokens.cacheRead),
    // A JSON number keeps its decimal text (14.82 stays 14.82 in numeric(14, 6)).
    costUsd: session.costUsd,
    summaryText: summary ? clean(summary.text) : null,
    summaryModel: summary ? clean(summary.model) : null,
    summaryAt: summary ? isoMillis(summary.generatedAt) : null,
  };
}

/** One window of one usage reading: one `UsageReading` row. */
export type UsageRow = {
  accountKey: string;
  source: string;
  windowId: string;
  utilization: number;
  resetsAt: string | null;
  /** UTC, millisecond precision (as stored). */
  observedAt: string;
};

function usageRowsOf(reading: SyncRequest["usage"][number]): UsageRow[] {
  return reading.windows.map((window) => ({
    accountKey: reading.accountKey,
    source: reading.source,
    windowId: window.id,
    utilization: window.utilization,
    resetsAt: window.resetsAt === null ? null : isoMillis(window.resetsAt),
    observedAt: isoMillis(reading.observedAt),
  }));
}

/** The stored reading's unique key, less the user. */
export function usageRowKey(row: UsageRow): string {
  return `${row.accountKey}|${row.source}|${row.windowId}|${row.observedAt}`;
}
