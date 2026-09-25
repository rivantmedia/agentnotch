/**
 * What to tell people when a tRPC call fails. Server messages are shown for the refusals the
 * access rules phrase for people (NOT_FOUND, FORBIDDEN, BAD_REQUEST); anything unexpected gets a
 * generic line, so internals never reach the page.
 */
import { TRPCClientError } from "@trpc/client";

export type ErrorDescription = {
  message: string;
  /** The session ended: offer to sign in again rather than to retry. */
  signIn: boolean;
  /** Retrying can help (network or server trouble). */
  retry: boolean;
};

const PHRASED = new Set(["NOT_FOUND", "FORBIDDEN", "BAD_REQUEST", "CONFLICT"]);

export function errorCode(error: unknown): string | null {
  // A TRPCError (a server-side prefetch through the RSC caller) carries its code directly.
  // Matched by shape, so this module stays free of @trpc/server in the browser bundle.
  if (error instanceof Error && error.name === "TRPCError") {
    const code: unknown = (error as Error & { code?: unknown }).code;
    if (typeof code === "string") return code;
  }
  if (!(error instanceof TRPCClientError)) return null;
  const data: unknown = error.data;
  if (data && typeof data === "object" && "code" in data) {
    const code = data.code;
    return typeof code === "string" ? code : null;
  }
  return null;
}

export function describeError(error: unknown): ErrorDescription {
  const code = errorCode(error);
  if (code === "UNAUTHORIZED") {
    return {
      message: "Your session ended. Sign in again to continue.",
      signIn: true,
      retry: false,
    };
  }
  if (code === "TOO_MANY_REQUESTS") {
    return {
      message: "Too many requests. Wait a moment and try again.",
      signIn: false,
      retry: true,
    };
  }
  if (code && PHRASED.has(code) && error instanceof Error && error.message) {
    return { message: error.message, signIn: false, retry: false };
  }
  if (error instanceof TRPCClientError && code === null) {
    // No tRPC error shape: the request never reached the server, or its answer was not JSON.
    return {
      message:
        "Couldn't reach Agent Notch. Check your connection and try again.",
      signIn: false,
      retry: true,
    };
  }
  return {
    message: "Something went wrong on our side. Try again in a moment.",
    signIn: false,
    retry: true,
  };
}

/** Client errors that retrying won't fix. */
export function isPermanentError(error: unknown): boolean {
  const code = errorCode(error);
  return (
    code === "UNAUTHORIZED" ||
    code === "FORBIDDEN" ||
    code === "NOT_FOUND" ||
    code === "BAD_REQUEST"
  );
}
