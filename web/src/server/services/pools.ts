/**
 * Pools: sharing one Claude account's sessions and usage between people who use it.
 * The rules live in access.ts; this file loads the facts they need and applies the outcome.
 *
 * Codes: one pool per (account, creator), with one current code that works for 7 days. When it
 * lapses or is revoked, asking for a code issues a new one for the same pool, so the people in
 * it stay together. Removing a member revokes the current code, so they can't rejoin with it.
 * Failed redemptions are counted per user (PoolJoinFailure) and cap how fast anyone can guess.
 */
import { type Db, type Prisma, type PrismaClient } from "~/server/db-types";
import {
  AccessDenied,
  codeIsActive,
  decideCreatePool,
  decideJoin,
  decideLeave,
  decideRemoveMember,
  decideRevoke,
  enforce,
  type Denied,
  type PoolFacts,
} from "~/server/services/access";
import {
  generatePoolCode,
  normalizePoolCode,
  POOL_CODE_EXAMPLE,
  POOL_CODE_TTL_MS,
  type RandomBytes,
} from "~/server/services/pool-code";
import { memberLabel } from "~/server/services/users";

/** At most this many failed redemptions per user in JOIN_FAILURE_WINDOW_MS; then no more tries. */
export const JOIN_FAILURE_LIMIT = 10;
export const JOIN_FAILURE_WINDOW_MS = 60 * 60 * 1000;

export type PoolMemberView = {
  userId: string;
  /** Their email (Google-verified): what tells members apart. */
  displayName: string;
  /** The name on their Google account, shown next to the email; they can change it. */
  name: string | null;
  joinedAt: Date;
  isCreator: boolean;
  isViewer: boolean;
};

/** The pool's current code, as its creator sees it. */
export type PoolShare = {
  /** The code, while it can be redeemed; null once it expired or was revoked. */
  code: string | null;
  status: "active" | "expired" | "revoked";
  expiresAt: Date;
  revokedAt: Date | null;
};

export type PoolView = {
  id: string;
  accountKey: string;
  account: {
    email: string | null;
    organizationName: string | null;
    plan: string | null;
    /** The viewer's own name for the account, if their app synced it. */
    label: string | null;
  };
  role: "creator" | "member";
  /** The code and its state; only the creator sees it. */
  share: PoolShare | null;
  createdAt: Date;
  createdBy: { id: string; displayName: string; name: string | null };
  members: PoolMemberView[];
};

const poolWithMembers = {
  members: {
    // Same-millisecond joins still list in a stable order.
    orderBy: [{ joinedAt: "asc" as const }, { userId: "asc" as const }],
    select: {
      userId: true,
      joinedAt: true,
      user: { select: { name: true, email: true } },
    },
  },
  createdBy: { select: { id: true, name: true, email: true } },
} satisfies Prisma.PoolInclude;

export async function listPools(
  db: Db,
  viewerId: string,
  now: Date = new Date(),
): Promise<PoolView[]> {
  const memberships = await db.poolMember.findMany({
    where: { userId: viewerId },
    select: { pool: { include: poolWithMembers } },
  });
  const pools = memberships.map((m) => m.pool);
  if (pools.length === 0) return [];

  // How to describe each account: the viewer's own report, else the creator's. Never another
  // member's: what a member's app reports is shown to that member only.
  const keys = [...new Set(pools.map((p) => p.accountKey))];
  const reports = await db.userAccount.findMany({
    where: {
      accountKey: { in: keys },
      userId: { in: [viewerId, ...new Set(pools.map((p) => p.createdById))] },
    },
  });

  return pools
    .map((pool): PoolView => {
      const own = reports.find(
        (r) => r.accountKey === pool.accountKey && r.userId === viewerId,
      );
      const creators = reports.find(
        (r) =>
          r.accountKey === pool.accountKey && r.userId === pool.createdById,
      );
      const report = own ?? creators;
      const isCreator = pool.createdById === viewerId;
      return {
        id: pool.id,
        accountKey: pool.accountKey,
        account: {
          email: report?.email ?? null,
          organizationName: report?.organizationName ?? null,
          plan: report?.plan ?? null,
          label: own?.label ?? null,
        },
        role: isCreator ? "creator" : "member",
        share: isCreator ? shareOf(pool, now) : null,
        createdAt: pool.createdAt,
        createdBy: {
          id: pool.createdBy.id,
          ...memberLabel(pool.createdBy),
        },
        members: pool.members.map((m) => ({
          userId: m.userId,
          ...memberLabel(m.user),
          joinedAt: m.joinedAt,
          isCreator: m.userId === pool.createdById,
          isViewer: m.userId === viewerId,
        })),
      };
    })
    .sort((a, b) => b.createdAt.getTime() - a.createdAt.getTime());
}

