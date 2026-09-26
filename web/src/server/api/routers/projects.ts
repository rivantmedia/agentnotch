import { z } from "zod";

import { accountKeyInput, idInput } from "~/server/api/routers/inputs";
import { createTRPCRouter, protectedProcedure } from "~/server/api/trpc";
import { getProject, listProjects } from "~/server/services/projects";

export const projectsRouter = createTRPCRouter({
  /**
   * Projects on one account, one per person and folder name (their Macs' rows grouped), with
   * session counts, Macs, token totals, last use and latest summaries.
   */
  list: protectedProcedure
    .input(z.object({ accountKey: accountKeyInput }))
    .query(async ({ ctx, input }) =>
      listProjects(ctx.db, await ctx.scope(), input.accountKey),
    ),

  /** The project a project row id belongs to (its group), as `list` shows it. */
  get: protectedProcedure
    .input(z.object({ id: idInput }))
    .query(async ({ ctx, input }) =>
      getProject(ctx.db, await ctx.scope(), input.id),
    ),
});
