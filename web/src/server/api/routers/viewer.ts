import {
  createTRPCRouter,
  protectedProcedure,
  publicProcedure,
} from "~/server/api/trpc";
import { displayName, memberLabel } from "~/server/services/users";

export const viewerRouter = createTRPCRouter({
  /** The signed-in viewer, or null. For page chrome that renders either way. */
  current: publicProcedure.query(({ ctx }) => {
    if (!ctx.viewer) return null;
    const { id, email, name } = ctx.viewer;
    return { id, email, name, displayName: displayName(ctx.viewer) };
  }),

  /** The viewer with the Macs their app has synced from. */
  me: protectedProcedure.query(async ({ ctx }) => {
    const [user, devices] = await Promise.all([
      ctx.db.user.findUniqueOrThrow({
        where: { id: ctx.viewer.id },
        select: { id: true, email: true, name: true, createdAt: true },
      }),
      ctx.db.device.findMany({
        where: { userId: ctx.viewer.id },
        orderBy: { lastSeenAt: "desc" },
        select: { id: true, name: true, appVersion: true, lastSeenAt: true },
      }),
    ]);
    return {
      ...user,
      displayName: displayName(user),
      /** How the viewer appears to the other members of their pools. */
      memberLabel: memberLabel(user),
      devices,
    };
  }),
});
