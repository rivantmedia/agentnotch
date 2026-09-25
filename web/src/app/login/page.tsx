import { type Metadata } from "next";
import Link from "next/link";
import { redirect } from "next/navigation";

import { isAcceptedSignIn } from "~/lib/auth-claims";
import { safeNextPath } from "~/lib/auth-paths";
import { createClient } from "~/lib/supabase/server";

import { LogoMark } from "../_components/logo";
import { GoogleSignInButton } from "./google-sign-in-button";

export const metadata: Metadata = { title: "Sign in" };

/** What each `?error=` from the OAuth callback means for the person signing in. */
function errorText(code: string): string {
  switch (code) {
    case "signin":
      return "Sign-in didn't finish: it was cancelled, or Google or Supabase turned it down. Try again.";
    case "session":
      return "Your session ended. Sign in again to continue.";
    case "provider":
      return "Agent Notch accepts Google sign-ins only. Continue with Google.";
    default:
      return "Something went wrong while signing you in. Try again.";
  }
}

export default async function LoginPage({
  searchParams,
}: {
  searchParams: Promise<{
    next?: string | string[];
    error?: string | string[];
  }>;
}) {
  const params = await searchParams;
  const next = safeNextPath(first(params.next));

  const supabase = await createClient();
  const { data } = await supabase.auth.getClaims();
  if (isAcceptedSignIn(data?.claims)) redirect(next);
  // Signed in some other way (only Google counts): say so rather than loop through here.
  const error = data?.claims.sub ? "provider" : first(params.error);

  return (
    <div className="flex justify-center px-4 py-16 sm:py-24">
      <div className="card flex w-full max-w-sm flex-col gap-6 p-6 shadow-sm sm:p-8">
        <div className="flex flex-col gap-3">
          <LogoMark className="size-9" />
          <h1 className="text-2xl font-semibold tracking-tight">
            Sign in to Agent Notch
          </h1>
          <p className="text-sm text-ink-2">
            See what your Claude accounts were used for, across every Mac you
            sync.
          </p>
        </div>
        {error !== undefined ? (
          <div
            role="alert"
            className="rounded-lg border border-critical/40 bg-critical-soft p-3 text-sm text-critical-ink"
          >
            {errorText(error)}
          </div>
        ) : null}
        <GoogleSignInButton next={next} />
        <p className="text-xs text-ink-3">
          Google is used only to sign you in: your name and email.{" "}
          <Link href="/#privacy" className="link">
            What the app sends
          </Link>
        </p>
      </div>
    </div>
  );
}

function first(value: string | string[] | undefined): string | undefined {
  return Array.isArray(value) ? value[0] : value;
}
