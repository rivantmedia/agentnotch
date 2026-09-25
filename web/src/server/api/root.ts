import { accountsRouter } from "~/server/api/routers/accounts";
import { poolsRouter } from "~/server/api/routers/pools";
import { projectsRouter } from "~/server/api/routers/projects";
import { sessionsRouter } from "~/server/api/routers/sessions";
import { usageRouter } from "~/server/api/routers/usage";
import { viewerRouter } from "~/server/api/routers/viewer";
import { createCallerFactory, createTRPCRouter } from "~/server/api/trpc";

/**
 * The website's API for its own pages. The Mac app does not use it (see src/app/api/app/v1).
 */
export const appRouter = createTRPCRouter({
  viewer: viewerRouter,
  accounts: accountsRouter,
  sessions: sessionsRouter,
  projects: projectsRouter,
  usage: usageRouter,
  pools: poolsRouter,
});

// export type definition of API
export type AppRouter = typeof appRouter;

/**
 * A server-side caller, e.g. `createCaller(ctx).accounts.list()`.
 */
export const createCaller = createCallerFactory(appRouter);
