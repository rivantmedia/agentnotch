/**
 * Helpers for the website's server-rendered pages.
 */
import "server-only";

import { TRPCError } from "@trpc/server";
import { headers } from "next/headers";
import { redirect } from "next/navigation";

import { env } from "~/env";
import { siteAddressFrom } from "~/lib/site-address";
import { api } from "~/trpc/server";

/**
 * The signed-in viewer, or a redirect to /login that comes back to `returnTo`. The middleware
 * already sends signed-out visitors away; this covers a session that ended in between.
 */
export async function requireViewer(returnTo: string) {
  const viewer = await api.viewer.current();
  if (!viewer) {
    redirect(`/login?next=${encodeURIComponent(returnTo)}`);
  }
  return viewer;
}

/** The address to paste into the Mac app, or null when it can't be told. */
export async function siteAddress(): Promise<string | null> {
  // Optional chaining: a build made with SKIP_ENV_VALIDATION may run without the variable.
  return siteAddressFrom(env.NEXT_PUBLIC_SITE_URL ?? null, await headers());
}

export function isTRPCCode(error: unknown, code: TRPCError["code"]): boolean {
  return error instanceof TRPCError && error.code === code;
}
