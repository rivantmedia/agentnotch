/**
 * tRPC setup for the website's own pages (the Mac app uses the plain routes in
 * src/app/api/app/v1 instead).
 *
 * The context resolves the viewer once per request (a Supabase session cookie, or a bearer
 * token). `protectedProcedure` refuses anonymous callers and hands resolvers the viewer's access
 * scope, the only thing reads may filter by.
 */
import { initTRPC, TRPCError } from "@trpc/server";
import superjson from "superjson";

import { env } from "~/env";
import { publicErrorShape } from "~/server/api/http";
import { getViewer, type Viewer } from "~/server/auth/viewer";
import { db } from "~/server/db";
import { type PrismaClient } from "~/server/db-types";
import { AccessDenied, loadAccessScope } from "~/server/services/access";

/**
 * 1. CONTEXT
 *
 * @see https://trpc.io/docs/server/context
 */
export const createTRPCContext = async (opts: {
  headers: Headers;
}): Promise<TRPCContext> => {
  return {
    db,
    viewer: await getViewer(opts.headers),
    headers: opts.headers,
  };
};

export type TRPCContext = {
  db: PrismaClient;
  viewer: Viewer | null;
  headers: Headers;
};

/**
 * 2. INITIALIZATION
 */
const t = initTRPC.context<TRPCContext>().create({
  transformer: superjson,
  // In production an unexpected error's text (Prisma, Postgres) never reaches the browser; the
  // route handler logs it instead.
  errorFormatter({ shape, error }) {
    return publicErrorShape(shape, error, env.NODE_ENV === "production");
  },
});

/**
 * Create a server-side caller.
 *
 * @see https://trpc.io/docs/server/server-side-calls
 */
export const createCallerFactory = t.createCallerFactory;

/**
 * 3. ROUTER & PROCEDURE
 */
export const createTRPCRouter = t.router;

/** Logs each call's duration in development. */
const timingMiddleware = t.middleware(async ({ next, path }) => {
  const start = Date.now();
  const result = await next();
  if (env.NODE_ENV === "development") {
    console.log(`[TRPC] ${path} took ${Date.now() - start}ms to execute`);
  }
  return result;
});

/** Turns a refusal from the access rules into the matching tRPC error. */
const accessErrors = t.middleware(async ({ next }) => {
  const result = await next();
  if (!result.ok && result.error.cause instanceof AccessDenied) {
    throw new TRPCError({
      code: result.error.cause.code,
      message: result.error.cause.message,
      cause: result.error.cause,
    });
  }
  return result;
});

/** Anyone; `ctx.viewer` may be null. */
export const publicProcedure = t.procedure.use(timingMiddleware);

/** Signed-in viewers only; `ctx.viewer` is set and `ctx.scope()` loads what they may see. */
export const protectedProcedure = t.procedure
  .use(timingMiddleware)
  .use(accessErrors)
  .use(({ ctx, next }) => {
    if (!ctx.viewer) {
      throw new TRPCError({ code: "UNAUTHORIZED", message: "Sign in again." });
    }
    const viewer = ctx.viewer;
    let scope: ReturnType<typeof loadAccessScope> | undefined;
    return next({
      ctx: {
        viewer,
        /** The viewer's access scope, loaded once per call. */
        scope: () => (scope ??= loadAccessScope(ctx.db, viewer.id)),
      },
    });
  });