function shareOf(
  pool: { code: string; codeExpiresAt: Date; revokedAt: Date | null },
  now: Date,
): PoolShare {
  const status =
    pool.revokedAt !== null
      ? "revoked"
      : pool.codeExpiresAt.getTime() > now.getTime()
        ? "active"
        : "expired";
  return {
    code: status === "active" ? pool.code : null,
    status,
    expiresAt: pool.codeExpiresAt,
    revokedAt: pool.revokedAt,
  };
}

/**
 * The viewer's working code for the account: the current one while it works, else a new one for
 * the same pool, else a new pool (with the viewer as its first member).
 */
export async function createPoolCode(
  db: PrismaClient,
  viewerId: string,
  accountKey: string,
  random?: RandomBytes,
  now: Date = new Date(),
): Promise<{
  poolId: string;
  code: string;
  expiresAt: Date;
  created: boolean;
}> {
  return db.$transaction(async (tx) => {
    // Serialises concurrent requests for the same (account, creator), so there is never more
    // than one pool or code for it. Held until the transaction ends.
    await tx.$executeRaw`SELECT pg_advisory_xact_lock(hashtextextended(${`pool:${accountKey}:${viewerId}`}, 0))`;

    const [synced, own] = await Promise.all([
      tx.userAccount.findUnique({
        where: { userId_accountKey: { userId: viewerId, accountKey } },
        select: { userId: true },
      }),
      tx.pool.findUnique({
        where: {
          accountKey_createdById: { accountKey, createdById: viewerId },
        },
        include: { members: { select: { userId: true } } },
      }),
    ]);
    const decision = enforce(
      decideCreatePool(
        viewerId,
        {
          viewerSyncedAccount: synced !== null,
          ownPool: own ? toFacts(own) : null,
        },
        now,
      ),
    );
    if (decision.action === "reuse" && own) {
      return {
        poolId: own.id,
        code: own.code,
        expiresAt: own.codeExpiresAt,
        created: false,
      };
    }

    const expiresAt = new Date(now.getTime() + POOL_CODE_TTL_MS);
    const code = await freeCode(tx, random);
    if (decision.action === "renew" && own) {
      // The old code stops existing: nobody can redeem it again, even a removed member.
      await tx.pool.update({
        where: { id: own.id },
        data: { code, codeExpiresAt: expiresAt, revokedAt: null },
      });
      return { poolId: own.id, code, expiresAt, created: true };
    }
    const pool = await tx.pool.create({
      data: {
        accountKey,
        createdById: viewerId,
        code,
        codeExpiresAt: expiresAt,
        members: { create: { userId: viewerId } },
      },
      select: { id: true },
    });
    return { poolId: pool.id, code, expiresAt, created: true };
  });
}

async function freeCode(
  tx: Prisma.TransactionClient,
  random?: RandomBytes,
): Promise<string> {
  for (let attempt = 0; attempt < 5; attempt++) {
    const code = generatePoolCode(random);
    const taken = await tx.pool.findUnique({
      where: { code },
      select: { id: true },
    });
    if (!taken) return code;
  }
  throw new Error("No free pool code after 5 attempts");
}

/**
 * Redeems a code. Unknown, expired and revoked codes get the same NOT_FOUND to anyone who isn't
 * a member, and each such failure counts: after JOIN_FAILURE_LIMIT in an hour, every attempt is
 * refused (right codes included, or the refusal itself would tell right from wrong).
 */
