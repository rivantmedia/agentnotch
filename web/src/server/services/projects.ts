/**
 * Projects (working directories, by name only) on one account, with their totals.
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
  SESSION_SUMS,
  tokenTotals,
  type Person,
  type TokenTotals,
} from "~/server/services/totals";

/**
 * A project as the website shows it. Its key (an HMAC of the path, made on the Mac) is left out
 * on purpose: it identifies nothing a page needs, and nobody is ever shown it (contract/README.md).
 */
export type ProjectSummary = {
  id: string;
  name: string;
  accountKey: string;
  owner: Person;
  sessionCount: number;
  tokens: TokenTotals;
  costUsd: number | null;
  firstUsedAt: Date | null;
  lastUsedAt: Date | null;
  /** The most recent session summaries, newest first (only when summaries were turned on). */
  latestSummaries: Array<{
    sessionRowId: string;
    title: string | null;
    text: string;
    model: string | null;
    generatedAt: Date | null;
  }>;
};

const SUMMARIES_PER_PROJECT = 3;

const projectInclude = {
  sessions: {
    where: { summaryText: { not: null } },
    orderBy: [{ summaryAt: "desc" }, { lastActivityAt: "desc" }],
    take: SUMMARIES_PER_PROJECT,
    select: {
      id: true,
      title: true,
      summaryText: true,
      summaryModel: true,
      summaryAt: true,
    },
  },
} satisfies Prisma.ProjectInclude;

type ProjectRow = Prisma.ProjectGetPayload<{ include: typeof projectInclude }>;

export async function listProjects(
  db: Db,
  scope: AccessScope,
  accountKey: string,
): Promise<ProjectSummary[]> {
  if (!canSeeAccount(scope, accountKey)) {
    throw new AccessDenied("NOT_FOUND", "No such account.");
  }
  const rows = await db.project.findMany({
    where: ownedRowWhere(scope, { accountKey }),
    include: projectInclude,
  });
  const summaries = await summarize(db, scope, rows);
  return summaries.sort(
    (a, b) =>
      (b.lastUsedAt?.getTime() ?? 0) - (a.lastUsedAt?.getTime() ?? 0) ||
      a.name.localeCompare(b.name),
  );
}

export async function getProject(
  db: Db,
  scope: AccessScope,
  id: string,
): Promise<ProjectSummary> {
  const row = await db.project.findUnique({
    where: { id },
    include: projectInclude,
  });
  if (!row || !canSeeRow(scope, row)) {
    throw new AccessDenied("NOT_FOUND", "No such project.");
  }
  const [summary] = await summarize(db, scope, [row]);
  return summary!;
}

async function summarize(
  db: Db,
  scope: AccessScope,
  rows: ProjectRow[],
): Promise<ProjectSummary[]> {
  if (rows.length === 0) return [];
  const [stats, people] = await Promise.all([
    db.session.groupBy({
      by: ["projectId"],
      where: {
        AND: [
          ownedRowWhere(scope),
          { projectId: { in: rows.map((r) => r.id) } },
        ],
      },
      _count: { _all: true },
      _sum: SESSION_SUMS,
      _min: { startedAt: true },
      _max: { lastActivityAt: true },
    }),
    peopleById(
      db,
      scope.viewerId,
      rows.map((r) => r.userId),
    ),
  ]);
  return rows.map((row) => {
    const stat = stats.find((s) => s.projectId === row.id);
    return {
      id: row.id,
      name: row.name,
      accountKey: row.accountKey,
      owner: personOrUnknown(people, row.userId, scope.viewerId),
      sessionCount: stat?._count._all ?? 0,
      tokens: tokenTotals(stat?._sum),
      costUsd: decimalToNumber(stat?._sum.costUsd),
      firstUsedAt: stat?._min.startedAt ?? null,
      lastUsedAt: stat?._max.lastActivityAt ?? null,
      latestSummaries: row.sessions.flatMap((s) =>
        s.summaryText
          ? [
              {
                sessionRowId: s.id,
                title: s.title,
                text: s.summaryText,
                model: s.summaryModel,
                generatedAt: s.summaryAt,
              },
            ]
          : [],
      ),
    };
  });
}
