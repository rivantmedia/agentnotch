import { type NextRequest } from "next/server";

import { env } from "~/env";
import { handleTrpcRequest } from "~/server/api/http";
import { appRouter } from "~/server/api/root";
import { createTRPCContext } from "~/server/api/trpc";

/**
 * The website's tRPC endpoint. Batch limits and error logging live in handleTrpcRequest
 * (src/server/api/http.ts).
 */
const handler = (req: NextRequest) =>
  handleTrpcRequest(req, {
    router: appRouter,
    createContext: () => createTRPCContext({ headers: req.headers }),
    verbose: env.NODE_ENV === "development",
  });

export { handler as GET, handler as POST };
