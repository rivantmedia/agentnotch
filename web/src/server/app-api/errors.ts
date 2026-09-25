/**
 * The contract's error shape: `{"error": {"code", "message"}}` with a fixed status per code.
 */
import { type ZodError } from "zod";

import { type AppErrorCode } from "./schema";

export const APP_ERROR_STATUS: Record<AppErrorCode, number> = {
  UNAUTHORIZED: 401,
  FORBIDDEN: 403,
  BAD_REQUEST: 400,
  PAYLOAD_TOO_LARGE: 413,
  RATE_LIMITED: 429,
  INTERNAL: 500,
};

/**
 * The other status a code may take (contract/README.md, "Conventions"): INTERNAL is 503 when the
 * website couldn't check the sign-in right now, which the app must retry, not treat as signed out.
 */
const ALTERNATE_STATUS: Partial<Record<AppErrorCode, readonly number[]>> = {
  INTERNAL: [503],
};

/** Thrown anywhere below a route handler; `toErrorResponse` turns it into the contract shape. */
export class AppApiError extends Error {
  readonly code: AppErrorCode;
  readonly headers: Record<string, string>;
  readonly status: number;

  constructor(
    code: AppErrorCode,
    message: string,
    headers: Record<string, string> = {},
    status?: number,
  ) {
    super(message);
    this.name = "AppApiError";
    this.code = code;
    this.headers = headers;
    this.status =
      status !== undefined && ALTERNATE_STATUS[code]?.includes(status)
        ? status
        : APP_ERROR_STATUS[code];
  }
}

/** The sign-in couldn't be checked right now: 503, and the app keeps its session. */
export function tokenCheckUnavailable(): AppApiError {
  return new AppApiError(
    "INTERNAL",
    "The website couldn't check your sign-in just now. It will try again shortly.",
    { "Retry-After": "30" },
    503,
  );
}

/** Responses of this API are per-user and must never be cached by a CDN. */
export const NO_STORE = { "Cache-Control": "no-store" } as const;

export function errorResponse(
  code: AppErrorCode,
  message: string,
  headers: Record<string, string> = {},
  status: number = APP_ERROR_STATUS[code],
): Response {
  return Response.json(
    { error: { code, message } },
    { status, headers: { ...NO_STORE, ...headers } },
  );
}

/**
 * Anything thrown in a handler. Unknown errors become INTERNAL without their details (they may
 * name tables or values); the caller logs them.
 */
export function toErrorResponse(error: unknown): Response {
  if (error instanceof AppApiError) {
    return errorResponse(
      error.code,
      error.message,
      error.headers,
      error.status,
    );
  }
  return errorResponse("INTERNAL", "Something went wrong. Try again later.");
}

/** A short, readable account of what failed validation: the first few paths and messages. */
export function describeZodError(error: ZodError, max = 3): string {
  const parts = error.issues.slice(0, max).map((issue) => {
    const path = issue.path.join(".");
    return path ? `${path}: ${issue.message}` : issue.message;
  });
  const more = error.issues.length - parts.length;
  return parts.join("; ") + (more > 0 ? ` (and ${more} more)` : "");
}
