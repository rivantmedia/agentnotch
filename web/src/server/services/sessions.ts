/**
 * Sessions the viewer can see: their own, and pool members' on shared accounts.
 *
 * Every time a session carries is clamped to the server's clock, in the SQL: a session dated in
 * the future (a Mac whose clock runs ahead) counts as now, never later, both where it sorts and
 * in the times it shows, its summary's time included.
 *
 * Paging walks one snapshot of that clock: the first page's "now" travels in its cursor and every
 * later page clamps against it, so a session dated in the future sorts in the same place on every
 * page, however much later the next one loads, and no row is skipped or shown twice.
 */
import { Prisma, type Db } from "~/server/db-types";
import {
  AccessDenied,
  canSeeAccount,
  ownedRowWhere,
  type AccessScope,
} from "~/server/services/access";
import {
  clampedTime,
  containsPattern,
  ownedRowSql,
  sqlTime,
} from "~/server/services/sql";
import {
  decimalToNumber,
  peopleById,
  personOrUnknown,
  tokenTotals,
  type Person,
  type TokenTotals,
} from "~/server/services/totals";

export type SessionListFilter = {
  accountKey?: string;
  /**
   * Sessions in this project and in its owner's other projects of the same name on the same
   * account: the same folder name on their other Macs (projects.ts groups them the same way).
   */
  projectId?: string;
  /**
   * With `projectId`: the owner's folders of that name on every account the viewer sees them
   * on, not only on the project row's own account (the project page, project-usage.ts).
   */
  acrossAccounts?: boolean;
  /** Only sessions of this person (the viewer or a pool member). */
  ownerId?: string;
  source?: string;
  /** Started at or after. */
  from?: Date;
  /** Started before. */
  to?: Date;
  /** true: still running (no end time); false: ended. */
  running?: boolean;
  /** Matched against title, project name and summary, case-insensitively. */
  search?: string;
  /** The previous page's `nextCursor` (`sessionCursor`). */
  cursor?: string;
  limit: number;
};

export type SessionItem = {
  id: string;
  sessionId: string;
  accountKey: string;
  project: { id: string; name: string };
  title: string | null;
  source: string;
  models: string[];
  /** Never later than the server's clock (nor are the next two). */
  startedAt: Date;
  lastActivityAt: Date;
  endedAt: Date | null;
  messageCount: number;
  tokens: TokenTotals;
  costUsd: number | null;
  summary: {
    text: string;
    model: string | null;
    generatedAt: Date | null;
  } | null;
  owner: Person;
  /** The Mac it ran on; only for the viewer's own sessions. */
  device: { id: string; name: string } | null;
};

export type SessionPage = { items: SessionItem[]; nextCursor: string | null };

/**
 * A page cursor: the moment the walk's first page was read (epoch milliseconds), a dot, and the
 * id of the page's last row.
 */
const CURSOR = /^(\d{1,15})\.(.+)$/s;

export function sessionCursor(asOf: Date, id: string): string {
  return `${asOf.getTime()}.${id}`;
}

export function parseSessionCursor(
  cursor: string,
): { asOf: Date; id: string } | null {
  const match = CURSOR.exec(cursor);
  return match ? { asOf: new Date(Number(match[1])), id: match[2]! } : null;
}

const sessionInclude = {
  project: { select: { id: true, name: true } },
  device: { select: { id: true, name: true } },
} satisfies Prisma.SessionInclude;

type SessionRow = Prisma.SessionGetPayload<{ include: typeof sessionInclude }>;

/** A session's id with its times clamped to the server's clock. */
type ClampedTimes = {
  id: string;
  startedAt: Date;
  lastActivityAt: Date;
  endedAt: Date | null;
  summaryAt: Date | null;
};

function clampedColumns(now: Prisma.Sql): Prisma.Sql {
  return Prisma.sql`s."id",
    LEAST(s."startedAt", ${now}) AS "startedAt",
    LEAST(s."lastActivityAt", ${now}) AS "lastActivityAt",
    ${clampedTime(Prisma.sql`s."endedAt"`, now)} AS "endedAt",
    ${clampedTime(Prisma.sql`s."summaryAt"`, now)} AS "summaryAt"`;
}

/**
 * The ids of every project in `projectId`'s group: same owner, account and name, or with
 * `acrossAccounts` same owner and name on any account. The row named must be one the viewer
 * sees, so an id of someone's unshared project can't select their shared ones of that name.
 */
function projectGroupIds(
  projectId: string,
  acrossAccounts: boolean,
  visible: Prisma.Sql,
): Prisma.Sql {
  const sameAccount = acrossAccounts
    ? Prisma.empty
    : Prisma.sql`AND g."accountKey" = p."accountKey"`;
  return Prisma.sql`
    SELECT g."id" FROM "Project" g
    JOIN "Project" p
      ON g."userId" = p."userId" AND g."name" = p."name" ${sameAccount}
    WHERE p."id" = ${projectId} AND ${visible}`;
}

