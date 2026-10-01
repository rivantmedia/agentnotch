import { z } from "zod";

import {
  accountKeyInput,
  accountKeysInput,
  storableText,
  timeZoneInput,
  usagePeriodInput,
  usageSourceInput,
} from "~/server/api/routers/inputs";
import { createTRPCRouter, protectedProcedure } from "~/server/api/trpc";
import { combinedUsage, usageTimeline } from "~/server/services/combined-usage";
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

  /**
   * The period's sessions on some accounts (every account the viewer sees when `accountKeys` is
   * left out) added up, and each account's part. Keys the viewer can't see are left out, as
   * missing ones are.
   */
  combined: protectedProcedure
    .input(
      z.object({
        period: usagePeriodInput,
        accountKeys: accountKeysInput.optional(),
      }),
    )
    .query(async ({ ctx, input }) =>
      combinedUsage(ctx.db, await ctx.scope(), input),
    ),

  /** The same over time, in days, weeks or months of the viewer's calendar. */
  timeline: protectedProcedure
    .input(
      z.object({
        period: usagePeriodInput,
        accountKeys: accountKeysInput.optional(),
        timeZone: timeZoneInput,
      }),
    )
    .query(async ({ ctx, input }) =>
      usageTimeline(ctx.db, await ctx.scope(), input),
    ),
});
