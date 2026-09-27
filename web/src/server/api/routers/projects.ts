import { z } from "zod";

import {
  accountKeyInput,
  idInput,
  usagePeriodInput,
} from "~/server/api/routers/inputs";
import { createTRPCRouter, protectedProcedure } from "~/server/api/trpc";
import {
  canSeeProject,
  projectDetail,
  projectUsage,
} from "~/server/services/project-usage";
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

  /**
   * Usage by project in a period: each person's folders of one name across every account the
   * viewer sees (or on `accountKey` only), with their split by account, most tokens first. With
   * `limit`, the projects past it are added up as `rest`.
   */
  usage: protectedProcedure
    .input(
      z.object({
        period: usagePeriodInput,
        accountKey: accountKeyInput.optional(),
        limit: z.number().int().min(1).max(100).optional(),
      }),
    )
    .query(async ({ ctx, input }) =>
      projectUsage(ctx.db, await ctx.scope(), input),
    ),

  /**
   * The project a project row id belongs to, across every account the viewer sees it on, with
   * its usage in the period by account.
   */
  detail: protectedProcedure
    .input(z.object({ id: idInput, period: usagePeriodInput }))
    .query(async ({ ctx, input }) =>
      projectDetail(ctx.db, await ctx.scope(), input.id, input.period),
    ),

  /**
   * Whether the viewer may see a project row: the cheap check the project page makes before it
   * streams anything, so a project they can't see answers 404.
   */
  visible: protectedProcedure
    .input(z.object({ id: idInput }))
    .query(async ({ ctx, input }) =>
      canSeeProject(ctx.db, await ctx.scope(), input.id),
    ),
});