export async function joinPool(
  db: PrismaClient,
  viewerId: string,
  typedCode: string,
  now: Date = new Date(),
): Promise<{ poolId: string; accountKey: string; joined: boolean }> {
  const code = normalizePoolCode(typedCode);
  if (!code) {
    throw new AccessDenied(
      "BAD_REQUEST",
      `A code is 12 letters and digits, like ${POOL_CODE_EXAMPLE}.`,
    );
  }

  const outcome = await db.$transaction(
    async (
      tx,
    ): Promise<
      | { ok: true; poolId: string; accountKey: string; joined: boolean }
      | { ok: false; denied: Denied }
    > => {
      // One attempt per user at a time, so the failure count is exact.
      await tx.$executeRaw`SELECT pg_advisory_xact_lock(hashtextextended(${`pool-join:${viewerId}`}, 0))`;
      const windowStart = new Date(now.getTime() - JOIN_FAILURE_WINDOW_MS);
      // The table only ever needs the last hour.
      await tx.poolJoinFailure.deleteMany({
        where: { at: { lte: windowStart } },
      });
      const failures = await tx.poolJoinFailure.count({
        where: { userId: viewerId, at: { gt: windowStart } },
      });
      if (failures >= JOIN_FAILURE_LIMIT) {
        return {
          ok: false,
          denied: {
            ok: false,
            code: "TOO_MANY_REQUESTS",
            message:
              "Too many codes that didn't work. Wait an hour, then try again.",
          },
        };
      }

      // Locks the pool, so a removal (which revokes the code) can't interleave with this join.
      const locked = await tx.$queryRaw<Array<{ id: string }>>`
        SELECT "id" FROM "Pool" WHERE "code" = ${code} FOR UPDATE`;
      const pool = locked[0]
        ? await tx.pool.findUnique({
            where: { id: locked[0].id },
            include: { members: { select: { userId: true } } },
          })
        : null;
      const decision = decideJoin(viewerId, pool ? toFacts(pool) : null, now);
      if (!decision.ok) {
        await tx.poolJoinFailure.create({
          data: { userId: viewerId, at: now },
        });
        return { ok: false, denied: decision };
      }
      if (decision.action === "join") {
        // skipDuplicates: a double click is still one membership.
        await tx.poolMember.createMany({
          data: [{ poolId: pool!.id, userId: viewerId }],
          skipDuplicates: true,
        });
      }
      return {
        ok: true,
        poolId: pool!.id,
        accountKey: pool!.accountKey,
        joined: decision.action === "join",
      };
    },
  );

  // Thrown after the commit, so the failure is recorded.
  if (!outcome.ok) {
    throw new AccessDenied(outcome.denied.code, outcome.denied.message);
  }
  const { poolId, accountKey, joined } = outcome;
  return { poolId, accountKey, joined };
}

export async function revokePoolCode(
  db: PrismaClient,
  viewerId: string,
  poolId: string,
  now: Date = new Date(),
): Promise<{ revoked: boolean }> {
  return db.$transaction(async (tx) => {
    const decision = enforce(
      decideRevoke(viewerId, await lockAndLoadFacts(tx, poolId)),
    );
    if (decision.action === "revoke") {
      await tx.pool.updateMany({
        where: { id: poolId, revokedAt: null },
        data: { revokedAt: now },
      });
    }
    return { revoked: decision.action === "revoke" };
  });
}

/**
 * Removes a member and revokes the pool's current code in the same transaction, so the removed
 * person can't come straight back with it. A new code has to be created to invite anyone else.
 */
export async function removePoolMember(
  db: PrismaClient,
  viewerId: string,
  poolId: string,
  memberId: string,
  now: Date = new Date(),
): Promise<{ removed: boolean; codeRevoked: boolean }> {
  return db.$transaction(async (tx) => {
    const facts = await lockAndLoadFacts(tx, poolId);
    const decision = enforce(decideRemoveMember(viewerId, facts, memberId));
    if (decision.action !== "remove") {
      return { removed: false, codeRevoked: false };
    }
    const codeRevoked = facts !== null && codeIsActive(facts, now);
    if (codeRevoked) {
      await tx.pool.updateMany({
        where: { id: poolId, revokedAt: null },
        data: { revokedAt: now },
      });
    }
    await tx.poolMember.deleteMany({ where: { poolId, userId: memberId } });
    return { removed: true, codeRevoked };
  });
}

export async function leavePool(
  db: PrismaClient,
  viewerId: string,
  poolId: string,
): Promise<{ deletedPool: boolean }> {
  return db.$transaction(async (tx) => {
    const decision = enforce(
      decideLeave(viewerId, await lockAndLoadFacts(tx, poolId)),
    );
    if (decision.action === "delete-pool") {
      // Members go with it (ON DELETE CASCADE).
      await tx.pool.deleteMany({
        where: { id: poolId, createdById: viewerId },
      });
      return { deletedPool: true };
    }
    await tx.poolMember.deleteMany({ where: { poolId, userId: viewerId } });
    return { deletedPool: false };
  });
}

/** The pool's facts, with its row locked until the transaction ends. */
async function lockAndLoadFacts(
  tx: Prisma.TransactionClient,
  poolId: string,
): Promise<PoolFacts | null> {
  const locked = await tx.$queryRaw<Array<{ id: string }>>`
    SELECT "id" FROM "Pool" WHERE "id" = ${poolId} FOR UPDATE`;
  if (locked.length === 0) return null;
  const pool = await tx.pool.findUnique({
    where: { id: poolId },
    include: { members: { select: { userId: true } } },
  });
  return pool ? toFacts(pool) : null;
}

function toFacts(pool: {
  id: string;
  accountKey: string;
  createdById: string;
  codeExpiresAt: Date;
  revokedAt: Date | null;
  members: { userId: string }[];
}): PoolFacts {
  return {
    id: pool.id,
    accountKey: pool.accountKey,
    createdById: pool.createdById,
    codeExpiresAt: pool.codeExpiresAt,
    revokedAt: pool.revokedAt,
    memberIds: pool.members.map((m) => m.userId),
  };
}
