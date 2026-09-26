"use client";

import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import {
  httpBatchStreamLink,
  httpLink,
  loggerLink,
  splitLink,
} from "@trpc/client";
import { createTRPCReact } from "@trpc/react-query";
import { type inferRouterInputs, type inferRouterOutputs } from "@trpc/server";
import { useState } from "react";
import SuperJSON from "superjson";

import { TRPC_MAX_BATCH_SIZE } from "~/server/api/limits";
import { type AppRouter } from "~/server/api/root";
import { shouldLogTrpc } from "./logging";
import { createQueryClient } from "./query-client";

let clientQueryClientSingleton: QueryClient | undefined = undefined;
const getQueryClient = () => {
  if (typeof window === "undefined") {
    // Server: always make a new query client
    return createQueryClient();
  }
  // Browser: use singleton pattern to keep the same query client
  clientQueryClientSingleton ??= createQueryClient();

  return clientQueryClientSingleton;
};

export const api = createTRPCReact<AppRouter>();

/**
 * Inference helper for inputs.
 *
 * @example type HelloInput = RouterInputs['example']['hello']
 */
export type RouterInputs = inferRouterInputs<AppRouter>;

/**
 * Inference helper for outputs.
 *
 * @example type HelloOutput = RouterOutputs['example']['hello']
 */
export type RouterOutputs = inferRouterOutputs<AppRouter>;

export function TRPCReactProvider(props: { children: React.ReactNode }) {
  const queryClient = getQueryClient();

  const [trpcClient] = useState(() =>
    api.createClient({
      links: [
        loggerLink({ enabled: () => shouldLogTrpc(process.env.NODE_ENV) }),
        // The server batches queries only, and at most TRPC_MAX_BATCH_SIZE at a time
        // (src/server/api/http.ts); mutations go one per request.
        splitLink({
          condition: (op) => op.type === "mutation",
          true: httpLink({
            transformer: SuperJSON,
            url: getBaseUrl() + "/api/trpc",
            headers: trpcHeaders,
          }),
          false: httpBatchStreamLink({
            transformer: SuperJSON,
            url: getBaseUrl() + "/api/trpc",
            maxItems: TRPC_MAX_BATCH_SIZE,
            headers: trpcHeaders,
          }),
        }),
      ],
    }),
  );

  return (
    <QueryClientProvider client={queryClient}>
      <api.Provider client={trpcClient} queryClient={queryClient}>
        {props.children}
      </api.Provider>
    </QueryClientProvider>
  );
}

function trpcHeaders() {
  const headers = new Headers();
  headers.set("x-trpc-source", "nextjs-react");
  return headers;
}

function getBaseUrl() {
  if (typeof window !== "undefined") return window.location.origin;
  if (process.env.VERCEL_URL) return `https://${process.env.VERCEL_URL}`;
  return `http://localhost:${process.env.PORT ?? 3000}`;
}
