import { z } from "zod";

import {
  accountKeyInput,
  idInput,
  searchInput,
  sessionSourceInput,
} from "~/server/api/routers/inputs";
import { createTRPCRouter, protectedProcedure } from "~/server/api/trpc";
import { getSession, listSessions } from "~/server/services/sessions";

export const sessionsRouter = createTRPCRouter({
  /** A page of sessions, newest first. Pass `nextCursor` back as `cursor` for the next page. */
  list: protectedProcedure
    .input(
      z.object({
        accountKey: accountKeyInput.optional(),
        projectId: idInput.optional(),
        ownerId: idInput.optional(),
        source: sessionSourceInput.optional(),
        from: z.date().optional(),
        to: z.date().optional(),
        running: z.boolean().optional(),
        search: searchInput.optional(),
        cursor: idInput.nullish(),
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
