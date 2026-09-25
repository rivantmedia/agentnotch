import { z } from "zod";

import {
  accountKeyInput,
  storableText,
  usageSourceInput,
} from "~/server/api/routers/inputs";
import { createTRPCRouter, protectedProcedure } from "~/server/api/trpc";
import { usageHistory } from "~/server/services/usage";

export const usageRouter = createTRPCRouter({
  /** Usage-limit readings for charts, one series per window (default: the last 7 days). */
  history: protectedProcedure
    .input(
      z
        .object({
          accountKey: accountKeyInput,
          windowId: storableText(z.string().min(1).max(64)).optional(),
          source: usageSourceInput.optional(),
          from: z.date().optional(),
          to: z.date().optional(),
        })
        .refine((v) => !v.from || !v.to || v.from <= v.to, {
          message: "`from` must not be after `to`",
          path: ["from"],
        }),
    )
    .query(async ({ ctx, input }) =>
      usageHistory(ctx.db, await ctx.scope(), input),
    ),
});
