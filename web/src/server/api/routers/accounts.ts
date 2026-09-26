import { z } from "zod";

import { accountKeyInput } from "~/server/api/routers/inputs";
import { createTRPCRouter, protectedProcedure } from "~/server/api/trpc";
import { canSeeAccount } from "~/server/services/access";
import { getAccount, listAccounts } from "~/server/services/accounts";

export const accountsRouter = createTRPCRouter({
  /** Every Claude account the viewer synced or shares through a pool. */
  list: protectedProcedure.query(async ({ ctx }) =>
    listAccounts(ctx.db, await ctx.scope()),
  ),

  /**
   * Whether the viewer may see an account: the cheap check the account page makes before it
   * streams anything, so an account they can't see answers 404.
   */
  visible: protectedProcedure
    .input(z.object({ accountKey: accountKeyInput }))
    .query(async ({ ctx, input }) =>
      canSeeAccount(await ctx.scope(), input.accountKey),
    ),

  /** One account, with the people whose data it includes. */
  get: protectedProcedure
    .input(z.object({ accountKey: accountKeyInput }))
    .query(async ({ ctx, input }) =>
      getAccount(ctx.db, await ctx.scope(), input.accountKey),
    ),
});
