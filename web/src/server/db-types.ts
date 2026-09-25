/**
 * The generated Prisma client's types, re-exported so services can take a client as a parameter
 * (tests pass their own) without importing the app-wide instance in src/server/db.ts.
 */
export { Prisma, PrismaClient } from "../../generated/prisma";
export type {
  Pool,
  PoolMember,
  Project,
  Session,
  UsageReading,
  User,
  UserAccount,
} from "../../generated/prisma";

import type { Prisma, PrismaClient } from "../../generated/prisma";

/** A client or an interactive transaction: anything the services can query through. */
export type Db = PrismaClient | Prisma.TransactionClient;
