import { z } from "zod";

import { accountKeyInput, idInput } from "~/server/api/routers/inputs";
import { createTRPCRouter, protectedProcedure } from "~/server/api/trpc";
import { getProject, listProjects } from "~/server/services/projects";

export const projectsRouter = createTRPCRouter({
  /** Projects on one account with session counts, token totals, last use and latest summaries. */
  list: protectedProcedure
    .input(z.object({ accountKey: accountKeyInput }))
    .query(async ({ ctx, input }) =>
      listProjects(ctx.db, await ctx.scope(), input.accountKey),
    ),

  get: protectedProcedure
    .input(z.object({ id: idInput }))
    .query(async ({ ctx, input }) =>
      getProject(ctx.db, await ctx.scope(), input.id),
    ),
});
