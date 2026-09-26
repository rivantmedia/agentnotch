import { z } from "zod";

import {
  DELETE_DATA_CONFIRMATION,
  REMOVE_SUMMARIES_CONFIRMATION,
} from "~/lib/data-controls";
import { createTRPCRouter, protectedProcedure } from "~/server/api/trpc";
import { deleteSyncedData, removeSummaries } from "~/server/services/my-data";

/** The viewer's own data, removed from /settings. Each needs its phrase, as typed there. */
export const myDataRouter = createTRPCRouter({
  /** Clears the summary of every session the viewer synced. */
  removeSummaries: protectedProcedure
    .input(z.object({ confirm: z.literal(REMOVE_SUMMARIES_CONFIRMATION) }))
    .mutation(({ ctx }) => removeSummaries(ctx.db, ctx.viewer.id)),

  /** Deletes everything the viewer's Macs synced, their pools and memberships included. */
  deleteAll: protectedProcedure
    .input(z.object({ confirm: z.literal(DELETE_DATA_CONFIRMATION) }))
    .mutation(({ ctx }) => deleteSyncedData(ctx.db, ctx.viewer.id)),
});
