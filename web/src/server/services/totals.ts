/**
 * Shapes shared by the dashboard read services.
 */
import { type Prisma, type Db } from "~/server/db-types";
import { memberLabel } from "~/server/services/users";

export type TokenTotals = {
  input: bigint;
  output: bigint;
  cacheCreation: bigint;
  cacheRead: bigint;
  /** All four added up. */
  total: bigint;
};

/**
 * Someone whose rows the viewer sees: the viewer, or a member of a shared pool. `displayName` is
 * their email; `name` (which they can change) is only ever shown next to it.
 */
export type Person = {
  id: string;
  displayName: string;
  name: string | null;
  isViewer: boolean;
};

/** The `_sum` block for session token totals and cost. */
export const SESSION_SUMS = {
  inputTokens: true,
  outputTokens: true,
  cacheCreationTokens: true,
  cacheReadTokens: true,
  costUsd: true,
} as const;

type TokenColumns = {
  inputTokens: bigint | null;
  outputTokens: bigint | null;
  cacheCreationTokens: bigint | null;
  cacheReadTokens: bigint | null;
};

export function tokenTotals(row: TokenColumns | null | undefined): TokenTotals {
  const input = row?.inputTokens ?? 0n;
  const output = row?.outputTokens ?? 0n;
  const cacheCreation = row?.cacheCreationTokens ?? 0n;
  const cacheRead = row?.cacheReadTokens ?? 0n;
  return {
    input,
    output,
    cacheCreation,
    cacheRead,
    total: input + output + cacheCreation + cacheRead,
  };
}

export function decimalToNumber(
  value: Prisma.Decimal | null | undefined,
): number | null {
  return value == null ? null : value.toNumber();
}

export function daysBefore(now: Date, days: number): Date {
  return new Date(now.getTime() - days * 24 * 60 * 60 * 1000);
}

/** Display names for the given users, as the viewer should see them. */
export async function peopleById(
  db: Db,
  viewerId: string,
  ids: Iterable<string>,
): Promise<Map<string, Person>> {
  const unique = [...new Set(ids)];
  if (unique.length === 0) return new Map();
  const users = await db.user.findMany({
    where: { id: { in: unique } },
    select: { id: true, name: true, email: true },
  });
  return new Map(
    users.map((u) => [
      u.id,
      { id: u.id, ...memberLabel(u), isViewer: u.id === viewerId },
    ]),
  );
}

export function personOrUnknown(
  people: Map<string, Person>,
  id: string,
  viewerId: string,
): Person {
  return (
    people.get(id) ?? {
      id,
      displayName: "Member",
      name: null,
      isViewer: id === viewerId,
    }
  );
}
