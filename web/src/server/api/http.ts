/**
 * How tRPC is served over HTTP (src/app/api/trpc/[trpc]/route.ts), kept apart from the routers so
 * the limits can be tested on their own.
 *
 * - A batch runs at most TRPC_MAX_BATCH_SIZE procedures, so one request can't fan out into
 *   hundreds of database queries. The client's batch link uses the same limit.
 * - Only GETs (queries) may be batched. Mutations are POSTs and go one per request: the client
 *   sends them through a plain link, and each counts against its own limits (pools.join counts
 *   failed codes per user).
 * - Errors are logged on the server; what reaches the client is shaped by `publicErrorShape`.
 */
import { type AnyTRPCRouter, type TRPCError } from "@trpc/server";
import {
  fetchRequestHandler,
  type FetchCreateContextFn,
} from "@trpc/server/adapters/fetch";
import { ZodError } from "zod";

import { TRPC_MAX_BATCH_SIZE } from "./limits";

export { TRPC_MAX_BATCH_SIZE };
export const TRPC_ENDPOINT = "/api/trpc";

/** Said instead of an unexpected error's own message, which may quote SQL or Prisma. */
export const GENERIC_INTERNAL_MESSAGE =
  "Something went wrong on our side. Try again in a moment.";

export function handleTrpcRequest<TRouter extends AnyTRPCRouter>(
  req: Request,
  options: {
    router: TRouter;
    createContext: FetchCreateContextFn<TRouter>;
    log?: (message: string, error: unknown) => void;
    /** Log every failure, not just unexpected ones (development). */
    verbose?: boolean;
  },
): Promise<Response> {
  const log =
    options.log ?? ((message, error) => console.error(message, error));
  return fetchRequestHandler({
    endpoint: TRPC_ENDPOINT,
    req,
    router: options.router,
    createContext: options.createContext,
    maxBatchSize: TRPC_MAX_BATCH_SIZE,
    allowBatching: req.method === "GET",
    onError: ({ path, error }) => {
      if (options.verbose || error.code === "INTERNAL_SERVER_ERROR") {
        log(
          `[tRPC] ${path ?? "<no path>"} failed: ${error.message}`,
          error.cause ?? error,
        );
      }
    },
  });
}

/**
 * The error shape sent to the browser: tRPC's own, plus the zod issues for bad input. When
 * `hideInternals` is set (production), an unexpected error's message is replaced by a generic
 * one; refusals the services phrase for people (NOT_FOUND, FORBIDDEN, …) keep theirs.
 */
export function publicErrorShape<
  TShape extends { message: string; data: object },
>(shape: TShape, error: TRPCError, hideInternals: boolean) {
  const internal = error.code === "INTERNAL_SERVER_ERROR";
  return {
    ...shape,
    message:
      internal && hideInternals ? GENERIC_INTERNAL_MESSAGE : shape.message,
    data: {
      ...shape.data,
      zodError: error.cause instanceof ZodError ? error.cause.flatten() : null,
    },
  };
}
