import { TRPCClientError } from "@trpc/client";
import { TRPCError } from "@trpc/server";
import { describe, expect, it } from "vitest";

import { describeError, errorCode, isPermanentError } from "~/trpc/errors";

function clientError(code: string, message: string) {
  return new TRPCClientError(message, {
    result: { error: { message, code: -32000, data: { code } } },
  });
}

describe("describeError", () => {
  it("reads the code from browser and server-side errors alike", () => {
    expect(errorCode(clientError("NOT_FOUND", "No such pool."))).toBe(
      "NOT_FOUND",
    );
    expect(errorCode(new TRPCError({ code: "FORBIDDEN" }))).toBe("FORBIDDEN");
    expect(errorCode(new Error("plain"))).toBeNull();
    expect(errorCode("nope")).toBeNull();
  });

  it("sends signed-out people to sign in instead of retrying", () => {
    expect(
      describeError(clientError("UNAUTHORIZED", "Sign in again.")),
    ).toEqual({
      message: "Your session ended. Sign in again to continue.",
      signIn: true,
      retry: false,
    });
  });

  it("shows the access rules' own words for refusals", () => {
    const d = describeError(
      clientError("FORBIDDEN", "That code was revoked. Ask for a new one."),
    );
    expect(d.message).toBe("That code was revoked. Ask for a new one.");
    expect(d.retry).toBe(false);
  });

  it("never shows an internal error's message", () => {
    const d = describeError(
      clientError(
        "INTERNAL_SERVER_ERROR",
        "PrismaClientKnownRequestError: P2002 …",
      ),
    );
    expect(d.message).not.toContain("Prisma");
    expect(d.retry).toBe(true);
  });

  it("calls a request that never got a tRPC answer a connection problem", () => {
    const d = describeError(new TRPCClientError("Failed to fetch"));
    expect(d.message).toMatch(/connection/);
    expect(d.retry).toBe(true);
  });

  it("marks refusals as permanent so queries don't retry them", () => {
    for (const code of [
      "UNAUTHORIZED",
      "FORBIDDEN",
      "NOT_FOUND",
      "BAD_REQUEST",
    ]) {
      expect(isPermanentError(clientError(code, "x"))).toBe(true);
    }
    expect(isPermanentError(new TRPCError({ code: "NOT_FOUND" }))).toBe(true);
    expect(isPermanentError(clientError("INTERNAL_SERVER_ERROR", "x"))).toBe(
      false,
    );
    expect(isPermanentError(new TRPCClientError("Failed to fetch"))).toBe(
      false,
    );
  });
});
