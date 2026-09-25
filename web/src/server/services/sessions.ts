/**
 * Sessions the viewer can see: their own, and pool members' on shared accounts.
 */
import { type Db, type Prisma } from "~/server/db-types";
import {
  AccessDenied,
  canSeeAccount,
  canSeeRow,
  ownedRowWhere,
  type AccessScope,
} from "~/server/services/access";
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
  projectId?: string;
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
  /** The `id` of the last item of the previous page. */
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

const sessionInclude = {
  project: { select: { id: true, name: true } },
  device: { select: { id: true, name: true } },
} satisfies Prisma.SessionInclude;

type SessionRow = Prisma.SessionGetPayload<{ include: typeof sessionInclude }>;

export async function listSessions(
  db: Db,
  scope: AccessScope,
  filter: SessionListFilter,
): Promise<SessionPage> {
  if (
    filter.accountKey !== undefined &&
    !canSeeAccount(scope, filter.accountKey)
  ) {
    throw new AccessDenied("NOT_FOUND", "No such account.");
  }

  const conditions: Prisma.SessionWhereInput[] = [
    ownedRowWhere(scope, { accountKey: filter.accountKey }),
  ];
  if (filter.projectId) conditions.push({ projectId: filter.projectId });
  if (filter.ownerId) conditions.push({ userId: filter.ownerId });
  if (filter.source) conditions.push({ source: filter.source });
  if (filter.from) conditions.push({ startedAt: { gte: filter.from } });
  if (filter.to) conditions.push({ startedAt: { lt: filter.to } });
  if (filter.running === true) conditions.push({ endedAt: null });
  if (filter.running === false) conditions.push({ endedAt: { not: null } });
  const search = filter.search?.trim();
  if (search) {
    conditions.push({
      OR: [
        { title: { contains: search, mode: "insensitive" } },
        { summaryText: { contains: search, mode: "insensitive" } },
        { project: { name: { contains: search, mode: "insensitive" } } },
      ],
    });
  }

  const rows = await db.session.findMany({
    where: { AND: conditions },
    include: sessionInclude,
    orderBy: [{ startedAt: "desc" }, { id: "desc" }],
    take: filter.limit + 1,
    ...(filter.cursor ? { cursor: { id: filter.cursor }, skip: 1 } : {}),
  });

  const page = rows.slice(0, filter.limit);
  const people = await peopleById(
    db,
    scope.viewerId,
    page.map((r) => r.userId),
  );
  return {
    items: page.map((row) => toItem(row, scope, people)),
    nextCursor: rows.length > filter.limit ? (page.at(-1)?.id ?? null) : null,
  };
}

export async function getSession(
  db: Db,
  scope: AccessScope,
  id: string,
): Promise<SessionItem> {
  const row = await db.session.findUnique({
    where: { id },
    include: sessionInclude,
  });
  if (!row || !canSeeRow(scope, row)) {
    throw new AccessDenied("NOT_FOUND", "No such session.");
  }
  const people = await peopleById(db, scope.viewerId, [row.userId]);
  return toItem(row, scope, people);
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
