/**
 * Who may see what, and who may change a pool. Every read on the website goes through here.
 *
 * Visibility (contract/README.md, "Pooling"):
 * - A user U sees a row (session, project, usage reading) owned by user V on Claude account K
 *   when V is U, or when some pool P on K has both U and V as members.
 * - Revoking a pool's code stops new joins only; members keep seeing each other's rows.
 * - U sees account K when U synced it (a UserAccount row) or is a member of a pool on K.
 *
 * Pools:
 * - Creating a code needs a UserAccount row for (U, K). Each (account, creator) has one pool with
 *   one current code: asking again returns it while it works, and otherwise issues a new code
 *   for the same pool (members stay).
 * - A code works for 7 days until it is revoked. Joining a pool you are in is a no-op; to anyone
 *   else an expired or revoked code looks exactly like an unknown one.
 * - The creator revokes the code and removes members (which revokes the code too, so a removed
 *   member can't come back with it); members leave; the creator leaving deletes the pool.
 *
 * Everything here is pure except `loadAccessScope`, which reads the two tables the rules need.
 */
import { type Db } from "~/server/db-types";

// ---------------------------------------------------------------------------------------------
// Visibility

/** A pool as the visibility rules see it. */
export type PoolMembership = {
  poolId: string;
  accountKey: string;
  memberIds: readonly string[];
  /** Who created the pool; their report describes the account to the other members. */
  createdById?: string;
};

/** What one viewer may see, computed once per request. */
export type AccessScope = {
  viewerId: string;
  /** Accounts the viewer synced themselves. */
  ownAccountKeys: ReadonlySet<string>;
  /**
   * Accounts the viewer is pooled on, each with every user whose rows the viewer sees there
   * (the viewer included).
   */
  pooledMembers: ReadonlyMap<string, ReadonlySet<string>>;
  /** The pools the viewer is in, by account. */
  poolIds: ReadonlyMap<string, readonly string[]>;
  /** Who created those pools, by account, in the order the pools were passed (oldest first). */
  poolCreators: ReadonlyMap<string, readonly string[]>;
};

/** A row that belongs to a user and a Claude account: a session, project or usage reading. */
export type OwnedRow = { userId: string; accountKey: string };

export function buildAccessScope(
  viewerId: string,
  ownAccountKeys: Iterable<string>,
  pools: readonly PoolMembership[],
): AccessScope {
  const pooledMembers = new Map<string, Set<string>>();
  const poolIds = new Map<string, string[]>();
  const poolCreators = new Map<string, string[]>();
  for (const pool of pools) {
    // Only pools the viewer is in count, whatever the caller passed.
    if (!pool.memberIds.includes(viewerId)) continue;
    if (pool.createdById !== undefined) {
      const creators = poolCreators.get(pool.accountKey) ?? [];
      if (!creators.includes(pool.createdById)) creators.push(pool.createdById);
      poolCreators.set(pool.accountKey, creators);
    }
    let members = pooledMembers.get(pool.accountKey);
    if (!members) {
      members = new Set([viewerId]);
      pooledMembers.set(pool.accountKey, members);
    }
    for (const id of pool.memberIds) members.add(id);
    poolIds.set(pool.accountKey, [
      ...(poolIds.get(pool.accountKey) ?? []),
      pool.poolId,
    ]);
  }
  return {
    viewerId,
    ownAccountKeys: new Set(ownAccountKeys),
    pooledMembers,
    poolIds,
    poolCreators,
  };
}

export function canSeeRow(scope: AccessScope, row: OwnedRow): boolean {
  if (row.userId === scope.viewerId) return true;
  return scope.pooledMembers.get(row.accountKey)?.has(row.userId) === true;
}

export function canSeeAccount(scope: AccessScope, accountKey: string): boolean {
  return (
    scope.ownAccountKeys.has(accountKey) || scope.pooledMembers.has(accountKey)
  );
}

/** Whether the viewer shares this account with at least one other person. */
export function isPooledWithOthers(
  scope: AccessScope,
  accountKey: string,
): boolean {
  return (scope.pooledMembers.get(accountKey)?.size ?? 0) > 1;
}

/** Every account the viewer can see, sorted. */
export function visibleAccountKeys(scope: AccessScope): string[] {
  return [
    ...new Set([...scope.ownAccountKeys, ...scope.pooledMembers.keys()]),
  ].sort();
}

/** Users whose rows on `accountKey` the viewer sees (always the viewer themself). */
export function visibleOwnerIds(
  scope: AccessScope,
  accountKey: string,
): string[] {
  const members = scope.pooledMembers.get(accountKey);
  return [...new Set([scope.viewerId, ...(members ?? [])])].sort();
}

/**
 * A Prisma `where` that matches exactly the rows `canSeeRow` allows. It fits Session, Project
 * and UsageReading, which all carry `userId` and `accountKey`.
 */
export type OwnedRowWhere =
  | { accountKey: string; userId: { in: string[] } }
  | {
      OR: Array<
        { userId: string } | { accountKey: string; userId: { in: string[] } }
      >;
    };

export function ownedRowWhere(
  scope: AccessScope,
  filter: { accountKey?: string } = {},
): OwnedRowWhere {
  if (filter.accountKey !== undefined) {
    return {
      accountKey: filter.accountKey,
      userId: { in: visibleOwnerIds(scope, filter.accountKey) },
    };
  }
  const pooled = [...scope.pooledMembers.entries()]
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([accountKey, members]) => ({
      accountKey,
      userId: { in: [...members].sort() },
    }));
  return { OR: [{ userId: scope.viewerId }, ...pooled] };
}

