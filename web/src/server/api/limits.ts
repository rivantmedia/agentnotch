/**
 * Limits the tRPC server and the browser's client must agree on. Plain constants, so the client
 * bundle can import them without pulling in any server code.
 */

/** The most procedures one batched request may run (src/server/api/http.ts). */
export const TRPC_MAX_BATCH_SIZE = 10;
