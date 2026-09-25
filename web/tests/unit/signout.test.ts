/**
 * Signing out of the website ends this browser's session only: the Mac app's session (same
 * Supabase user) must survive it, so the scope is "local", never Supabase's default "global".
 */
import { NextRequest } from "next/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

const signOut = vi.fn(async () => ({ error: null }));
vi.mock("~/lib/supabase/server", () => ({
  createClient: async () => ({ auth: { signOut } }),
}));

const { POST } = await import("~/app/auth/signout/route");

const SITE = "https://notch.example.com";

beforeEach(() => signOut.mockClear());

describe("POST /auth/signout", () => {
  it("signs out this browser only (scope local) and goes to /login", async () => {
    const response = await POST(
      new NextRequest(`${SITE}/auth/signout`, {
        method: "POST",
        headers: { origin: SITE, host: "notch.example.com" },
      }),
    );
    expect(signOut).toHaveBeenCalledExactlyOnceWith({ scope: "local" });
    expect(response.status).toBe(303);
    expect(response.headers.get("location")).toBe(`${SITE}/login`);
  });

  it("refuses a cross-site post without signing anyone out", async () => {
    const response = await POST(
      new NextRequest(`${SITE}/auth/signout`, {
        method: "POST",
        headers: { origin: "https://evil.example", host: "notch.example.com" },
      }),
    );
    expect(response.status).toBe(403);
    expect(signOut).not.toHaveBeenCalled();
  });
});
