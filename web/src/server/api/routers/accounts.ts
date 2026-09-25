import { z } from "zod";

import { accountKeyInput } from "~/server/api/routers/inputs";
import { createTRPCRouter, protectedProcedure } from "~/server/api/trpc";
import { getAccount, listAccounts } from "~/server/services/accounts";

export const accountsRouter = createTRPCRouter({
  /** Every Claude account the viewer synced or shares through a pool. */
  list: protectedProcedure.query(async ({ ctx }) =>
    listAccounts(ctx.db, await ctx.scope()),
  ),

  /** One account, with the people whose data it includes. */
  get: protectedProcedure
    .input(z.object({ accountKey: accountKeyInput }))
    .query(async ({ ctx, input }) =>
      getAccount(ctx.db, await ctx.scope(), input.accountKey),
    ),
});