export async function listSessions(
  db: Db,
  scope: AccessScope,
  filter: SessionListFilter,
  now: Date = new Date(),
): Promise<SessionPage> {
  if (
    filter.accountKey !== undefined &&
    !canSeeAccount(scope, filter.accountKey)
  ) {
    throw new AccessDenied("NOT_FOUND", "No such account.");
  }

  // The walk's clock: the first page's now, carried by the cursor (never later than this now).
  const cursor =
    filter.cursor === undefined ? null : parseSessionCursor(filter.cursor);
  if (filter.cursor !== undefined && cursor === null) {
    throw new AccessDenied("BAD_REQUEST", "That isn't a page of sessions.");
  }
  const asOf = cursor && cursor.asOf < now ? cursor.asOf : now;
  const at = sqlTime(asOf);
  // Newest first by start, a future start counting as now; ties by id, as the cursor expects.
  const startedAt = Prisma.sql`LEAST(s."startedAt", ${at})`;
  const conditions: Prisma.Sql[] = [
    ownedRowSql(ownedRowWhere(scope, { accountKey: filter.accountKey }), "s"),
  ];
  if (filter.projectId) {
    const group = projectGroupIds(
      filter.projectId,
      filter.acrossAccounts === true,
      ownedRowSql(ownedRowWhere(scope), "p"),
    );
    conditions.push(Prisma.sql`s."projectId" IN (${group})`);
  }
  if (filter.ownerId)
    conditions.push(Prisma.sql`s."userId" = ${filter.ownerId}`);
  if (filter.source) conditions.push(Prisma.sql`s."source" = ${filter.source}`);
  if (filter.from) {
    conditions.push(Prisma.sql`${startedAt} >= ${sqlTime(filter.from)}`);
  }
  if (filter.to)
    conditions.push(Prisma.sql`${startedAt} < ${sqlTime(filter.to)}`);
  if (filter.running === true) conditions.push(Prisma.sql`s."endedAt" IS NULL`);
  if (filter.running === false) {
    conditions.push(Prisma.sql`s."endedAt" IS NOT NULL`);
  }
  const search = filter.search?.trim();
  if (search) {
    const pattern = containsPattern(search);
    conditions.push(Prisma.sql`(
      s."title" ILIKE ${pattern}
      OR s."summaryText" ILIKE ${pattern}
      OR EXISTS (
        SELECT 1 FROM "Project" sp WHERE sp."id" = s."projectId" AND sp."name" ILIKE ${pattern}))`);
  }
  if (cursor) {
    // After the cursor's place in the same order. An unknown row matches nothing.
    conditions.push(Prisma.sql`(${startedAt}, s."id") < (
      SELECT LEAST(c."startedAt", ${at}), c."id" FROM "Session" c WHERE c."id" = ${cursor.id})`);
  }

  const rows = await db.$queryRaw<ClampedTimes[]>`
    SELECT ${clampedColumns(at)}
    FROM "Session" s
    WHERE ${Prisma.join(conditions, " AND ")}
    ORDER BY ${startedAt} DESC, s."id" DESC
    LIMIT ${filter.limit + 1}`;

  const page = rows.slice(0, filter.limit);
  const last = page.at(-1);
  return {
    items: await toItems(db, scope, page),
    nextCursor:
      rows.length > filter.limit && last ? sessionCursor(asOf, last.id) : null,
  };
}

export async function getSession(
  db: Db,
  scope: AccessScope,
  id: string,
  now: Date = new Date(),
): Promise<SessionItem> {
  const rows = await db.$queryRaw<ClampedTimes[]>`
    SELECT ${clampedColumns(sqlTime(now))}
    FROM "Session" s
    WHERE s."id" = ${id} AND ${ownedRowSql(ownedRowWhere(scope), "s")}`;
  const [item] = await toItems(db, scope, rows);
  if (!item) throw new AccessDenied("NOT_FOUND", "No such session.");
  return item;
}

/** The sessions behind `rows`, in their order, carrying the clamped times. */
async function toItems(
  db: Db,
  scope: AccessScope,
  rows: readonly ClampedTimes[],
): Promise<SessionItem[]> {
  if (rows.length === 0) return [];
  const full = await db.session.findMany({
    where: { id: { in: rows.map((r) => r.id) } },
    include: sessionInclude,
  });
  const byId = new Map(full.map((row) => [row.id, row]));
  const people = await peopleById(
    db,
    scope.viewerId,
    full.map((r) => r.userId),
  );
  return rows.flatMap((times) => {
    const row = byId.get(times.id);
    // Deleted between the two reads: left out.
    return row ? [toItem({ ...row, ...times }, scope, people)] : [];
  });
}

function toItem(
  row: SessionRow,
  scope: AccessScope,
  people: Map<string, Person>,
): SessionItem {
  const own = row.userId === scope.viewerId;
  return {
    id: row.id,
    sessionId: row.sessionId,
    accountKey: row.accountKey,
    project: row.project,
    title: row.title,
    source: row.source,
    models: row.models,
    startedAt: row.startedAt,
    lastActivityAt: row.lastActivityAt,
    endedAt: row.endedAt,
    messageCount: row.messageCount,
    tokens: tokenTotals(row),
    costUsd: decimalToNumber(row.costUsd),
    summary: row.summaryText
      ? {
          text: row.summaryText,
          model: row.summaryModel,
          generatedAt: row.summaryAt,
        }
      : null,
    owner: personOrUnknown(people, row.userId, scope.viewerId),
    // Which Mac a pool member used is theirs to know, not the pool's.
    device: own && row.device ? row.device : null,
  };
}
