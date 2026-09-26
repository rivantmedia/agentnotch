import { z } from "zod";

import { env } from "~/env";
import {
  accountKeyInput,
  idInput,
  storableText,
} from "~/server/api/routers/inputs";
import { createTRPCRouter, protectedProcedure } from "~/server/api/trpc";
import { clientIpKey } from "~/server/client-ip";
import {
  createPoolCode,
  joinPool,
  leavePool,
  listPools,
  removePoolMember,
  revokePoolCode,
} from "~/server/services/pools";

const poolInput = z.object({ poolId: idInput });

export const poolsRouter = createTRPCRouter({
  /** Pools the viewer created or joined, with members (by email, with their name). */
  list: protectedProcedure.query(({ ctx }) => listPools(ctx.db, ctx.viewer.id)),

  /**
   * The viewer's working share code for an account they synced: the current one while it works,
   * else a new one (for the same pool, when they have one).
   */
  create: protectedProcedure
    .input(z.object({ accountKey: accountKeyInput }))
    .mutation(({ ctx, input }) =>
      createPoolCode(ctx.db, ctx.viewer.id, input.accountKey),
    ),

  /** Stops the code from being redeemed; current members stay. Creator only. */
  revoke: protectedProcedure
    .input(poolInput)
    .mutation(({ ctx, input }) =>
      revokePoolCode(ctx.db, ctx.viewer.id, input.poolId),
    ),

  /**
   * Redeems a code (case, dashes and O/0, I/L/1 mix-ups don't matter). Failed attempts are
   * counted per user and per client IP address; too many in an hour answer TOO_MANY_REQUESTS.
   */
  join: protectedProcedure
    .input(z.object({ code: storableText(z.string().min(1).max(32)) }))
    .mutation(({ ctx, input }) =>
      joinPool(
        ctx.db,
        ctx.viewer.id,
        input.code,
        new Date(),
        clientIpKey(ctx.headers, env.RATE_LIMIT_PEPPER),
      ),
    ),

  /** Leaves a pool; the creator leaving deletes it. */
  leave: protectedProcedure
    .input(poolInput)
    .mutation(({ ctx, input }) =>
      leavePool(ctx.db, ctx.viewer.id, input.poolId),
    ),

  /** Removes another member and revokes the current code, so they can't rejoin with it. */
  removeMember: protectedProcedure
    .input(poolInput.extend({ userId: idInput }))
    .mutation(({ ctx, input }) =>
      removePoolMember(ctx.db, ctx.viewer.id, input.poolId, input.userId),
    ),
});