/** Reads what the rules need about one viewer: their synced accounts and their pools. */
export async function loadAccessScope(
  db: Db,
  viewerId: string,
): Promise<AccessScope> {
  const [own, memberships] = await Promise.all([
    db.userAccount.findMany({
      where: { userId: viewerId },
      select: { accountKey: true },
    }),
    db.poolMember.findMany({
      where: { userId: viewerId },
      // Oldest pool first, so "the creator's report" is stable when there are several.
      orderBy: [{ pool: { createdAt: "asc" } }, { poolId: "asc" }],
      select: {
        pool: {
          select: {
            id: true,
            accountKey: true,
            createdById: true,
            members: { select: { userId: true } },
          },
        },
      },
    }),
  ]);
  return buildAccessScope(
    viewerId,
    own.map((row) => row.accountKey),
    memberships.map(({ pool }) => ({
      poolId: pool.id,
      accountKey: pool.accountKey,
      memberIds: pool.members.map((m) => m.userId),
      createdById: pool.createdById,
    })),
  );
}

// ---------------------------------------------------------------------------------------------
// Pool rules

/** A pool as the pool rules see it. */
export type PoolFacts = {
  id: string;
  accountKey: string;
  createdById: string;
  /** When the current code stops working. */
  codeExpiresAt: Date;
  /** When the current code was revoked, if it was. */
  revokedAt: Date | null;
  memberIds: readonly string[];
};

/** Whether the pool's current code can still be redeemed at `now`. */
export function codeIsActive(pool: PoolFacts, now: Date): boolean {
  return (
    pool.revokedAt === null && pool.codeExpiresAt.getTime() > now.getTime()
  );
}

export type DeniedCode =
  "FORBIDDEN" | "NOT_FOUND" | "BAD_REQUEST" | "TOO_MANY_REQUESTS";
export type Denied = { ok: false; code: DeniedCode; message: string };
export type Allowed<A extends string> = { ok: true; action: A };

/** A refusal from these rules, carried up to the tRPC layer (which maps `code`). */
export class AccessDenied extends Error {
  readonly code: DeniedCode;
  constructor(code: DeniedCode, message: string) {
    super(message);
    this.name = "AccessDenied";
    this.code = code;
  }
}

export function enforce<A extends string>(
  decision: Allowed<A> | Denied,
): Allowed<A> {
  if (!decision.ok) throw new AccessDenied(decision.code, decision.message);
  return decision;
}

const deny = (code: DeniedCode, message: string): Denied => ({
  ok: false,
  code,
  message,
});

// A pool the viewer is not in is reported as missing, so its existence doesn't leak.
const NO_SUCH_POOL = deny("NOT_FOUND", "No such pool.");

export function decideCreatePool(
  viewerId: string,
  facts: {
    /** Whether (viewer, account) has a UserAccount row. */
    viewerSyncedAccount: boolean;
    /** The viewer's own pool on this account, if they made one. */
    ownPool: PoolFacts | null;
  },
  now: Date,
): Allowed<"create" | "reuse" | "renew"> | Denied {
  if (!facts.viewerSyncedAccount) {
    return deny(
      "FORBIDDEN",
      "Sync this Claude account from the Agent Notch app before sharing it.",
    );
  }
  const own = facts.ownPool;
  if (own?.createdById !== viewerId) return { ok: true, action: "create" };
  // The same pool gets a new code, so the people already in it stay together.
  return codeIsActive(own, now)
    ? { ok: true, action: "reuse" }
    : { ok: true, action: "renew" };
}

// Unknown, expired and revoked codes all get this answer, so a guess learns nothing.
const NO_SUCH_CODE = deny(
  "NOT_FOUND",
  "No pool uses that code. It may have expired or been revoked.",
);

export function decideJoin(
  viewerId: string,
  pool: PoolFacts | null,
  now: Date,
): Allowed<"join" | "already-member"> | Denied {
  if (!pool) return NO_SUCH_CODE;
  // Membership outlives the code, so a member redeeming it again is fine even after it lapsed.
  if (pool.memberIds.includes(viewerId)) {
    return { ok: true, action: "already-member" };
  }
  if (!codeIsActive(pool, now)) return NO_SUCH_CODE;
  return { ok: true, action: "join" };
}

export function decideRevoke(
  viewerId: string,
  pool: PoolFacts | null,
): Allowed<"revoke" | "already-revoked"> | Denied {
  if (!pool?.memberIds.includes(viewerId)) return NO_SUCH_POOL;
  if (pool.createdById !== viewerId) {
    return deny(
      "FORBIDDEN",
      "Only the person who created the code can revoke it.",
    );
  }
  if (pool.revokedAt !== null) return { ok: true, action: "already-revoked" };
  return { ok: true, action: "revoke" };
}

export function decideRemoveMember(
  viewerId: string,
  pool: PoolFacts | null,
  memberId: string,
): Allowed<"remove" | "not-a-member"> | Denied {
  if (!pool?.memberIds.includes(viewerId)) return NO_SUCH_POOL;
  if (pool.createdById !== viewerId) {
    return deny(
      "FORBIDDEN",
      "Only the person who created the pool can remove members.",
    );
  }
  if (memberId === viewerId) {
    return deny(
      "BAD_REQUEST",
      "You can't remove yourself. Leave the pool to delete it.",
    );
  }
  if (!pool.memberIds.includes(memberId))
    return { ok: true, action: "not-a-member" };
  return { ok: true, action: "remove" };
}

export function decideLeave(
  viewerId: string,
  pool: PoolFacts | null,
): Allowed<"leave" | "delete-pool"> | Denied {
  if (!pool?.memberIds.includes(viewerId)) return NO_SUCH_POOL;
  if (pool.createdById === viewerId) return { ok: true, action: "delete-pool" };
  return { ok: true, action: "leave" };
}
