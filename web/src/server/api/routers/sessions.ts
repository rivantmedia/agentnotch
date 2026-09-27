import { z } from "zod";

import {
  accountKeyInput,
  idInput,
  searchInput,
  sessionSourceInput,
  storableText,
} from "~/server/api/routers/inputs";
import { createTRPCRouter, protectedProcedure } from "~/server/api/trpc";
import { getSession, listSessions } from "~/server/services/sessions";

export const sessionsRouter = createTRPCRouter({
  /**
   * A page of sessions, newest first (a start dated in the future counts as now). Pass
   * `nextCursor` back as `cursor` for the next page: it carries the first page's clock, so the
   * walk has no gaps or repeats. `projectId` selects the project's whole group: its owner's rows
   * of that folder name on the account, from every Mac; with `acrossAccounts`, on every account
   * the viewer sees them on.
   */
  list: protectedProcedure
    .input(
      z.object({
        accountKey: accountKeyInput.optional(),
        projectId: idInput.optional(),
        acrossAccounts: z.boolean().optional(),
        ownerId: idInput.optional(),
        source: sessionSourceInput.optional(),
        from: z.date().optional(),
        to: z.date().optional(),
        running: z.boolean().optional(),
        search: searchInput.optional(),
        // A time and a row id (services/sessions.ts, `sessionCursor`).
        cursor: storableText(z.string().min(1).max(160)).nullish(),
        limit: z.number().int().min(1).max(100).default(25),
      }),
    )
    .query(async ({ ctx, input }) =>
      listSessions(ctx.db, await ctx.scope(), {
        ...input,
        cursor: input.cursor ?? undefined,
      }),
    ),

  get: protectedProcedure
    .input(z.object({ id: idInput }))
    .query(async ({ ctx, input }) =>
      getSession(ctx.db, await ctx.scope(), input.id),
    ),
});
