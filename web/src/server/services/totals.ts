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

/** What a period's sessions added up to. */
export type UsageTotals = {
  sessions: number;
  tokens: TokenTotals;
  /** The sum of the sessions' costs (Claude Code's own, else estimated at list prices); null when none has one. */
  costUsd: number | null;
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

/** Parts added up. A cost is null only when no part has one: unknown stays unknown, not zero. */
export function addUp(parts: readonly UsageTotals[]): UsageTotals {
  let sessions = 0;
  let input = 0n;
  let output = 0n;
  let cacheCreation = 0n;
  let cacheRead = 0n;
  let cost: number | null = null;
  for (const part of parts) {
    sessions += part.sessions;
    input += part.tokens.input;
    output += part.tokens.output;
    cacheCreation += part.tokens.cacheCreation;
    cacheRead += part.tokens.cacheRead;
    if (part.costUsd !== null) cost = (cost ?? 0) + part.costUsd;
  }
  return {
    sessions,
    tokens: tokenTotals({
      inputTokens: input,
      outputTokens: output,
      cacheCreationTokens: cacheCreation,
      cacheReadTokens: cacheRead,
    }),
    costUsd: cost,
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
