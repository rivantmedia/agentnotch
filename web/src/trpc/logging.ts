/**
 * Whether the browser's tRPC client logs its calls to the console: in development only. In
 * production a refusal the page expects and handles (NOT_FOUND for an account you can't see,
 * TOO_MANY_REQUESTS for a code that didn't work) would otherwise print as an error, and the page
 * already tells people about anything that failed.
 */
export function shouldLogTrpc(nodeEnv: string | undefined): boolean {
  return nodeEnv === "development";
}
